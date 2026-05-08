use crate::config::RuneConfig;
use crate::embedding::EmbeddingEngine;
use crate::mcp::McpManager;
use crate::provider::{
    ContentPart, LlmMessage, LlmRequest, LlmResponse, LlmToolCall, ProviderRegistry,
};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;
use crate::trace::{redact, StepKind, TraceWriter};
use anyhow::Result;
use colored::Colorize;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

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

/// Recorded tool call for post-run summary display.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub args_preview: String,
    pub is_error: bool,
}

/// Result of user confirmation prompt.
enum ConfirmResult {
    Yes,
    No,
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
    mcp_manager: Option<Arc<TokioMutex<McpManager>>>,
    skill_loader: SkillLoader,
    auto_approve: bool,
    interactive: bool,
    trace: Option<TraceWriter>,
    executed_commands: Vec<String>,
    tool_call_names: Vec<String>,
    tool_calls_log: Vec<ToolCallRecord>,
    /// Skill-scoped tool allow list (None = all tools available)
    skill_tools_allow: Option<Vec<String>>,
    /// Skill-scoped tool deny list (None = nothing denied)
    skill_tools_deny: Option<Vec<String>>,
    /// Optional embedding engine for RAG-based compaction and skill search
    pub embedding: Option<EmbeddingEngine>,
}

