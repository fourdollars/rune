use crate::config::RuneConfig;
use crate::provider::{LlmMessage, LlmRequest, LlmResponse, LlmToolCall, ProviderRegistry};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;
use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Agent's stop reason.
#[derive(Debug)]
pub enum StopReason {
    FinalAnswer(String),
    MaxSteps,
    TokenBudgetExhausted,
    Error(String),
    UserInterrupt,
}

/// Result of user confirmation prompt.
enum ConfirmResult {
    Yes,
    No,
    Always,
}

/// The AI Agent — orchestrates LLM calls and tool execution.
pub struct Agent {
    pub config: RuneConfig,
    messages: Vec<LlmMessage>,
    step_count: u32,
    tokens_used: u32,
    provider: ProviderRegistry,
    tools: ToolRegistry,
    skill_loader: SkillLoader,
    auto_approve: bool,
    executed_commands: Vec<String>,
}

impl Agent {
    pub fn new(config: RuneConfig, provider: ProviderRegistry) -> Self {
        let mut tools = ToolRegistry::new(vec![PathBuf::from("/tmp"), PathBuf::from(".")]); tools.set_policy(&config.policy);
        let skill_loader = SkillLoader::new(vec![PathBuf::from(&config.skills_dir)]);
        Agent {
            config,
            messages: Vec::new(),
            step_count: 0,
            tokens_used: 0,
            provider,
            tools,
            skill_loader,
            auto_approve: false,
            executed_commands: Vec::new(),
        }
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: &str) {
        if self.messages.first().map(|m| m.role == "system").unwrap_or(false) {
            self.messages[0].content = Some(prompt.to_string());
        } else {
            self.messages.insert(0, LlmMessage {
                role: "system".to_string(),
                content: Some(prompt.to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    /// Reset conversation state for a new run (keeps system prompt).

    pub fn tokens_used(&self) -> u32 { self.tokens_used }
    pub fn step_count(&self) -> u32 { self.step_count }

    /// Get message count by role.
    pub fn context_summary(&self) -> Vec<(String, usize)> {
        let mut counts = std::collections::HashMap::new();
        for m in &self.messages {
            *counts.entry(m.role.clone()).or_insert(0) += 1;
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort();
        v
    }

    /// Get total message count.
    pub fn message_count(&self) -> usize { self.messages.len() }

    /// Get estimated context size in chars.
    pub fn context_chars(&self) -> usize {
        self.messages.iter().map(|m| m.content.as_ref().map(|c| c.len()).unwrap_or(0)).sum()
    }

    /// Compact context: keep system prompt + summarize older messages.
    pub fn compact(&mut self) {
        if self.messages.len() <= 3 {
            return; // nothing to compact
        }
        let system = self.messages.first().cloned();
        // Keep last 4 messages (2 exchanges)
        let keep_last = 4;
        let total = self.messages.len();
        let to_summarize = if total > keep_last + 1 { total - keep_last - 1 } else { 0 };
        if to_summarize == 0 { return; }

        // Build summary of older messages
        let mut summary_parts: Vec<String> = Vec::new();
        for m in self.messages.iter().skip(1).take(to_summarize) {
            let preview = m.content.as_ref().map(|c| c.chars().take(100).collect::<String>()).unwrap_or_default();
            if !preview.is_empty() {
                summary_parts.push(format!("[{}]: {}", m.role, preview));
            }
        }
        let summary = format!("[Compacted {} messages]
{}", to_summarize, summary_parts.join("
"));

        // Rebuild: system + summary + last N messages
        let mut new_messages = Vec::new();
        if let Some(sys) = system {
            new_messages.push(sys);
        }
        new_messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(summary),
            tool_calls: None,
            tool_call_id: None,
        });
        new_messages.extend(self.messages[total - keep_last..].iter().cloned());
        self.messages = new_messages;
    }
    pub fn reset(&mut self) {
        let system = self.messages.first().cloned();
        self.messages.clear();
        if let Some(sys) = system {
            self.messages.push(sys);
        }
        self.step_count = 0;
        self.tokens_used = 0;
    }

    /// Resolve @skill references in user input and inject skill content as system context.
    fn inject_skills(&mut self, user_input: &str) {
        let skill_refs = SkillLoader::extract_skill_refs(user_input);
        if skill_refs.is_empty() {
            return;
        }

        for name in &skill_refs {
            match self.skill_loader.load(name) {
                Ok(skill) => {
                    info!(skill = %name, "loaded skill");
                    eprintln!("  {} Loaded skill: {}", "📚".dimmed(), name.green());
                    // Inject skill content as a system message
                    self.messages.push(LlmMessage {
                        role: "system".to_string(),
                        content: Some(format!(
                            "[Skill: {}]\n{}\n[End Skill: {}]",
                            skill.metadata.name, skill.content, skill.metadata.name
                        )),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Err(e) => {
                    warn!(skill = %name, error = %e, "failed to load skill");
                    eprintln!("  {} Skill '{}' not found: {}", "⚠".yellow(), name, e);
                }
            }
        }
    }

    /// Run the agent loop: send user input → LLM → tools → repeat until done.
    pub async fn run(&mut self, user_input: &str) -> StopReason {
        // Resolve and inject @skill references
        self.executed_commands.clear();
        self.inject_skills(user_input);

        // Add user message
        self.messages.push(LlmMessage {
            role: "user".to_string(),
            content: Some(user_input.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });

        loop {
            if self.step_count >= self.config.max_steps {
                return StopReason::MaxSteps;
            }
            if self.tokens_used >= self.config.token_budget {
                return StopReason::TokenBudgetExhausted;
            }

            self.step_count += 1;
            info!(step = self.step_count, tokens = self.tokens_used, "agent loop step");

            // Call LLM
            let response = match self.call_llm().await {
                Ok(r) => r,
                Err(e) => return StopReason::Error(format!("LLM call failed: {}", e)),
            };

            // Update token usage
            self.tokens_used += response.usage.total_tokens;

            // If no tool calls, we have our final answer
            if response.tool_calls.is_empty() {
                let answer = response.content.unwrap_or_default();
                self.messages.push(LlmMessage {
                    role: "assistant".to_string(),
                    content: Some(answer.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                return StopReason::FinalAnswer(answer);
            }

            // We have tool calls
            self.messages.push(LlmMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute each tool call
            for tc in &response.tool_calls {
                let result = self.execute_tool_call(tc).await;
                self.messages.push(LlmMessage {
                    role: "tool".to_string(),
                    content: Some(result),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                });
            }
        }
    }

    /// Call the LLM provider.
    async fn call_llm(&self) -> Result<LlmResponse> {
        let tool_defs = self.tools.tool_definitions();
        let request = LlmRequest {
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: if tool_defs.is_empty() { None } else { Some(tool_defs) },
            max_tokens: None,
        };
        debug!(model = %self.config.model, messages = self.messages.len(), "calling LLM");
        self.provider.chat(request).await
    }

    /// Execute a single tool call.
    async fn execute_tool_call(&mut self, tc: &LlmToolCall) -> String {
        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        eprintln!("  {} {}", "⚙".dimmed(), format!("{}({})", tc.function.name, &tc.function.arguments[..tc.function.arguments.len().min(80)]).dimmed());

        // Confirm mode: ask user before executing dangerous tools
        // Track executed commands
        if tc.function.name == "execute_cmd" {
            if let Some(cmd) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                .ok().and_then(|a| a.get("cmd").and_then(|c| c.as_str()).map(|s| s.to_string())) {
                self.executed_commands.push(cmd);
            }
        }

        // Confirm mode: ask user before executing dangerous tools
        if self.config.policy.mode == "confirm" && Self::is_dangerous_tool(&tc.function.name) && !self.auto_approve {
            eprint!("
  {} Execute? [Y/n/A(lways)] ", "⚠".yellow().bold());
            std::io::Write::flush(&mut std::io::stderr()).ok();
            match Self::prompt_confirm_with_always() {
                ConfirmResult::Yes => eprintln!("{}", "approved".green()),
                ConfirmResult::Always => {
                    self.auto_approve = true;
                    eprintln!("{}", "always approve (session)".green());
                }
                ConfirmResult::No => {
                    eprintln!("{}", "denied".red());
                    return "DENIED: user rejected tool execution".to_string();
                }
            }
        }

        let output = self.tools.execute(&tc.function.name, args).await;

        if output.is_error {
            eprintln!("  {} {}", "✗".red(), output.content[..output.content.len().min(200)].dimmed());
        } else {
            eprintln!("  {} {}", "✓".green(), format!("{}...ok", tc.function.name).dimmed());
        }

        output.content
    }

    /// Tools that modify state or execute arbitrary commands.
    fn is_dangerous_tool(name: &str) -> bool {
        matches!(name, "execute_cmd" | "write_file" | "fetch_url")
    }

    /// Prompt user for Y/n confirmation via /dev/tty (bypasses stdin pipe).
    fn prompt_confirm_with_always() -> ConfirmResult {
        use std::io::{BufRead, Write};
        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            let mut reader = std::io::BufReader::new(tty);
            std::io::stderr().flush().ok();
            let mut input = String::new();
            if reader.read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                if trimmed == "a" || trimmed == "always" {
                    return ConfirmResult::Always;
                }
                if trimmed == "n" || trimmed == "no" {
                    return ConfirmResult::No;
                }
                return ConfirmResult::Yes; // empty or y/yes
            }
        }
        // Non-interactive fallback: auto-approve
        ConfirmResult::Yes
    }

    /// Get the list of commands executed during this session.
    pub fn executed_commands(&self) -> &[String] {
        &self.executed_commands
    }
}
