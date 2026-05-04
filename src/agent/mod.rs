use crate::config::RuneConfig;
use crate::provider::{LlmMessage, LlmRequest, LlmResponse, LlmToolCall, ProviderRegistry};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;
use crate::trace::{redact, StepKind, TraceWriter};
use anyhow::Result;
use colored::Colorize;
use std::io::Write;
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

/// Rough token estimator using a chars/4 approximation.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        (chars + 3) / 4
    }
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
    /// Skill-scoped tool allow list (None = all tools available)
    skill_tools_allow: Option<Vec<String>>,
    /// Skill-scoped tool deny list (None = nothing denied)
    skill_tools_deny: Option<Vec<String>>,
}

impl Agent {
    pub fn new(config: RuneConfig, provider: ProviderRegistry, interactive: bool) -> Self {
        let mut config = config;
        // Auto-add CWD to allowed_paths_ro so read_file in project dir does not require confirm
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_str = cwd.to_string_lossy().to_string();
            if !config
                .policy
                .allowed_paths_ro
                .iter()
                .any(|p| cwd_str.starts_with(p.trim_end_matches("/")))
                && !config
                    .policy
                    .allowed_paths_rw
                    .iter()
                    .any(|p| cwd_str.starts_with(p.trim_end_matches("/")))
            {
                config.policy.allowed_paths_ro.push(cwd_str);
            }
        }
        let allowed_dirs = if config.policy.allowed_paths_rw.is_empty() {
            vec![PathBuf::from("/tmp"), PathBuf::from(".")]
        } else {
            config
                .policy
                .allowed_paths_rw
                .iter()
                .map(PathBuf::from)
                .collect()
        };
        let mut tools = ToolRegistry::new(allowed_dirs);
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
            skill_tools_allow: None,
            skill_tools_deny: None,
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
                    content_parts: None,
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

