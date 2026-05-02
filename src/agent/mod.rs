use crate::config::RuneConfig;
use crate::provider::{LlmMessage, LlmRequest, LlmResponse, LlmToolCall, ProviderRegistry};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;
use crate::trace::{redact, StepKind, TraceWriter};
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
    interactive: bool,
    trace: Option<TraceWriter>,
    executed_commands: Vec<String>,
    tool_call_names: Vec<String>,
}

impl Agent {
    pub fn new(config: RuneConfig, provider: ProviderRegistry, interactive: bool) -> Self {
        let mut tools = ToolRegistry::new(vec![PathBuf::from("/tmp"), PathBuf::from(".")]);
        tools.set_policy(&config.policy);
        let skill_loader = SkillLoader::new(vec![PathBuf::from(&config.skills_dir)]);
        let trace = if config.trace {
            Some(TraceWriter::new(
                TraceWriter::generate_run_id(),
                config.model.clone(),
                PathBuf::from(".rune/traces"),
                true,
            ))
        } else {
            None
        };
        let auto_approve = config.auto_approve;
        Agent {
            config,
            messages: Vec::new(),
            step_count: 0,
            tokens_used: 0,
            provider,
            tools,
            skill_loader,
            auto_approve,
            interactive,
            trace,
            executed_commands: Vec::new(),
            tool_call_names: Vec::new(),
        }
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: &str) {
        if self
            .messages
            .first()
            .map(|m| m.role == "system")
            .unwrap_or(false)
        {
            self.messages[0].content = Some(prompt.to_string());
        } else {
            self.messages.insert(
                0,
                LlmMessage {
                    role: "system".to_string(),
                    content: Some(prompt.to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
            );
        }
    }

    /// Reset conversation state for a new run (keeps system prompt).
    pub fn tokens_used(&self) -> u32 {
        self.tokens_used
    }
    pub fn step_count(&self) -> u32 {
        self.step_count
    }

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
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get estimated context size in chars.
    pub fn context_chars(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.content.as_ref().map(|c| c.len()).unwrap_or(0))
            .sum()
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
        let to_summarize = if total > keep_last + 1 {
            total - keep_last - 1
        } else {
            0
        };
        if to_summarize == 0 {
            return;
        }

        // Build summary of older messages
        let mut summary_parts: Vec<String> = Vec::new();
        for m in self.messages.iter().skip(1).take(to_summarize) {
            let preview = m
                .content
                .as_ref()
                .map(|c| c.chars().take(100).collect::<String>())
                .unwrap_or_default();
            if !preview.is_empty() {
                summary_parts.push(format!("[{}]: {}", m.role, preview));
            }
        }
        let summary = format!(
            "[Compacted {} messages]
{}",
            to_summarize,
            summary_parts.join(
                "
"
            )
        );

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
                    // Skill safety: restrict tools if skill defines tools_allow
                    if let Some(ref allowed) = skill.metadata.tools_allow {
                        self.tools.set_allowed_domains(vec![]); // reset
                        eprintln!(
                            "    {} tool restriction: {}",
                            "🔒".dimmed(),
                            allowed.join(", ")
                        );
                    }
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
        self.tool_call_names.clear();
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
                let r = StopReason::MaxSteps;
                self.finish_trace(&r);
                return r;
            }
            if self.tokens_used >= self.config.token_budget {
                let r = StopReason::TokenBudgetExhausted;
                self.finish_trace(&r);
                return r;
            }

            self.step_count += 1;
            info!(
                step = self.step_count,
                tokens = self.tokens_used,
                "agent loop step"
            );

            // Call LLM
            let response = match self.call_llm().await {
                Ok(r) => r,
                Err(e) => {
                    let r = StopReason::Error(format!("LLM call failed: {}", e));
                    self.finish_trace(&r);
                    return r;
                }
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
                let r = StopReason::FinalAnswer(answer);
                self.finish_trace(&r);
                return r;
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
                let result = match self.execute_tool_call(tc).await {
                    Ok(result) => result,
                    Err(stop) => {
                        // Push error as tool result so conversation stays valid
                        let err_msg = match &stop {
                            StopReason::Error(e) => e.clone(),
                            other => format!("{:?}", other),
                        };
                        self.messages.push(LlmMessage {
                            role: "tool".to_string(),
                            content: Some(err_msg),
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });
                        self.finish_trace(&stop);
                        return stop;
                    }
                };
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
    async fn call_llm(&mut self) -> Result<LlmResponse> {
        let tool_defs = self.tools.tool_definitions();
        let request = LlmRequest {
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: if tool_defs.is_empty() {
                None
            } else {
                Some(tool_defs)
            },
            max_tokens: None,
        };
        debug!(model = %self.config.model, messages = self.messages.len(), "calling LLM");
        if let Some(ref mut t) = self.trace {
            t.record(StepKind::LlmRequest {
                messages_count: self.messages.len(),
                model: self.config.model.clone(),
            });
        }
        let resp = self.provider.chat(request).await;
        if let Some(ref mut t) = self.trace {
            if let Ok(ref r) = resp {
                t.record(StepKind::LlmResponse {
                    tokens_used: r.usage.total_tokens,
                    has_tool_calls: !r.tool_calls.is_empty(),
                });
            }
        }
        resp
    }

    /// Finish trace recording and write to disk.
    fn finish_trace(&mut self, result: &StopReason) {
        if let Some(ref mut t) = self.trace {
            let exit_code = match result {
                StopReason::FinalAnswer(_) => 0,
                StopReason::Error(_) => 1,
                StopReason::MaxSteps => 2,
                StopReason::TokenBudgetExhausted => 3,
                StopReason::UserInterrupt => 130,
            };
            if let Err(e) = t.finish(exit_code) {
                eprintln!("  {} trace write failed: {}", "⚠".red(), e);
            }
        }
    }

    /// Execute a single tool call.
    async fn execute_tool_call(&mut self, tc: &LlmToolCall) -> Result<String, StopReason> {
        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        eprintln!(
            "  {} {}",
            "⚙".dimmed(),
            format!(
                "{}({})",
                tc.function.name,
                &tc.function.arguments[..tc.function.arguments.len().min(80)]
            )
            .dimmed()
        );

        // Confirm mode: ask user before executing dangerous tools
        // Trace tool call
        if let Some(ref mut t) = self.trace {
            t.record(StepKind::ToolCall {
                name: tc.function.name.clone(),
                arguments_preview: redact(
                    &tc.function.arguments[..tc.function.arguments.len().min(100)],
                ),
            });
        }

        // Track tool calls and executed commands
        self.tool_call_names.push(tc.function.name.clone());
        if tc.function.name == "execute_cmd" {
            if let Some(cmd) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                .ok()
                .and_then(|a| a.get("cmd").and_then(|c| c.as_str()).map(|s| s.to_string()))
            {
                self.executed_commands.push(cmd);
            }
        }

        // Confirm mode: ask user before executing dangerous tools
        // Skip confirm if the involved domain/command is already in the allowlist
        let already_allowed = self.is_already_allowed(&tc.function.name, &args);
        if self.config.policy.mode == "confirm"
            && Self::is_dangerous_tool(&tc.function.name)
            && !self.auto_approve
            && !already_allowed
        {
            if !self.interactive {
                let msg = format!(
                    "non-interactive mode requires --yes (or a non-confirm policy) before executing {}",
                    tc.function.name
                );
                eprintln!("  {} {}", "✗".red(), msg.dimmed());
                return Err(StopReason::Error(msg));
            }

            // Show what we're about to do
            let involved_domain = Self::extract_domain_from_args(&tc.function.name, &args);
            let involved_cmd = Self::extract_command_from_args(&tc.function.name, &args);

            eprint!(
                "
  {} Execute? [Y/n/A(lways)] ",
                "⚠".yellow().bold()
            );
            std::io::Write::flush(&mut std::io::stderr()).ok();
            match Self::prompt_confirm_with_always() {
                ConfirmResult::Yes => eprintln!("{}", "approved".green()),
                ConfirmResult::Always => {
                    // Permanently allow: persist domain/command to config
                    let mut added: Vec<String> = Vec::new();

                    if let Some(ref domain) = involved_domain {
                        if !self.config.policy.allowed_domains.contains(domain) {
                            self.tools.add_allowed_domain(domain);
                            self.config.policy.allowed_domains.push(domain.clone());
                            crate::config::persist_domain(domain);
                            added.push(format!("domain '{}' → allowed_domains", domain));
                        }
                    }

                    if let Some(ref cmd_name) = involved_cmd {
                        if !self.config.policy.allowed_commands.contains(cmd_name) {
                            self.config.policy.allowed_commands.push(cmd_name.clone());
                            crate::config::persist_command(cmd_name);
                            added.push(format!("command '{}' → allowed_commands", cmd_name));
                        }
                    }

                    if added.is_empty() {
                        eprintln!("{}", "approved (already in allowlist)".green());
                    } else {
                        eprintln!(
                            "{}",
                            "permanently allowed → saved to ~/.rune/rune.toml".green()
                        );
                        for item in &added {
                            eprintln!("    {} {}", "+".green(), item);
                        }
                    }
                }
                ConfirmResult::No => {
                    eprintln!("{}", "denied".red());
                    return Ok("DENIED: user rejected tool execution".to_string());
                }
            }
        }

        let output = self.tools.execute(&tc.function.name, args.clone()).await;

        if output.is_error {
            // Check if it's a domain block we can interactively resolve
            if let Some(domain) = Self::extract_blocked_domain(&output.content) {
                if self.interactive {
                    eprint!(
                        "\n  {} Add '{}' to allowed_domains? [Y/n] ",
                        "🔓".yellow(),
                        domain
                    );
                    std::io::Write::flush(&mut std::io::stderr()).ok();
                    let answer = Self::prompt_yn();
                    if answer {
                        self.tools.add_allowed_domain(&domain);
                        self.config.policy.allowed_domains.push(domain.clone());
                        crate::config::persist_domain(&domain);
                        eprintln!(
                            "{}",
                            format!(
                                "  ✓ '{}' added to allowed_domains (saved to config)",
                                domain
                            )
                            .green()
                        );
                        // Retry the tool call
                        let retry_output = self.tools.execute(&tc.function.name, args).await;
                        if retry_output.is_error {
                            eprintln!(
                                "  {} {}",
                                "✗".red(),
                                retry_output.content[..retry_output.content.len().min(200)]
                                    .dimmed()
                            );
                            if Self::is_policy_blocked(&retry_output.content) {
                                return Err(StopReason::Error(retry_output.content));
                            }
                        } else {
                            eprintln!(
                                "  {} {}",
                                "✓".green(),
                                format!("{}...ok", tc.function.name).dimmed()
                            );
                        }
                        return Ok(retry_output.content);
                    }
                }
                // User said no, or non-interactive
                eprintln!(
                    "  {} {}",
                    "✗".red(),
                    output.content[..output.content.len().min(200)].dimmed()
                );
                return Err(StopReason::Error(output.content));
            }

            eprintln!(
                "  {} {}",
                "✗".red(),
                output.content[..output.content.len().min(200)].dimmed()
            );
            if Self::is_policy_blocked(&output.content) {
                return Err(StopReason::Error(output.content));
            }
        } else {
            eprintln!(
                "  {} {}",
                "✓".green(),
                format!("{}...ok", tc.function.name).dimmed()
            );
        }

        Ok(output.content)
    }

    /// Extract blocked domain from error message, if applicable.
    fn extract_blocked_domain(content: &str) -> Option<String> {
        // Pattern: "BLOCKED: domain 'xxx' is not in allowed_domains"
        if content.contains("is not in allowed_domains") {
            if let Some(start) = content.find("domain '") {
                let after = &content[start + 8..];
                if let Some(end) = after.find('\'') {
                    return Some(after[..end].to_string());
                }
            }
        }
        None
    }

    /// Check if the tool call's domain/command is already in the allowlist.
    fn is_already_allowed(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        match tool_name {
            "fetch_url" => {
                if let Some(domain) = Self::extract_domain_from_args(tool_name, args) {
                    return self.config.policy.allowed_domains.iter().any(|d| {
                        d == &domain || (d.starts_with("*.") && domain.ends_with(&d[1..]))
                    });
                }
                false
            }
            "execute_cmd" => {
                if let Some(cmd_name) = Self::extract_command_from_args(tool_name, args) {
                    return self
                        .config
                        .policy
                        .allowed_commands
                        .iter()
                        .any(|c| c == &cmd_name || c == "*");
                }
                false
            }
            _ => false,
        }
    }

    /// Extract domain from tool call arguments (for fetch_url).
    fn extract_domain_from_args(tool_name: &str, args: &serde_json::Value) -> Option<String> {
        if tool_name == "fetch_url" {
            if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                let without_scheme = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))
                    .unwrap_or(url);
                let host = without_scheme.split('/').next()?;
                let domain = host.split(':').next()?;
                if !domain.is_empty() {
                    return Some(domain.to_string());
                }
            }
        }
        None
    }

    /// Extract the base command name from execute_cmd arguments.
    fn extract_command_from_args(tool_name: &str, args: &serde_json::Value) -> Option<String> {
        if tool_name == "execute_cmd" {
            if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
                let first_token = cmd.split_whitespace().next().unwrap_or("");
                let binary = first_token.rsplit('/').next().unwrap_or(first_token);
                if !binary.is_empty() {
                    return Some(binary.to_string());
                }
            }
        }
        None
    }

    /// Simple Y/n prompt via /dev/tty.
    fn prompt_yn() -> bool {
        use std::io::{BufRead, Write};
        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            let mut reader = std::io::BufReader::new(tty);
            std::io::stderr().flush().ok();
            let mut input = String::new();
            if reader.read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                if trimmed == "n" || trimmed == "no" {
                    return false;
                }
                return true; // empty or y/yes
            }
        }
        true
    }

    fn is_policy_blocked(content: &str) -> bool {
        let s = content.trim_start();
        s.starts_with("BLOCKED:")
            || s.starts_with("BLOCKED by policy:")
            || s.contains("Network access requires explicit allowlist configuration")
            || s.contains("command '") && s.contains("is not in allowed_commands")
    }

    /// Tools that modify state or execute arbitrary commands.
    fn is_dangerous_tool(name: &str) -> bool {
        matches!(
            name,
            "execute_cmd" | "write_file" | "fetch_url" | "read_file"
        )
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

    /// Get all tool call names executed during this session.
    pub fn tool_call_names(&self) -> &[String] {
        &self.tool_call_names
    }

    /// Count all tool calls executed during this session.
    pub fn tool_call_count(&self) -> usize {
        self.tool_call_names.len()
    }
}