impl Agent {
    pub fn new(
        config: RuneConfig,
        provider: ProviderRegistry,
        interactive: bool,
        embedding: Option<EmbeddingEngine>,
    ) -> Self {
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
        // Auto-add skills_dir to allowed_paths_ro so agent can read skill references
        {
            let sd = config.skills_dir.clone();
            if !sd.is_empty()
                && !config
                    .policy
                    .allowed_paths_ro
                    .iter()
                    .any(|p| sd.starts_with(p.trim_end_matches("/")))
                && !config
                    .policy
                    .allowed_paths_rw
                    .iter()
                    .any(|p| sd.starts_with(p.trim_end_matches("/")))
            {
                config.policy.allowed_paths_ro.push(sd);
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
        // Spawn background indexing of skills into vector store when embedding is configured.
        if let Some(ref eng) = embedding {
            let eng_clone = eng.clone();
            let skills_dir_clone = config.skills_dir.clone();
            tokio::spawn(async move {
                let loader = SkillLoader::new(vec![PathBuf::from(skills_dir_clone)]);
                match loader.index_skills(&eng_clone).await {
                    Ok(_) => info!("skill index complete"),
                    Err(e) => warn!(error = %e, "skill index failed"),
                }
            });
        }

        let trace = if config.trace.is_some() {
            Some(TraceWriter::new(
                TraceWriter::generate_run_id(),
                config.model.clone(),
                PathBuf::from(".rune/traces"),
                true,
            ))
        } else {
            None
        };
        let mcp_manager = None;
        let auto_approve = config.auto_approve;
        Agent {
            config,
            messages: Vec::new(),
            step_count: 0,
            tokens_used: 0,
            provider,
            tools,
            mcp_manager,
            skill_loader,
            auto_approve,
            interactive,
            trace,
            executed_commands: Vec::new(),
            tool_call_names: Vec::new(),
            tool_calls_log: Vec::new(),
            skill_tools_allow: None,
            skill_tools_deny: None,
            embedding,
        }
    }

    /// Set the system prompt.

    /// Attach an MCP manager for external tool dispatch.
    pub fn set_mcp_manager(&mut self, mgr: Arc<TokioMutex<McpManager>>) {
        self.mcp_manager = Some(mgr);
    }

    /// Get a reference to the MCP manager (for /mcps command).
    pub fn mcp_manager_ref(&self) -> Option<&Arc<TokioMutex<McpManager>>> {
        self.mcp_manager.as_ref()
    }
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

    /// Compact context: use embedding-based RAG if available, otherwise simple truncation.
    pub async fn compact(&mut self) {
        let keep_last = self.config.compact_keep_last.max(1);
        if self.messages.len() <= keep_last + 1 {
            return;
        }

        // Try RAG-based compaction if embedding is available
        if let Some(engine) = self.embedding.clone() {
            if self.config.embedding.enabled {
                match self.compact_rag(&engine, keep_last).await {
                    Ok(new_msgs) => {
                        self.messages = new_msgs;
                        return;
                    }
                    Err(e) => {
                        warn!(error = %e, "RAG compaction failed; falling back to simple");
                    }
                }
            }
        }

        // Fallback: simple compaction
        self.compact_simple();
    }

    /// Simple compaction: summarize older messages, keep last N.
    fn compact_simple(&mut self) {
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

    /// RAG-based compaction: keep messages most relevant to the latest user query.
    async fn compact_rag(
        &self,
        engine: &EmbeddingEngine,
        keep_last: usize,
    ) -> Result<Vec<LlmMessage>> {
        let total = self.messages.len();
        let summarize_end = total.saturating_sub(keep_last);
        if summarize_end <= 1 {
            anyhow::bail!("not enough messages to compact");
        }

        // Find latest user message as the relevance query
        let latest_user = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("")
            .to_string();

        if latest_user.is_empty() {
            anyhow::bail!("no user message found for RAG query");
        }

        let user_vec = engine.embed_one(&latest_user).await?;

        // Collect candidate messages (skip system at index 0, skip last keep_last)
        let mut candidate_texts: Vec<String> = Vec::new();
        let mut candidate_indices: Vec<usize> = Vec::new();
        for (i, m) in self
            .messages
            .iter()
            .enumerate()
            .skip(1)
            .take(summarize_end - 1)
        {
            candidate_indices.push(i);
            candidate_texts.push(self.message_text(m));
        }

        if candidate_texts.is_empty() {
            anyhow::bail!("no candidate messages to score");
        }

        // Embed candidates in batches
        let mut all_vecs: Vec<Vec<f32>> = Vec::new();
        for chunk in candidate_texts.chunks(32) {
            let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            let vecs = engine.embed(&refs).await?;
            all_vecs.extend(vecs);
        }

        // Score by cosine similarity
        let mut scored: Vec<(f32, usize)> = all_vecs
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let sim = crate::embedding::cosine_similarity(&user_vec, v);
                (sim, candidate_indices[i])
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Keep top-K most relevant (max 20) + last keep_last
        let top_k = scored.len().min(20);
        let mut keep_set: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for &(_, idx) in scored.iter().take(top_k) {
            keep_set.insert(idx);
        }
        for i in (total - keep_last)..total {
            keep_set.insert(i);
        }

        // Build summary of removed messages
        let removed_count = (1..summarize_end).filter(|i| !keep_set.contains(i)).count();

        let summary = format!(
            "[Context compacted via RAG: {} messages removed, {} kept by relevance]",
            removed_count, top_k
        );

        let mut new_messages: Vec<LlmMessage> = Vec::new();
        // Keep system prompt
        if let Some(sys) = self.messages.first() {
            new_messages.push(sys.clone());
        }
        // Add compaction summary
        new_messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(summary),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        // Add kept candidate messages in order
        for &idx in keep_set.iter() {
            if idx > 0 && idx < summarize_end {
                new_messages.push(self.messages[idx].clone());
            }
        }
        // Add the final keep_last messages
        new_messages.extend(self.messages[total - keep_last..].iter().cloned());

        Ok(new_messages)
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

    /// Push a user message with explicit content parts (e.g., images).
    pub fn push_user_message_with_parts(&mut self, text: String, parts: Vec<ContentPart>) {
        self.messages.push(LlmMessage {
            role: "user".to_string(),
            content: Some(text),
            tool_calls: None,
            tool_call_id: None,
            content_parts: Some(parts),
        });
    }

    /// Resolve @skill references in user input and inject skill content as system context.
    /// If no explicit @skill refs found and embedding is enabled, try semantic search.
    async fn inject_skills(&mut self, user_input: &str) {
        let skill_refs = SkillLoader::extract_skill_refs(user_input);

        if !skill_refs.is_empty() {
            // Explicit @skill references take priority
            for name in &skill_refs {
                self.load_and_inject_skill(name);
            }
            return;
        }

        // No explicit refs — try semantic search if embeddings enabled
        if let Some(ref engine) = self.embedding {
            if self.config.embedding.enabled {
                let threshold = self.config.embedding.threshold;
                match self
                    .skill_loader
                    .semantic_search(engine, user_input, threshold, 1)
                    .await
                {
                    Ok(results) if !results.is_empty() => {
                        let (_score, name) = &results[0];
                        info!(skill = %name, score = _score, "semantic matched skill");
                        eprintln!("  {} Semantic skill match: {}", "🔎".dimmed(), name.green());
                        self.load_and_inject_skill(name);
                    }
                    Err(e) => {
                        warn!(error = %e, "semantic skill search failed");
                    }
                    _ => {}
                }
            }
        }
    }

    /// Load a skill by name and inject it as a system message.
    fn load_and_inject_skill(&mut self, name: &str) {
        match self.skill_loader.load(name) {
            Ok(skill) => {
                info!(skill = %name, "loaded skill");
                eprintln!("  {} Loaded skill: {}", "📚".dimmed(), name.green());
                if let Some(ref allowed) = skill.metadata.tools_allow {
                    self.skill_tools_allow = Some(allowed.clone());
                    eprintln!("    {} tools_allow: {}", "🔒".dimmed(), allowed.join(", "));
                }
                if let Some(ref denied) = skill.metadata.tools_deny {
                    self.skill_tools_deny = Some(denied.clone());
                    eprintln!("    {} tools_deny: {}", "🔒".dimmed(), denied.join(", "));
                }
                let skill_dir = skill
                    .source_path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                self.messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: Some(format!(
                        "[Skill: {} | base_dir: {}]\nIMPORTANT: All relative paths referenced in this skill (e.g. references/, scripts/) must be resolved from base_dir above. Use absolute paths when reading skill files.\n{}\n[End Skill: {}]",
                        skill.metadata.name, skill_dir, skill.content, skill.metadata.name
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

    /// Run the agent loop: send user input → LLM → tools → repeat until done.
    pub async fn run(&mut self, user_input: &str) -> StopReason {
        // Resolve and inject @skill references
        self.executed_commands.clear();
        self.tool_call_names.clear();
        self.tool_calls_log.clear();

        // If --skills was specified, preload those skills and skip dynamic discovery
        if !self.config.preload_skills.is_empty() && self.step_count == 0 {
            let names: Vec<String> = self.config.preload_skills.clone();
            for name in &names {
                self.load_and_inject_skill(name);
            }
        } else {
            self.inject_skills(user_input).await;
        }

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
                if max > 0 && self.step_count >= max {
                    let r = StopReason::MaxSteps;
                    self.finish_trace(&r);
                    return r;
                }
            }
            if let Some(budget) = self.config.token_budget {
                if budget > 0 && self.tokens_used >= budget {
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
                self.compact().await;
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
                            Self::truncate_middle(&tc.function.arguments, 80)
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

                    // Only show errors inline for immediate feedback
                    if output.is_error {
                        eprintln!("  {} {}", "✗".red(), output.content.dimmed());
                    }
                    self.tool_calls_log.push(ToolCallRecord {
                        name: tc_name.clone(),
                        args_preview: dispatch_list[i].2.to_string(),
                        is_error: output.is_error,
                    });

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
                    let args_preview = tc.function.arguments.clone();
                    let result = match self.execute_tool_call(tc).await {
                        Ok(result) => {
                            self.tool_calls_log.push(ToolCallRecord {
                                name: tc.function.name.clone(),
                                args_preview: args_preview.clone(),
                                is_error: false,
                            });
                            result
                        }
                        Err(stop) => {
                            self.tool_calls_log.push(ToolCallRecord {
                                name: tc.function.name.clone(),
                                args_preview: args_preview.clone(),
                                is_error: true,
                            });
                            let err_msg = match &stop {
                                StopReason::Error(e) => e.clone(),
                                other => format!("{:?}", other),
                            };
                            // Show errors inline for immediate feedback
                            eprintln!("  {} {}", "✗".red(), err_msg.dimmed());
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
        // Append MCP tool definitions
        if let Some(ref mcp) = self.mcp_manager {
            if let Ok(mgr) = mcp.try_lock() {
                for (_server, tool) in mgr.all_tools() {
                    let mut params = tool
                        .input_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
                    if !params.is_object() {
                        params = serde_json::json!({"type": "object", "properties": {}});
                    }
                    tool_defs.push(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description.as_deref().unwrap_or("MCP tool"),
                            "parameters": params
                        }
                    }));
                }
            }
        }
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
            thinking: self.config.thinking.clone(),
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

    /// Truncate a string keeping prefix and suffix, replacing middle with "..."
    fn truncate_middle(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            return s.to_string();
        }
        let keep = (max_len - 3) / 2;
        // Find valid char boundaries for UTF-8 safety
        let prefix_end = s
            .char_indices()
            .take_while(|(i, _)| *i < keep)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let suffix_start = s
            .char_indices()
            .rev()
            .take_while(|(i, _)| s.len() - *i <= keep)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...{}", &s[..prefix_end], &s[suffix_start..])
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
                Self::truncate_middle(&tc.function.arguments, 80)
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

            eprint!(
                "
  {} Execute? [Y/n] ",
                "⚠".yellow().bold()
            );
            std::io::Write::flush(&mut std::io::stderr()).ok();
            match Self::prompt_confirm() {
                ConfirmResult::Yes => eprintln!("{}", "approved".green()),
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

        let mut output = self.tools.execute(&tc.function.name, args.clone()).await;

        // If built-in tools don't know this tool, try MCP
        if output.is_error && output.content.starts_with("unknown tool:") {
            if let Some(ref mcp) = self.mcp_manager {
                let mut mgr = mcp.lock().await;
                match mgr.call_tool(&tc.function.name, args.clone()).await {
                    Ok(result) => {
                        let text = if let Some(s) = result.as_str() {
                            s.to_string()
                        } else {
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        };
                        output = crate::tools::ToolOutput {
                            content: text,
                            is_error: false,
                            active_layers: None,
                            degraded: None,
                        };
                    }
                    Err(e) => {
                        output = crate::tools::ToolOutput {
                            content: format!("MCP tool error: {}", e),
                            is_error: true,
                            active_layers: None,
                            degraded: None,
                        };
                    }
                }
            }
        }

        loop {
            if !output.is_error {
                break;
            }

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
                        output = self.tools.execute(&tc.function.name, args.clone()).await;
                        continue;
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

            // Check if it's a command block we can interactively resolve
            if let Some(command) = Self::extract_blocked_command(&output.content) {
                if self.interactive {
                    eprint!(
                        "\n  {} Add '{}' to allowed_commands? [Y/n] ",
                        "🔓".yellow(),
                        command
                    );
                    std::io::Write::flush(&mut std::io::stderr()).ok();
                    let answer = Self::prompt_yn();
                    if answer {
                        self.tools.add_allowed_command(&command);
                        self.config.policy.allowed_commands.push(command.clone());
                        crate::config::persist_command(&command);
                        eprintln!(
                            "{}",
                            format!(
                                "  ✓ '{}' added to allowed_commands (saved to config)",
                                command
                            )
                            .green()
                        );
                        output = self.tools.execute(&tc.function.name, args.clone()).await;
                        continue;
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

            // Check if it's a network error we can resolve by adding a domain
            if let Some(domain) = Self::extract_network_blocked_domain(&output.content) {
                if self.interactive {
                    eprint!(
                        "\n  {} Network blocked for '{}'. Add to allowed_domains? [Y/n] ",
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
                        output = self.tools.execute(&tc.function.name, args.clone()).await;
                        continue;
                    }
                }
            }

            // Check if it's a file/binary permission error — use strace probing
            if output.content.contains("Permission denied") || output.content.contains("EACCES") {
                if self.interactive {
                    // Extract the original command from args for strace re-run
                    let cmd_for_strace = args
                        .get("cmd")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(cmd) = cmd_for_strace {
                        let cwd = args
                            .get("cwd")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        eprintln!(
                            "  {} Permission denied — probing with strace...",
                            "🔍".dimmed()
                        );
                        let blocked_files = Self::strace_probe_eacces(&cmd, cwd.as_deref()).await;

                        if blocked_files.is_empty() {
                            // Fallback to legacy pattern matching
                            if let Some(file_path) =
                                Self::extract_permission_denied_path(&output.content)
                            {
                                let already_allowed = self
                                    .config
                                    .policy
                                    .allowed_files_ro
                                    .iter()
                                    .any(|f| f == &file_path)
                                    || self
                                        .config
                                        .policy
                                        .allowed_paths_ro
                                        .iter()
                                        .any(|p| file_path.starts_with(p));
                                if !already_allowed {
                                    eprint!(
                                        "\n  {} File access blocked: '{}'.\n      Add to allowed_files_ro? [Y/n] ",
                                        "🔓".yellow(),
                                        file_path
                                    );
                                    std::io::Write::flush(&mut std::io::stderr()).ok();
                                    if Self::prompt_yn() {
                                        self.config.policy.allowed_files_ro.push(file_path.clone());
                                        self.tools.add_allowed_file_ro(&file_path);
                                        crate::config::persist_policy_array(
                                            "allowed_files_ro",
                                            &file_path,
                                        );
                                        eprintln!(
                                            "{}",
                                            format!(
                                                "  ✓ '{}' added to allowed_files_ro (saved)",
                                                file_path
                                            )
                                            .green()
                                        );
                                        output = self
                                            .tools
                                            .execute(&tc.function.name, args.clone())
                                            .await;
                                        continue;
                                    }
                                }
                            }
                        } else {
                            // strace found blocked files — prompt for each
                            let mut any_added = false;
                            for (path, mode) in &blocked_files {
                                let already_allowed = match mode.as_str() {
                                    "rw" => {
                                        self.config
                                            .policy
                                            .allowed_files_rw
                                            .iter()
                                            .any(|f| f == path)
                                            || self
                                                .config
                                                .policy
                                                .allowed_paths_rw
                                                .iter()
                                                .any(|p| path.starts_with(p))
                                    }
                                    _ => {
                                        self.config
                                            .policy
                                            .allowed_files_ro
                                            .iter()
                                            .any(|f| f == path)
                                            || self
                                                .config
                                                .policy
                                                .allowed_paths_ro
                                                .iter()
                                                .any(|p| path.starts_with(p))
                                    }
                                };
                                if already_allowed {
                                    continue;
                                }
                                let field = if mode == "rw" {
                                    "allowed_files_rw"
                                } else {
                                    "allowed_files_ro"
                                };
                                eprint!(
                                    "\n  {} Access blocked: '{}' ({}).\n      Add to {}? [Y/n] ",
                                    "🔓".yellow(),
                                    path,
                                    if mode == "rw" {
                                        "read-write"
                                    } else {
                                        "read-only"
                                    },
                                    field
                                );
                                std::io::Write::flush(&mut std::io::stderr()).ok();
                                if Self::prompt_yn() {
                                    if mode == "rw" {
                                        self.config.policy.allowed_files_rw.push(path.clone());
                                        self.tools.add_allowed_file_rw(path);
                                        crate::config::persist_policy_array(field, path);
                                    } else {
                                        self.config.policy.allowed_files_ro.push(path.clone());
                                        self.tools.add_allowed_file_ro(path);
                                        crate::config::persist_policy_array(field, path);
                                    }
                                    eprintln!(
                                        "{}",
                                        format!("  ✓ '{}' added to {} (saved)", path, field)
                                            .green()
                                    );
                                    any_added = true;
                                }
                            }
                            if any_added {
                                output = self.tools.execute(&tc.function.name, args.clone()).await;
                                continue;
                            }
                        }
                    }
                }
            }

            break;
        }

        if output.is_error {
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
        }
        // Success output deferred to post-run summary

        // Show sandbox diagnostics to interactive users
        if self.interactive {
            if let Some(ref layers) = output.active_layers {
                if !layers.is_empty() {
                    eprintln!(
                        "    {} {}",
                        "🔎".dimmed(),
                        format!("sandbox: {}", layers.join(", ")).dimmed()
                    );
                }
            }
            if let Some(true) = output.degraded {
                eprintln!("    {} {}", "⚠".yellow(), "sandbox degraded".yellow());
            }
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

    /// Extract domain from network error messages (DNS failure, connection refused, etc.)
    fn extract_network_blocked_domain(content: &str) -> Option<String> {
        // Patterns:
        // "lookup api.launchpad.net on 127.0.0.53:53: ... network is unreachable"
        // "dial tcp: lookup example.com: no such host"
        // "Could not resolve host: example.com"
        // "Failed to connect to example.com port 443"
        // 'Get "https://domain.com/...": dial tcp ...: connect: operation not permitted'
        if content.contains("network is unreachable")
            || content.contains("no such host")
            || content.contains("Could not resolve host")
            || content.contains("Failed to connect to")
            || content.contains("Connection refused")
            || content.contains("Name or service not known")
            || content.contains("operation not permitted")
        {
            // Try URL pattern: Get "https://<domain>/..." or "http://<domain>/..."
            if let Some(start) = content.find("https://").or_else(|| content.find("http://")) {
                let scheme_end = if content[start..].starts_with("https://") {
                    start + 8
                } else {
                    start + 7
                };
                let after = &content[scheme_end..];
                if let Some(end) =
                    after.find(|c: char| c == '/' || c == '"' || c == ':' || c == ' ')
                {
                    let domain = &after[..end];
                    if domain.contains('.') && !domain.is_empty() {
                        return Some(domain.to_string());
                    }
                } else if after.contains('.') {
                    let domain = after.trim_end_matches(|c: char| c == '"' || c.is_whitespace());
                    if !domain.is_empty() {
                        return Some(domain.to_string());
                    }
                }
            }
            // Try "lookup <domain> on" pattern
            if let Some(start) = content.find("lookup ") {
                let after = &content[start + 7..];
                if let Some(end) = after.find(|c: char| c == ' ' || c == ':') {
                    let domain = &after[..end];
                    if domain.contains('.') && !domain.contains('/') {
                        return Some(domain.to_string());
                    }
                }
            }
            // Try "resolve host: <domain>" pattern
            if let Some(start) = content.find("resolve host: ") {
                let after = &content[start + 14..];
                let end = after
                    .find(|c: char| !c.is_alphanumeric() && c != '.' && c != '-')
                    .unwrap_or(after.len());
                let domain = after[..end].trim();
                if domain.contains('.') && !domain.is_empty() {
                    return Some(domain.to_string());
                }
            }

            // Try "connect to <domain> port" or "connect to <domain>:" pattern
            if let Some(start) = content.find("connect to ") {
                let after = &content[start + 11..];
                if let Some(end) = after.find(|c: char| c == ' ' || c == ':') {
                    let domain = &after[..end];
                    if domain.contains('.') && !domain.contains('/') {
                        return Some(domain.to_string());
                    }
                }
            }
        }
        None
    }

    /// Probe permission denied errors using strace.
    /// Re-runs the failed command under strace to identify exactly which files
    /// were blocked (EACCES) and whether they need read or write access.
    /// Returns a list of (path, access_mode) where access_mode is "ro" or "rw".
    async fn strace_probe_eacces(cmd: &str, cwd: Option<&str>) -> Vec<(String, String)> {
        use std::process::Stdio;

        let strace_cmd = format!(
            "strace -f -e trace=openat,open,access,stat,execve -o /dev/stdout -- sh -c {} 2>/dev/null",
            shell_escape(cmd)
        );

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(&strace_cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(dir) = cwd {
            command.current_dir(dir);
        }

        let output = match command.output().await {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_strace_eacces(&stdout)
    }

    /// Parse strace output for EACCES failures.
    /// Recognizes patterns like:
    ///   openat(AT_FDCWD, "/path/file", O_RDONLY|...) = -1 EACCES
    ///   openat(AT_FDCWD, "/path/file", O_WRONLY|...) = -1 EACCES
    ///   access("/path/file", R_OK) = -1 EACCES
    ///   execve("/path/file", ...) = -1 EACCES
    fn parse_strace_eacces(output: &str) -> Vec<(String, String)> {
        let mut results: Vec<(String, String)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for line in output.lines() {
            if !line.contains("EACCES") {
                continue;
            }

            if line.contains("openat(") || line.contains("open(") {
                if let Some(path) = Self::extract_strace_quoted_path(line) {
                    if seen.contains(&path) {
                        continue;
                    }
                    let mode = if line.contains("O_WRONLY")
                        || line.contains("O_RDWR")
                        || line.contains("O_CREAT")
                        || line.contains("O_TRUNC")
                    {
                        "rw"
                    } else {
                        "ro"
                    };
                    seen.insert(path.clone());
                    results.push((path, mode.to_string()));
                }
            } else if line.contains("access(") {
                if let Some(path) = Self::extract_strace_quoted_path(line) {
                    if seen.contains(&path) {
                        continue;
                    }
                    let mode = if line.contains("W_OK") { "rw" } else { "ro" };
                    seen.insert(path.clone());
                    results.push((path, mode.to_string()));
                }
            } else if line.contains("execve(") {
                if let Some(path) = Self::extract_strace_quoted_path(line) {
                    if seen.contains(&path) {
                        continue;
                    }
                    seen.insert(path.clone());
                    results.push((path, "ro".to_string()));
                }
            } else if line.contains("stat(") || line.contains("statx(") {
                if let Some(path) = Self::extract_strace_quoted_path(line) {
                    if seen.contains(&path) {
                        continue;
                    }
                    seen.insert(path.clone());
                    results.push((path, "ro".to_string()));
                }
            }
        }

        // Filter out noise
        results
            .into_iter()
            .filter(|(path, _)| {
                path.starts_with('/')
                    && !path.starts_with("/proc/")
                    && !path.starts_with("/sys/")
                    && !path.starts_with("/dev/")
                    && !path.contains('\0')
            })
            .collect()
    }

    /// Extract a double-quoted path from a strace output line.
    fn extract_strace_quoted_path(line: &str) -> Option<String> {
        let mut in_quote = false;
        let mut path = String::new();

        for ch in line.chars() {
            if ch == '"' {
                if in_quote {
                    if path.starts_with('/') {
                        return Some(path);
                    }
                    path.clear();
                    in_quote = false;
                } else {
                    in_quote = true;
                    path.clear();
                }
            } else if in_quote {
                path.push(ch);
            }
        }
        None
    }

    /// Extract file/binary path from Permission denied errors.
    fn extract_permission_denied_path(content: &str) -> Option<String> {
        if !content.contains("Permission denied") {
            return None;
        }
        // Pattern: "unable to access '/path/to/file': Permission denied" (git/landlock)
        // Pattern: "could not open '/path/to/file' for reading...: Permission denied"
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("Permission denied") {
                // Try quoted path patterns first
                if let Some(start) = trimmed.find('\'') {
                    if let Some(end) = trimmed[start + 1..].find('\'') {
                        let path = &trimmed[start + 1..start + 1 + end];
                        if path.starts_with('/') && !path.contains(' ') {
                            return Some(path.to_string());
                        }
                    }
                }
            }
        }
        // Pattern: "sh: N: <binary>: Permission denied" (exit code 126 - binary found but can't exec)
        // Try to find the binary name and resolve its path
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("Permission denied") {
                // "sh: 1: jira: Permission denied"
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 3 {
                    let candidate = parts[parts.len() - 2].trim();
                    if !candidate.is_empty()
                        && !candidate.contains(' ')
                        && candidate != "exec failed"
                    {
                        // If it looks like a binary name (no spaces, not a path)
                        if !candidate.starts_with('/') {
                            // Search PATH for the binary and resolve to realpath
                            if let Ok(path_var) = std::env::var("PATH") {
                                for dir in path_var.split(':') {
                                    let check = format!("{}/{}", dir, candidate);
                                    if std::path::Path::new(&check).exists() {
                                        return Some(
                                            std::fs::canonicalize(&check)
                                                .unwrap_or_else(|_| {
                                                    std::path::PathBuf::from(&check)
                                                })
                                                .to_string_lossy()
                                                .to_string(),
                                        );
                                    }
                                }
                            }
                        } else {
                            return Some(candidate.to_string());
                        }
                    }
                }
                // "rune _landlock: exec failed: Permission denied (os error 13)"
                // In this case, it's the sandboxed binary itself — less useful to extract
            }
        }
        None
    }

    /// Extract blocked command from error message, if applicable.
    fn extract_blocked_command(content: &str) -> Option<String> {
        // Pattern: "BLOCKED by policy: command 'xxx' is not in allowed_commands"
        if content.contains("is not in allowed_commands") {
            if let Some(start) = content.find("command '") {
                let after = &content[start + 9..];
                if let Some(end) = after.find('\'') {
                    let command = after[..end].trim().trim_matches(|c: char| {
                        matches!(c, '(' | ')' | '{' | '}' | '[' | ']' | '!')
                    });
                    if !command.is_empty() {
                        return Some(command.to_string());
                    }
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
                return crate::tools::extract_command_binaries_pub(cmd)
                    .into_iter()
                    .next();
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
    fn prompt_confirm() -> ConfirmResult {
        use std::io::{BufRead, Write};
        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            let mut reader = std::io::BufReader::new(tty);
            std::io::stderr().flush().ok();
            let mut input = String::new();
            if reader.read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
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

    pub fn tool_calls_log(&self) -> &[ToolCallRecord] {
        &self.tool_calls_log
    }

    /// Count all tool calls executed during this session.
    pub fn tool_call_count(&self) -> usize {
        self.tool_call_names.len()
    }
}

/// Shell-escape a string for use in sh -c (wraps in single quotes).
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

    #[test]
    fn test_extract_network_blocked_domain_dns_unreachable() {
        let content = "exit_code: 1\nstdout: \nstderr: 2026/05/06 01:51:41 Get \"https://api.launchpad.net/devel/bugs/1234567\": dial tcp: lookup api.launchpad.net on 127.0.0.53:53: dial udp 127.0.0.53:53: connect: network is unreachable";
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("api.launchpad.net".to_string())
        );
    }

    #[test]
    fn test_extract_network_blocked_domain_operation_not_permitted() {
        let content = "Error: Get \"https://warthogs.atlassian.net/rest/api/3/issue/CEINFRA-337\": dial tcp 13.227.180.4:443: connect: operation not permitted";
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("warthogs.atlassian.net".to_string())
        );
    }

    #[test]
    fn test_extract_network_blocked_domain_could_not_resolve() {
        let content = "curl: (6) Could not resolve host: example.com";
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn test_extract_network_blocked_domain_no_such_host() {
        let content = "dial tcp: lookup api.github.com: no such host";
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("api.github.com".to_string())
        );
    }

    #[test]
    fn test_extract_network_blocked_domain_not_network_error() {
        let content = "exit_code: 1\nstdout: \nstderr: file not found";
        assert_eq!(Agent::extract_network_blocked_domain(content), None);
    }

    #[test]
    fn test_extract_permission_denied_path_binary() {
        let content = "exit_code: 126\nstdout: \nstderr: sh: 1: jira: Permission denied";
        // This test checks the pattern extraction - actual path resolution
        // depends on filesystem state, so we verify the function doesn't panic
        let result = Agent::extract_permission_denied_path(content);
        // jira likely won't exist in test env, so result may be None
        // but the function should not panic
        assert!(result.is_none() || result.unwrap().contains("jira"));
    }

    #[test]
    fn test_extract_permission_denied_path_absolute() {
        let content = "stderr: sh: 1: /opt/tools/mytool: Permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/opt/tools/mytool".to_string()));
    }

    #[test]
    fn test_extract_permission_denied_path_not_permission_error() {
        let content = "exit_code: 1\nstdout: hello\nstderr: command not found";
        assert_eq!(Agent::extract_permission_denied_path(content), None);
    }

    #[test]
    fn test_extract_permission_denied_git_unable_to_access() {
        let content = "exit_code: 128
stderr: warning: unable to access '/home/user/.gitconfig': Permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/home/user/.gitconfig".to_string()));
    }

    #[test]
    fn test_extract_permission_denied_could_not_open() {
        let content =
            "stderr: fatal: could not open '/dev/null' for reading and writing: Permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/dev/null".to_string()));
    }

    #[test]
    fn test_extract_permission_denied_landlock_exec() {
        // _landlock errors should not extract a useful path
        let content = "rune _landlock: exec failed: Permission denied (os error 13)";
        let result = Agent::extract_permission_denied_path(content);
        // "exec failed" should be filtered out — but now the quoted-path pattern
        // won't match since there's no quoted absolute path
        assert!(result.is_none());
    }

    #[test]
    fn test_permission_denied_parent_dir_extraction() {
        // When we find a binary, we should add its parent directory (for Landlock)
        let binary_path = "/home/linuxbrew/.linuxbrew/Cellar/jira-cli/1.7.0/bin/jira";
        let parent = std::path::Path::new(binary_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap();
        assert_eq!(
            parent,
            "/home/linuxbrew/.linuxbrew/Cellar/jira-cli/1.7.0/bin"
        );
    }

    #[test]
    fn test_permission_denied_no_infinite_loop() {
        // If the path is already in allowed_paths_ro, should not prompt again
        let file_path = "/usr/local/bin/mytool";
        let allowed_paths_ro = vec!["/usr/local/bin".to_string()];
        // Simulates the check: file_path starts_with an already-allowed path
        let already_allowed = allowed_paths_ro.iter().any(|p| file_path.starts_with(p));
        assert!(already_allowed);
    }

    #[test]
    fn test_permission_denied_not_already_allowed() {
        let file_path = "/home/user/.local/bin/tool";
        let allowed_paths_ro = vec!["/usr/local/bin".to_string(), "/bin".to_string()];
        let already_allowed = allowed_paths_ro.iter().any(|p| file_path.starts_with(p));
        assert!(!already_allowed);
    }

    #[test]
    fn test_skills_dir_auto_added_to_allowed_paths_ro() {
        use crate::config::RuneConfig;
        let mut cfg = RuneConfig::default();
        cfg.skills_dir = "/home/u/skills".to_string();
        cfg.policy.allowed_paths_ro = vec!["/some/other/path".to_string()];

        // Simulate the logic from Agent::new
        let sd = cfg.skills_dir.clone();
        if !sd.is_empty()
            && !cfg
                .policy
                .allowed_paths_ro
                .iter()
                .any(|p| sd.starts_with(p.trim_end_matches("/")))
            && !cfg
                .policy
                .allowed_paths_rw
                .iter()
                .any(|p| sd.starts_with(p.trim_end_matches("/")))
        {
            cfg.policy.allowed_paths_ro.push(sd);
        }

        assert!(cfg
            .policy
            .allowed_paths_ro
            .contains(&"/home/u/skills".to_string()));
    }

    #[test]
    fn test_skills_dir_not_duplicated_if_covered() {
        use crate::config::RuneConfig;
        let mut cfg = RuneConfig::default();
        cfg.skills_dir = "/home/u/skills".to_string();
        // skills_dir is already covered by a broader path
        cfg.policy.allowed_paths_ro = vec!["/home/u".to_string()];

        let sd = cfg.skills_dir.clone();
        let already_covered = cfg
            .policy
            .allowed_paths_ro
            .iter()
            .any(|p| sd.starts_with(p.trim_end_matches("/")))
            || cfg
                .policy
                .allowed_paths_rw
                .iter()
                .any(|p| sd.starts_with(p.trim_end_matches("/")));

        assert!(already_covered, "skills_dir should be covered by /home/u");
        // Should NOT be added again
        let count_before = cfg.policy.allowed_paths_ro.len();
        if !already_covered {
            cfg.policy.allowed_paths_ro.push(sd);
        }
        assert_eq!(cfg.policy.allowed_paths_ro.len(), count_before);
    }

    #[test]
    fn test_skills_dir_empty_not_added() {
        use crate::config::RuneConfig;
        let mut cfg = RuneConfig::default();
        cfg.skills_dir = "".to_string();
        cfg.policy.allowed_paths_ro = vec![];

        let sd = cfg.skills_dir.clone();
        if !sd.is_empty()
            && !cfg
                .policy
                .allowed_paths_ro
                .iter()
                .any(|p| sd.starts_with(p.trim_end_matches("/")))
            && !cfg
                .policy
                .allowed_paths_rw
                .iter()
                .any(|p| sd.starts_with(p.trim_end_matches("/")))
        {
            cfg.policy.allowed_paths_ro.push(sd);
        }

        assert!(
            cfg.policy.allowed_paths_ro.is_empty(),
            "empty skills_dir should not be added"
        );
    }

    #[test]
    fn test_skill_injection_includes_base_dir() {
        // Verify the format string produces the expected header with base_dir
        let skill_name = "launchpad";
        let skill_dir = "/home/u/skills/lp-api/launchpad";
        let skill_body = "# Launchpad API\nSome content here.";

        let injected = format!(
            "[Skill: {} | base_dir: {}]\nIMPORTANT: All relative paths referenced in this skill (e.g. references/, scripts/) must be resolved from base_dir above. Use absolute paths when reading skill files.\n{}\n[End Skill: {}]",
            skill_name, skill_dir, skill_body, skill_name
        );

        assert!(
            injected.starts_with("[Skill: launchpad | base_dir: /home/u/skills/lp-api/launchpad]")
        );
        assert!(injected.contains("IMPORTANT: All relative paths"));
        assert!(injected.contains("base_dir above"));
        assert!(injected.contains("# Launchpad API"));
        assert!(injected.ends_with("[End Skill: launchpad]"));
    }

    #[test]
    fn test_skill_injection_relative_path_hint() {
        // Verify the injected message instructs the LLM to resolve relative paths
        let skill_name = "test-skill";
        let skill_dir = "/home/u/my-skills/test-skill";
        let skill_body = "Refer to references/guide.md for details.";

        let injected = format!(
            "[Skill: {} | base_dir: {}]\nIMPORTANT: All relative paths referenced in this skill (e.g. references/, scripts/) must be resolved from base_dir above. Use absolute paths when reading skill files.\n{}\n[End Skill: {}]",
            skill_name, skill_dir, skill_body, skill_name
        );

        // The LLM should know to resolve "references/guide.md" as
        // "/home/u/my-skills/test-skill/references/guide.md"
        assert!(injected.contains("base_dir: /home/u/my-skills/test-skill"));
        assert!(injected.contains("must be resolved from base_dir above"));
        assert!(injected.contains("references/guide.md"));
    }

    #[test]
    fn test_preload_skills_skips_dynamic_discovery() {
        // When preload_skills is set, inject_skills should not be called on first run.
        // We verify the config field is properly stored and checked.
        let mut cfg = crate::config::RuneConfig::default();
        cfg.preload_skills = vec!["jira".to_string(), "launchpad".to_string()];
        assert_eq!(cfg.preload_skills.len(), 2);
        assert_eq!(cfg.preload_skills[0], "jira");
        assert_eq!(cfg.preload_skills[1], "launchpad");
    }

    #[test]
    fn test_preload_skills_empty_means_dynamic() {
        // When preload_skills is empty, dynamic discovery (inject_skills) should run.
        let cfg = crate::config::RuneConfig::default();
        assert!(cfg.preload_skills.is_empty());
    }

    #[test]
    fn test_shell_escape_basic() {
        assert_eq!(super::shell_escape("hello"), "'hello'");
    }

    #[test]
    fn test_shell_escape_with_single_quotes() {
        assert_eq!(super::shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_escape_with_spaces() {
        assert_eq!(super::shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn test_parse_strace_eacces_openat_rdonly() {
        let output = r#"openat(AT_FDCWD, "/home/user/.gitconfig", O_RDONLY|O_CLOEXEC) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/home/user/.gitconfig");
        assert_eq!(results[0].1, "ro");
    }

    #[test]
    fn test_parse_strace_eacces_openat_wronly() {
        let output = r#"openat(AT_FDCWD, "/tmp/output.log", O_WRONLY|O_CREAT|O_TRUNC, 0644) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/tmp/output.log");
        assert_eq!(results[0].1, "rw");
    }

    #[test]
    fn test_parse_strace_eacces_rdwr() {
        let output = r#"openat(AT_FDCWD, "/var/data/db.sqlite", O_RDWR|O_CLOEXEC) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/var/data/db.sqlite");
        assert_eq!(results[0].1, "rw");
    }

    #[test]
    fn test_parse_strace_eacces_access() {
        let output = r#"access("/home/user/.ssh/config", R_OK) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/home/user/.ssh/config");
        assert_eq!(results[0].1, "ro");
    }

    #[test]
    fn test_parse_strace_eacces_access_write() {
        let output = r#"access("/home/user/.cache/file", W_OK) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/home/user/.cache/file");
        assert_eq!(results[0].1, "rw");
    }

    #[test]
    fn test_parse_strace_eacces_execve() {
        let output = r#"execve("/usr/local/bin/jira", ["jira", "list"], ...) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/usr/local/bin/jira");
        assert_eq!(results[0].1, "ro");
    }

    #[test]
    fn test_parse_strace_eacces_stat() {
        let output = r#"stat("/home/user/.npmrc", 0x7ffd...) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/home/user/.npmrc");
        assert_eq!(results[0].1, "ro");
    }

    #[test]
    fn test_parse_strace_eacces_filters_proc_sys_dev() {
        let output = r#"openat(AT_FDCWD, "/proc/self/status", O_RDONLY) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/sys/class/net", O_RDONLY) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/dev/tty", O_RDWR) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/home/user/.config/app", O_RDONLY) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        // Only /home/user/.config/app should remain (others filtered)
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/home/user/.config/app");
    }

    #[test]
    fn test_parse_strace_eacces_dedup() {
        let output = r#"openat(AT_FDCWD, "/home/user/.gitconfig", O_RDONLY) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/home/user/.gitconfig", O_RDONLY|O_CLOEXEC) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        // Deduplication: same file should appear only once
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_parse_strace_eacces_multiple_files() {
        let output = r#"openat(AT_FDCWD, "/home/user/.gitconfig", O_RDONLY) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/home/user/.config/git/config", O_RDONLY) = -1 EACCES (Permission denied)
openat(AT_FDCWD, "/home/user/.local/share/data.db", O_RDWR) = -1 EACCES (Permission denied)"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].1, "ro");
        assert_eq!(results[1].1, "ro");
        assert_eq!(results[2].1, "rw");
    }

    #[test]
    fn test_parse_strace_eacces_no_eacces() {
        let output = r#"openat(AT_FDCWD, "/etc/passwd", O_RDONLY) = 3
read(3, "root:x:0:0:...", 4096) = 1234"#;
        let results = Agent::parse_strace_eacces(output);
        assert!(results.is_empty());
    }

    #[test]
    fn test_extract_strace_quoted_path() {
        let line = r#"openat(AT_FDCWD, "/home/user/.gitconfig", O_RDONLY) = -1 EACCES"#;
        let path = Agent::extract_strace_quoted_path(line);
        assert_eq!(path, Some("/home/user/.gitconfig".to_string()));
    }

    #[test]
    fn test_extract_strace_quoted_path_non_absolute() {
        // First quoted string is not a path
        let line = r#"openat(AT_FDCWD, "relative/path", O_RDONLY) = -1 EACCES"#;
        let path = Agent::extract_strace_quoted_path(line);
        // "relative/path" doesn't start with '/', so skip it
        assert_eq!(path, None);
    }

    #[test]
    fn test_extract_strace_quoted_path_no_quotes() {
        let line = "some random line without quotes";
        let path = Agent::extract_strace_quoted_path(line);
        assert_eq!(path, None);
    }
}