    pub fn is_interactive(&self) -> bool {
        self.interactive
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

    /// Estimate total context tokens across all messages.
    pub fn total_context_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| estimate_tokens(&self.message_text(m)) + 4)
            .sum()
    }

    fn message_text(&self, m: &LlmMessage) -> String {
        let mut parts = Vec::new();
        parts.push(format!("role: {}", m.role));
        if let Some(content) = m.content.as_ref() {
            parts.push(format!("content: {}", content));
        }
        if let Some(tool_calls) = m.tool_calls.as_ref() {
            for tc in tool_calls {
                parts.push(format!(
                    "tool_call: {} {}",
                    tc.function.name, tc.function.arguments
                ));
            }
        }
        if let Some(tool_call_id) = m.tool_call_id.as_ref() {
            parts.push(format!("tool_call_id: {}", tool_call_id));
        }
        parts.join(
            "
",
        )
    }

    fn message_preview(&self, m: &LlmMessage) -> String {
        let mut text = if let Some(content) = m.content.as_ref() {
            content.clone()
        } else if let Some(tool_calls) = m.tool_calls.as_ref() {
            tool_calls
                .iter()
                .map(|tc| format!("{} {}", tc.function.name, tc.function.arguments))
                .collect::<Vec<_>>()
                .join("; ")
        } else if let Some(tool_call_id) = m.tool_call_id.as_ref() {
            tool_call_id.clone()
        } else {
            String::new()
        };
        text.truncate(text.chars().take(200).map(char::len_utf8).sum());
        text
    }

    /// Compact context: keep system prompt + summarize older messages.
    pub fn compact(&mut self) {
        let keep_last = self.config.compact_keep_last.max(1);
        if self.messages.len() <= keep_last + 1 {
            return;
        }

        let system = self.messages.first().cloned();
        let total = self.messages.len();
        let summarize_end = total.saturating_sub(keep_last);
        if summarize_end <= 1 {
            return;
        }

        let mut summary_parts: Vec<String> = Vec::new();
        for m in self.messages.iter().skip(1).take(summarize_end - 1) {
            let preview = self.message_preview(m);
            summary_parts.push(format!("[{}]: {}", m.role, preview));
        }

        let summary = format!(
            "[Context compacted: {} messages summarized]
{}",
            summarize_end - 1,
            summary_parts.join(
                "
"
            )
        );

        let mut new_messages = Vec::new();
        if let Some(sys) = system {
            new_messages.push(sys);
        }
        new_messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(summary),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
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
                    // Skill safety: restrict tools if skill defines tools_allow/tools_deny
                    if let Some(ref allowed) = skill.metadata.tools_allow {
                        self.skill_tools_allow = Some(allowed.clone());
                        eprintln!("    {} tools_allow: {}", "🔒".dimmed(), allowed.join(", "));
                    }
                    if let Some(ref denied) = skill.metadata.tools_deny {
                        self.skill_tools_deny = Some(denied.clone());
                        eprintln!("    {} tools_deny: {}", "🔒".dimmed(), denied.join(", "));
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
                        content_parts: None,
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
            content_parts: None,
        });

        loop {
            if let Some(max) = self.config.max_steps {
                if self.step_count >= max {
                    let r = StopReason::MaxSteps;
                    self.finish_trace(&r);
                    return r;
                }
            }
            if let Some(budget) = self.config.token_budget {
                if self.tokens_used >= budget {
                    let r = StopReason::TokenBudgetExhausted;
                    self.finish_trace(&r);
                    return r;
                }
            }

            let context_tokens = self.total_context_tokens();
            let context_limit =
                ((self.config.context_window as f64) * self.config.compact_threshold) as usize;
            if context_tokens > context_limit {
                warn!(
                    context_tokens,
                    context_limit,
                    context_window = self.config.context_window,
                    compact_threshold = self.config.compact_threshold,
                    "context window threshold exceeded; compacting"
                );
                self.compact();
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
                    content_parts: None,
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
                content_parts: None,
            });

            // Execute tool calls — parallel when multiple and non-interactive
            if response.tool_calls.len() > 1 && (self.auto_approve || !self.interactive) {
                // Pre-process: tracking, trace, policy checks (sequential, needs &mut self)
                let mut dispatch_list: Vec<(String, String, serde_json::Value)> = Vec::new();
                let early_stop: Option<StopReason> = None;

                for tc in &response.tool_calls {
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

                    if let Some(ref mut t) = self.trace {
                        t.record(StepKind::ToolCall {
                            name: tc.function.name.clone(),
                            arguments_preview: redact(
                                &tc.function.arguments[..tc.function.arguments.len().min(100)],
                            ),
                        });
                    }

                    self.tool_call_names.push(tc.function.name.clone());
                    if tc.function.name == "execute_cmd" {
                        if let Some(cmd) =
                            serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                                .ok()
                                .and_then(|a| {
                                    a.get("cmd").and_then(|c| c.as_str()).map(|s| s.to_string())
                                })
                        {
                            self.executed_commands.push(cmd);
                        }
                    }

                    dispatch_list.push((tc.id.clone(), tc.function.name.clone(), args));
                }

                if let Some(stop) = early_stop {
                    self.finish_trace(&stop);
                    return stop;
                }

                // Parallel dispatch via ToolRegistry (which only needs &self)
                let futs: Vec<_> = dispatch_list
                    .iter()
                    .map(|(_id, name, args)| self.tools.execute(name, args.clone()))
                    .collect();
                let results = futures::future::join_all(futs).await;

                // Push results in order
                for (i, output) in results.into_iter().enumerate() {
                    let tc_id = &dispatch_list[i].0;
                    let tc_name = &dispatch_list[i].1;
                    let content_preview = redact(&output.content[..output.content.len().min(200)]);

                    if output.is_error {
                        eprintln!("  {} {}", "✗".red(), output.content.dimmed());
                    } else {
                        eprintln!("  {} {}", "✓".green(), format!("{}...ok", tc_name).dimmed());
                    }

                    if let Some(ref mut t) = self.trace {
                        t.record(StepKind::ToolResult {
                            name: tc_name.clone(),
                            is_error: output.is_error,
                            content_preview,
                        });
                    }

                    let is_err = output.is_error;
                    self.messages.push(LlmMessage {
                        role: "tool".to_string(),
                        content: Some(output.content),
                        tool_calls: None,
                        tool_call_id: Some(tc_id.clone()),
                        content_parts: None,
                    });

                    // In non-interactive mode, tool errors are fatal
                    if is_err && !self.interactive {
                        let stop = StopReason::Error(
                            self.messages
                                .last()
                                .unwrap()
                                .content
                                .clone()
                                .unwrap_or_default(),
                        );
                        self.finish_trace(&stop);
                        return stop;
                    }
                }
            } else {
                // Sequential execution (single tool call or interactive confirm mode)
                for tc in &response.tool_calls {
                    let result = match self.execute_tool_call(tc).await {
                        Ok(result) => result,
                        Err(stop) => {
                            let err_msg = match &stop {
                                StopReason::Error(e) => e.clone(),
                                other => format!("{:?}", other),
                            };
                            self.messages.push(LlmMessage {
                                role: "tool".to_string(),
                                content: Some(err_msg),
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                                content_parts: None,
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
                        content_parts: None,
                    });
                }
            }
        }
    }

    fn build_llm_request(&self) -> LlmRequest {
        let mut tool_defs = self.tools.tool_definitions();
        if let Some(ref allowed) = self.skill_tools_allow {
            tool_defs.retain(|def| {
                def.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|name| allowed.iter().any(|a| a == name))
                    .unwrap_or(false)
            });
        }
        if let Some(ref denied) = self.skill_tools_deny {
            tool_defs.retain(|def| {
                def.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|name| !denied.iter().any(|d| d == name))
                    .unwrap_or(true)
            });
        }

        LlmRequest {
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: if tool_defs.is_empty() {
                None
            } else {
                Some(tool_defs)
            },
            max_tokens: None,
        }
    }

    fn record_llm_request_trace(&mut self) {
        if let Some(ref mut t) = self.trace {
            t.record(StepKind::LlmRequest {
                messages_count: self.messages.len(),
                model: self.config.model.clone(),
            });
        }
    }

    async fn call_llm_non_streaming(&mut self) -> Result<LlmResponse> {
        let request = self.build_llm_request();
        debug!(model = %self.config.model, messages = self.messages.len(), "calling LLM");
        self.record_llm_request_trace();
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

    async fn call_llm_streaming(&mut self) -> Result<LlmResponse> {
        let request = self.build_llm_request();
        debug!(model = %self.config.model, messages = self.messages.len(), "calling LLM (streaming)");
        self.record_llm_request_trace();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
        let printer = tokio::spawn(async move {
            let mut stderr = std::io::stderr();
            while let Some(token) = rx.recv().await {
                let _ = write!(stderr, "{}", token);
                let _ = stderr.flush();
            }
        });

        let resp = self.provider.chat_streaming(request, tx).await;
        let _ = printer.await;

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

    /// Call the LLM provider.
    async fn call_llm(&mut self) -> Result<LlmResponse> {
        if self.interactive {
            self.call_llm_streaming().await
        } else {
            self.call_llm_non_streaming().await
        }
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
        // Unrestricted mode skips all confirmation
        let already_allowed = self.is_already_allowed(&tc.function.name, &args);
        if self.config.policy.mode != "unrestricted"
            && self.config.policy.mode == "confirm"
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
            let involved_path = Self::extract_path_from_args(&tc.function.name, &args);

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

                    // Persist ALL binaries in the pipeline, not just the first one
                    let all_binaries = Self::extract_all_command_binaries(&args);
                    for bin in &all_binaries {
                        if !self.config.policy.allowed_commands.contains(bin) {
                            self.tools.add_allowed_command(bin);
                            self.config.policy.allowed_commands.push(bin.clone());
                            crate::config::persist_command(bin);
                            added.push(format!("command '{}' → allowed_commands", bin));
                        }
                    }

                    if let Some(ref path) = involved_path {
                        // Persist the parent directory for path-based tools
                        let dir = std::path::Path::new(path.as_str())
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        let dir = if dir.is_empty() { ".".to_string() } else { dir };
                        let resolved = self.resolve_tool_path(&dir);
                        if tc.function.name == "write_file" {
                            if !self
                                .is_path_in_list(&resolved, &self.config.policy.allowed_paths_rw)
                            {
                                self.tools.add_allowed_path_rw(&resolved);
                                self.config.policy.allowed_paths_rw.push(resolved.clone());
                                crate::config::persist_path_rw(&resolved);
                                added.push(format!("path '{}' → allowed_paths_rw", resolved));
                            }
                        } else {
                            // read_file
                            if !self
                                .is_path_in_list(&resolved, &self.config.policy.allowed_paths_ro)
                                && !self.is_path_in_list(
                                    &resolved,
                                    &self.config.policy.allowed_paths_rw,
                                )
                            {
                                self.tools.add_allowed_path_ro(&resolved);
                                self.config.policy.allowed_paths_ro.push(resolved.clone());
                                crate::config::persist_path_ro(&resolved);
                                added.push(format!("path '{}' → allowed_paths_ro", resolved));
                            }
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

        // Enforce skill-scoped tool restrictions
        if let Some(ref allowed) = self.skill_tools_allow {
            if !allowed.iter().any(|t| t == &tc.function.name) {
                let msg = format!(
                    "BLOCKED by skill policy: tool '{}' is not in tools_allow",
                    tc.function.name
                );
                eprintln!("  {} {}", "✗".red(), msg.dimmed());
                return Ok(msg);
            }
        }
        if let Some(ref denied) = self.skill_tools_deny {
            if denied.iter().any(|t| t == &tc.function.name) {
                let msg = format!(
                    "BLOCKED by skill policy: tool '{}' is in tools_deny",
                    tc.function.name
                );
                eprintln!("  {} {}", "✗".red(), msg.dimmed());
                return Ok(msg);
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
            // Non-interactive: any tool error is a hard stop (sandbox enforcement)
            if !self.interactive {
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
            "read_file" => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    let p = self.resolve_tool_path(path);
                    return self.is_path_in_list(&p, &self.config.policy.allowed_paths_ro)
                        || self.is_path_in_list(&p, &self.config.policy.allowed_paths_rw);
                }
                false
            }
            "write_file" => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    let p = self.resolve_tool_path(path);
                    return self.is_path_in_list(&p, &self.config.policy.allowed_paths_rw);
                }
                false
            }
            "fetch_url" => {
                if let Some(domain) = Self::extract_domain_from_args(tool_name, args) {
                    return self.config.policy.allowed_domains.iter().any(|d| {
                        d == &domain || (d.starts_with("*.") && domain.ends_with(&d[1..]))
                    });
                }
                false
            }
            "execute_cmd" => {
                let binaries = Self::extract_all_command_binaries(args);
                if binaries.is_empty() {
                    return false;
                }
                // All binaries in the pipeline must be allowed
                binaries.iter().all(|bin| {
                    self.config
                        .policy
                        .allowed_commands
                        .iter()
                        .any(|c| c == bin || c == "*")
                })
            }
            _ => false,
        }
    }

    /// Resolve a potentially relative path to absolute for policy matching.
    fn resolve_tool_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!(
                "{}/{}",
                std::env::current_dir().unwrap_or_default().display(),
                path
            )
        }
    }

    /// Check if a path falls under any entry in the given list.
    fn is_path_in_list(&self, path: &str, list: &[String]) -> bool {
        list.iter().any(|allowed| {
            path == allowed || path.starts_with(&format!("{}/", allowed.trim_end_matches('/')))
        })
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

    /// Extract all command binaries from execute_cmd arguments (pipeline-aware).
    fn extract_all_command_binaries(args: &serde_json::Value) -> Vec<String> {
        if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
            return crate::tools::extract_command_binaries_pub(cmd);
        }
        Vec::new()
    }

    /// Extract path from read_file/write_file arguments.
    fn extract_path_from_args(tool_name: &str, args: &serde_json::Value) -> Option<String> {
        if tool_name == "read_file" || tool_name == "write_file" {
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                if !path.is_empty() {
                    return Some(path.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_short() {
        // "hello" = 5 chars → (5+3)/4 = 2 tokens
        assert_eq!(estimate_tokens("hello"), 2);
    }

    #[test]
    fn test_estimate_tokens_longer() {
        // 100 chars → 25 tokens
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens(&text), 25);
    }

    #[test]
    fn test_estimate_tokens_unicode() {
        // 4 unicode chars → (4+3)/4 = 1
        assert_eq!(estimate_tokens("你好世界"), 1);
    }
}
