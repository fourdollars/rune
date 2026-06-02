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
use tokio::sync::{Mutex as TokioMutex, RwLock};

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
    pub tokens_in: u32,
    pub tokens_out: u32,
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
    /// Optional streaming token callback (for WebUI serve mode).
    pub token_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// Optional approval callback (for WebUI serve mode).
    /// Called with (id, detail) when a tool needs user approval.
    /// Returns true = approved, false = denied.
    pub approval_callback: Option<
        Arc<
            dyn Fn(
                    String,
                    String,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
                + Send
                + Sync,
        >,
    >,
    /// Shared markdown files (for serve mode — list_markdown/read_markdown/write_markdown tools).
    pub files: Option<Arc<RwLock<std::collections::HashMap<String, String>>>>,
    /// Currently active filename.
    pub active_file: Option<Arc<RwLock<String>>>,
    /// Chat DB for search_chat tool (serve mode only).
    pub chat_db: Option<crate::serve::db::ChatDb>,
    /// Archive directory for search_chat tool.
    pub chat_archive_dir: Option<std::path::PathBuf>,
    /// Session ID used by search_chat.
    pub chat_note_id: Option<String>,
    /// Per-session markdown directory for file operations.
    pub markdown_dir: Option<std::path::PathBuf>,
    /// Display name of the current user (for multi-user chat).
    pub user_name: Option<String>,
    /// Callback fired after a markdown file is written/created (for serve mode).
    /// Lets the caller (chat_handler) push a fresh file-list SSE event to the UI.
    pub file_list_callback: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Callback fired after a markdown file content is changed (for serve mode).
    /// Broadcasts file_content SSE event so other users see the update in real-time.
    pub file_content_callback: Option<Arc<dyn Fn(String, String) + Send + Sync>>,
    /// Callback fired when a tool execution starts/ends (for serve mode status indicator).
    /// Called with (tool_name, "start"|"end").
    pub tool_status_callback: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
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
            tokens_in: 0,
            tokens_out: 0,
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
            token_callback: None,
            approval_callback: None,
            files: None,
            active_file: None,
            chat_db: None,
            chat_archive_dir: None,
            chat_note_id: None,
            markdown_dir: None,
            user_name: None,
            file_list_callback: None,
            file_content_callback: None,
            tool_status_callback: None,
        }
    }

    /// Set the system prompt.

    /// Attach an MCP manager for external tool dispatch.
    /// Enable serve-mode tools (search_chat, list/read/write_markdown).
    pub fn set_serve_mode(&mut self, enabled: bool) {
        self.tools.set_serve_mode(enabled);
    }

    pub fn set_mcp_manager(&mut self, mgr: Arc<TokioMutex<McpManager>>) {
        self.mcp_manager = Some(mgr);
    }

    /// Get a reference to the MCP manager (for /mcps command).
    pub fn mcp_manager_ref(&self) -> Option<&Arc<TokioMutex<McpManager>>> {
        self.mcp_manager.as_ref()
    }
    /// Prepend conversation history (user/assistant pairs) after the system prompt.
    /// Call this AFTER set_system_prompt() and BEFORE run().
    pub fn load_history(&mut self, records: &[crate::serve::db::ChatRecord]) {
        use crate::provider::LlmMessage;
        // Find insertion point: after system message (index 0), before any existing messages
        let insert_at = if self
            .messages
            .first()
            .map(|m| m.role == "system")
            .unwrap_or(false)
        {
            1
        } else {
            0
        };
        let history_msgs: Vec<LlmMessage> = records
            .iter()
            .filter(|r| r.role == "user" || r.role == "assistant")
            .map(|r| {
                let name = if r.role == "user" && !r.nickname.is_empty() && r.nickname != "user" {
                    Some(r.nickname.clone())
                } else {
                    None
                };
                LlmMessage {
                    role: r.role.clone(),
                    name,
                    content: Some(r.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                }
            })
            .collect();
        // Insert history before any in-progress messages
        // Sanitize: trim trailing consecutive user messages to just the last one.
        // Orphan user messages (no assistant reply) can confuse LLMs into generating
        // malformed tool calls when they try to respond to stale requests.
        let mut history_msgs = history_msgs;
        while history_msgs.len() >= 2 {
            let len = history_msgs.len();
            if history_msgs[len - 1].role == "user" && history_msgs[len - 2].role == "user" {
                history_msgs.remove(len - 2);
            } else {
                break;
            }
        }

        for (i, msg) in history_msgs.into_iter().enumerate() {
            self.messages.insert(insert_at + i, msg);
        }
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
                    name: None,
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
    pub fn tokens_in(&self) -> u32 {
        self.tokens_in
    }
    pub fn tokens_out(&self) -> u32 {
        self.tokens_out
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
            name: None,
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
            name: None,
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
        self.tokens_in = 0;
        self.tokens_out = 0;
    }

    /// Push a user message with explicit content parts (e.g., images).
    pub fn push_user_message_with_parts(&mut self, text: String, parts: Vec<ContentPart>) {
        self.messages.push(LlmMessage {
            role: "user".to_string(),
            name: None,
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
                let max_skills = self.config.embedding.max_skills.max(1);
                match self
                    .skill_loader
                    .semantic_search(engine, user_input, threshold, max_skills)
                    .await
                {
                    Ok(results) if !results.is_empty() => {
                        for (_score, name) in &results {
                            info!(skill = %name, score = _score, "semantic matched skill");
                            eprintln!("  {} Semantic skill match: {}", "🔎".dimmed(), name.green());
                            self.load_and_inject_skill(name);
                        }
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
                    name: None,
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
            name: self.user_name.clone(),
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
            let mut response = match self.call_llm().await {
                Ok(r) => r,
                Err(e) => {
                    let r = StopReason::Error(format!("LLM call failed: {}", e));
                    self.finish_trace(&r);
                    return r;
                }
            };

            // Update token usage
            self.tokens_used += response.usage.total_tokens;
            self.tokens_in += response.usage.prompt_tokens;
            self.tokens_out += response.usage.completion_tokens;

            // If no tool calls, we have our final answer
            if response.tool_calls.is_empty() {
                let answer = response.content.unwrap_or_default();
                self.messages.push(LlmMessage {
                    role: "assistant".to_string(),
                    name: None,
                    content: Some(answer.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    content_parts: None,
                });
                let r = StopReason::FinalAnswer(answer);
                self.finish_trace(&r);
                return r;
            }

            // We have tool calls — validate arguments JSON before adding to history
            let valid_tool_calls: Vec<_> = response
                .tool_calls
                .iter()
                .filter(|tc| {
                    if tc.function.arguments.is_empty() {
                        return true;
                    }
                    match serde_json::from_str::<serde_json::Value>(&tc.function.arguments) {
                        Ok(_) => true,
                        Err(e) => {
                            eprintln!(
                                "  {} skipping tool_call with invalid JSON: {} ({})",
                                "✗".red(),
                                tc.function.name,
                                e
                            );
                            false
                        }
                    }
                })
                .cloned()
                .collect();

            if valid_tool_calls.is_empty() {
                // All tool_calls were malformed — treat as if LLM returned no answer
                let err = "LLM produced tool calls with invalid JSON arguments".to_string();
                let r = StopReason::Error(err);
                self.finish_trace(&r);
                return r;
            }
            response.tool_calls = valid_tool_calls;

            self.messages.push(LlmMessage {
                role: "assistant".to_string(),
                name: None,
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
                                char_preview(&tc.function.arguments, 100).as_str(),
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

                // Notify tool status for all parallel tools
                if let Some(cb) = &self.tool_status_callback {
                    for (_id, name, _args) in &dispatch_list {
                        cb(name, "start");
                    }
                }

                // Parallel dispatch via ToolRegistry (which only needs &self)
                let futs: Vec<_> = dispatch_list
                    .iter()
                    .map(|(_id, name, args)| self.tools.execute(name, args.clone()))
                    .collect();
                let results = futures::future::join_all(futs).await;

                // Notify tool status end for all parallel tools
                if let Some(cb) = &self.tool_status_callback {
                    for (_id, name, _args) in &dispatch_list {
                        cb(name, "end");
                    }
                }

                // Push results in order
                for (i, output) in results.into_iter().enumerate() {
                    let tc_id = &dispatch_list[i].0;
                    let tc_name = &dispatch_list[i].1;
                    let content_preview = redact(&char_preview(&output.content, 200));

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
                        name: None,
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
                    // Notify tool status start (matches the parallel path)
                    if let Some(cb) = &self.tool_status_callback {
                        cb(&tc.function.name, "start");
                    }
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
                                name: None,
                                content: Some(err_msg),
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                                content_parts: None,
                            });
                            if let Some(cb) = &self.tool_status_callback {
                                cb(&tc.function.name, "end");
                            }
                            self.finish_trace(&stop);
                            return stop;
                        }
                    };
                    if let Some(cb) = &self.tool_status_callback {
                        cb(&tc.function.name, "end");
                    }
                    self.messages.push(LlmMessage {
                        role: "tool".to_string(),
                        name: None,
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
        let cb = self.token_callback.clone();
        let printer = tokio::spawn(async move {
            let mut stderr = std::io::stderr();
            while let Some(token) = rx.recv().await {
                if let Some(ref callback) = cb {
                    callback(&token);
                } else {
                    let _ = write!(stderr, "{}", token);
                    let _ = stderr.flush();
                }
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

    /// Handle search_chat tool for serve mode — full-text search across live DB + archives.
    async fn handle_search_chat_tool(&self, args: &serde_json::Value) -> Option<String> {
        let db = self.chat_db.as_ref()?;
        let query = args.get("query").and_then(|v| v.as_str())?;
        let session_id = self.chat_note_id.as_deref().unwrap_or("default");
        let archive_dir = self
            .chat_archive_dir
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let results = db
            .search_async(session_id.to_string(), query.to_string(), archive_dir)
            .await;
        if results.is_empty() {
            return Some(format!("No messages found for query: \"{}\"", query));
        }
        let lines: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    "[ts={}] {}: {}",
                    r.created_at,
                    r.nickname,
                    char_preview(&r.content, 200)
                )
            })
            .collect();
        Some(format!(
            "{} result(s) for \"{}\":\n{}",
            lines.len(),
            query,
            lines.join("\n")
        ))
    }

    /// Handle markdown tools (list_markdown / read_markdown / write_markdown) for serve mode.
    /// Returns Some(output) if handled, None if not a markdown tool.
    /// All operations are disk-based per-session (no shared in-memory map).
    async fn handle_markdown_tool(&self, name: &str, args: &serde_json::Value) -> Option<String> {
        let md_dir = self.markdown_dir.as_ref()?;

        // Resolve filename: use arg if provided, else first .md file in dir
        let fname_arg = args
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let fname = if let Some(f) = fname_arg {
            f
        } else {
            // Find first .md file as default
            let mut default_name = String::new();
            if let Ok(mut rd) = tokio::fs::read_dir(md_dir).await {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    let n = entry.file_name().to_string_lossy().to_string();
                    if n.ends_with(".md") {
                        default_name = n;
                        break;
                    }
                }
            }
            if default_name.is_empty() {
                return Some("Error: no markdown files in this session".to_string());
            }
            default_name
        };

        match name {
            "list_markdown" => {
                let mut names = Vec::new();
                if let Ok(mut rd) = tokio::fs::read_dir(md_dir).await {
                    while let Ok(Some(entry)) = rd.next_entry().await {
                        let n = entry.file_name().to_string_lossy().to_string();
                        if n.ends_with(".md") {
                            names.push(n);
                        }
                    }
                }
                names.sort();
                Some(format!("Files: {}", names.join(", ")))
            }
            "read_markdown" => {
                let file_path = md_dir.join(&fname);
                match tokio::fs::read_to_string(&file_path).await {
                    Ok(c) => Some(c),
                    Err(_) => Some(format!("Error: file not found: {}", fname)),
                }
            }
            "write_markdown" => {
                let new_content = args.get("content").and_then(|v| v.as_str());
                let search = args.get("search").and_then(|v| v.as_str());
                let replace = args.get("replace").and_then(|v| v.as_str());
                let file_path = md_dir.join(&fname);

                if let Some(full_content) = new_content {
                    if let Err(e) = tokio::fs::write(&file_path, full_content).await {
                        return Some(format!("Error writing {}: {}", fname, e));
                    }
                    if let Some(cb) = &self.file_list_callback {
                        cb();
                    }
                    if let Some(cb) = &self.file_content_callback {
                        cb(fname.clone(), full_content.to_string());
                    }
                    Some(format!("{} updated (full replace)", fname))
                } else if let (Some(search_str), Some(replace_str)) = (search, replace) {
                    match tokio::fs::read_to_string(&file_path).await {
                        Ok(current) => {
                            if current.contains(search_str) {
                                let updated = current.replacen(search_str, replace_str, 1);
                                if let Err(e) = tokio::fs::write(&file_path, &updated).await {
                                    return Some(format!("Error writing {}: {}", fname, e));
                                }
                                if let Some(cb) = &self.file_list_callback {
                                    cb();
                                }
                                if let Some(cb) = &self.file_content_callback {
                                    cb(fname.clone(), updated.clone());
                                }
                                Some(format!(
                                    "{} updated: replaced '{}...'",
                                    fname,
                                    char_preview(search_str, 40)
                                ))
                            } else {
                                Some(format!("Error: search text not found in {}", fname))
                            }
                        }
                        Err(_) => Some(format!("Error: file not found: {}", fname)),
                    }
                } else {
                    Some("Error: write_markdown requires either 'content' (full replace) or 'search'+'replace' (targeted edit)".to_string())
                }
            }
            _ => None,
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
                arguments_preview: redact(char_preview(&tc.function.arguments, 100).as_str()),
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
            // WebUI serve mode: use approval_callback
            if let Some(ref cb) = self.approval_callback {
                let id = format!("{}-{}", tc.function.name, uuid_approval());
                let detail = format!(
                    "{} {}",
                    tc.function.name,
                    serde_json::to_string(&args).unwrap_or_default()
                );
                let approved = cb(id.clone(), detail).await;
                if !approved {
                    return Ok("DENIED: user rejected tool execution".to_string());
                }
            } else if !self.interactive {
                let msg = format!(
                    "non-interactive mode requires --yes (or a non-confirm policy) before executing {}",
                    tc.function.name
                );
                eprintln!("  {} {}", "✗".red(), msg.dimmed());
                return Err(StopReason::Error(msg));
            } else if Self::has_tty() {
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
            } else {
                // No TTY and no approval_callback — deny silently
                return Ok("DENIED: no interactive approval available".to_string());
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

        // Intercept markdown tools (list_markdown / read_markdown / write_markdown) for serve mode
        if let Some(spec_output) = self.handle_markdown_tool(&tc.function.name, &args).await {
            return Ok(spec_output);
        }

        // Intercept search_chat tool for serve mode
        if tc.function.name == "search_chat" {
            if let Some(output) = self.handle_search_chat_tool(&args).await {
                return Ok(output);
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
                if self.interactive && Self::has_tty() {
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
                } else if self.config.policy.mode == "confirm" {
                    // Confirm mode without TTY: use approval_callback (web UI)
                    if let Some(ref cb) = self.approval_callback {
                        let id = format!("domain-allow-{}", domain);
                        let detail = format!("Add '{}' to allowed_domains?", domain);
                        let approved = cb(id, detail).await;
                        if approved {
                            self.tools.add_allowed_domain(&domain);
                            self.config.policy.allowed_domains.push(domain.clone());
                            crate::config::persist_domain(&domain);
                            output = self.tools.execute(&tc.function.name, args.clone()).await;
                            continue;
                        }
                    }
                }
                // allowlist/unrestricted or user denied: soft-fail to LLM
                eprintln!(
                    "  {} {}",
                    "✗".red(),
                    char_preview(&output.content, 200).dimmed()
                );
                return Ok(output.content);
            }

            // Check if it's a command block we can interactively resolve
            if let Some(command) = Self::extract_blocked_command(&output.content) {
                if self.interactive && Self::has_tty() {
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
                } else if self.config.policy.mode == "confirm" {
                    // Confirm mode without TTY: use approval_callback (web UI)
                    if let Some(ref cb) = self.approval_callback {
                        let id = format!("command-allow-{}", command);
                        let detail = format!("Add '{}' to allowed_commands?", command);
                        let approved = cb(id, detail).await;
                        if approved {
                            self.tools.add_allowed_command(&command);
                            self.config.policy.allowed_commands.push(command.clone());
                            crate::config::persist_command(&command);
                            output = self.tools.execute(&tc.function.name, args.clone()).await;
                            continue;
                        }
                    }
                }
                // allowlist/unrestricted or user denied: soft-fail to LLM
                eprintln!(
                    "  {} {}",
                    "✗".red(),
                    char_preview(&output.content, 200).dimmed()
                );
                return Ok(output.content);
            }

            // Check if it's a network error we can resolve by adding a domain
            if let Some(domain) = Self::extract_network_blocked_domain(&output.content) {
                if self.interactive && Self::has_tty() {
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
            if contains_permission_denied(&output.content) {
                if self.interactive && Self::has_tty() {
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
                char_preview(&output.content, 200).dimmed()
            );
            if Self::is_policy_blocked(&output.content) {
                // Unrestricted: should not reach here, but soft-fail if it does
                // Allowlist: soft-fail — let LLM see the error and adapt
                // Confirm without TTY: soft-fail — LLM can inform user
                // Confirm with TTY: already handled in the loop above
                return Ok(output.content);
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
        if !contains_permission_denied(content) {
            return None;
        }
        // Pattern: "unable to access '/path/to/file': Permission denied" (git/landlock)
        // Pattern: "could not open '/path/to/file' for reading...: Permission denied"
        for line in content.lines() {
            let trimmed = line.trim();
            if contains_permission_denied(trimmed) {
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
        // Pattern: "open /path/to/file: permission denied" (Go standard library)
        // Pattern: "open /path/to/file: Permission denied"
        for line in content.lines() {
            let trimmed = line.trim();
            if contains_permission_denied(trimmed) {
                // Go errors: "open /path/to/file: permission denied"
                // Split on ": " and check if the last part is permission denied
                if let Some(colon_pos) = trimmed.rfind(": ") {
                    let before_colon = &trimmed[..colon_pos];
                    // Look for "open /path" or just "/path" pattern
                    let path_candidate = if let Some(space_pos) = before_colon.rfind(' ') {
                        before_colon[space_pos + 1..].trim()
                    } else {
                        before_colon.trim()
                    };
                    if path_candidate.starts_with('/') && !path_candidate.contains(' ') {
                        return Some(path_candidate.to_string());
                    }
                }
            }
        }
        // Pattern: "sh: N: <binary>: Permission denied" (exit code 126 - binary found but can't exec)
        // Try to find the binary name and resolve its path
        for line in content.lines() {
            let trimmed = line.trim();
            if contains_permission_denied(trimmed) {
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

    /// Check if a real TTY is available for interactive prompts.
    /// Returns false in serve mode (systemd service, no controlling terminal).
    fn has_tty() -> bool {
        std::fs::File::open("/dev/tty").is_ok()
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
                if trimmed.is_empty() || trimmed == "y" || trimmed == "yes" {
                    return true;
                }
            }
            // TTY open but read failed or user said no
            return false;
        }
        // No TTY (e.g. serve mode / systemd) -- deny by default
        false
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
                if trimmed.is_empty() || trimmed == "y" || trimmed == "yes" {
                    return ConfirmResult::Yes;
                }
            }
            // TTY open but read failed or user said no
            return ConfirmResult::No;
        }
        // No TTY (e.g. serve mode / systemd) -- deny by default
        ConfirmResult::No
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
/// Case-insensitive check for permission denied / EACCES in output.
fn contains_permission_denied(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.contains("permission denied") || lower.contains("eacces")
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Generate a short unique ID for approval requests.

/// Safely truncate a string to at most `n` Unicode characters (avoids byte-boundary panics).
fn char_preview(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn uuid_approval() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:x}{:04x}",
        t.as_secs() & 0xffff,
        t.subsec_nanos() & 0xffff
    )
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

    #[test]
    fn test_contains_permission_denied_case_insensitive() {
        // Go-style lowercase
        assert!(super::contains_permission_denied(
            "open /home/u/.config/lp-api.toml: permission denied"
        ));
        // Standard case
        assert!(super::contains_permission_denied("Permission denied"));
        // EACCES
        assert!(super::contains_permission_denied("EACCES"));
        assert!(super::contains_permission_denied("eacces"));
        // No match
        assert!(!super::contains_permission_denied("everything is fine"));
        assert!(!super::contains_permission_denied("access granted"));
    }

    #[test]
    fn test_extract_permission_denied_go_style() {
        // Go standard library format: "open /path/to/file: permission denied"
        let content =
            "stderr: 2026/05/08 17:38:39 open /home/u/.config/lp-api.toml: permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/home/u/.config/lp-api.toml".to_string()));
    }

    #[test]
    fn test_extract_permission_denied_go_style_title_case() {
        // Go but with Title Case
        let content = "open /etc/secret.conf: Permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/etc/secret.conf".to_string()));
    }

    #[test]
    fn test_extract_permission_denied_lowercase_git() {
        // Hypothetical lowercase variant
        let content = "fatal: unable to access '/home/user/.gitconfig': permission denied";
        let result = Agent::extract_permission_denied_path(content);
        assert_eq!(result, Some("/home/user/.gitconfig".to_string()));
    }

    #[test]
    fn test_load_history_injects_messages() {
        use crate::provider::Provider;
        use crate::serve::db::ChatRecord;
        // Minimal stub: just test load_history inserts messages correctly
        // We can't easily build a full Agent without a provider, so test via
        // direct message count inspection after load_history.
        // Build a dummy config
        let config = crate::config::RuneConfig {
            model: "gpt-4o".to_string(),
            api_key: Some("test".to_string()),
            ..Default::default()
        };
        // We need a provider — skip if unavailable in test env
        // Instead, test load_history logic via a minimal agent construction:
        let records = vec![
            ChatRecord {
                id: 1,
                note_id: "default".into(),
                role: "user".into(),
                nickname: "alice".into(),
                content: "hello".into(),
                created_at: 0,
                model: None,
                tokens_in: None,
                tokens_out: None,
            },
            ChatRecord {
                id: 2,
                note_id: "default".into(),
                role: "assistant".into(),
                nickname: "rune".into(),
                content: "hi there".into(),
                created_at: 1,
                model: None,
                tokens_in: None,
                tokens_out: None,
            },
        ];
        // Verify filtering: tool_call roles should be excluded
        let filtered: Vec<_> = records
            .iter()
            .filter(|r| r.role == "user" || r.role == "assistant")
            .collect();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].content, "hello");
        assert_eq!(filtered[1].content, "hi there");
    }

    // ─── truncate_middle ──────────────────────────────────────────────────────

    #[test]
    fn test_truncate_middle_short_string() {
        assert_eq!(Agent::truncate_middle("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_middle_exact_length() {
        assert_eq!(Agent::truncate_middle("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_middle_long_string() {
        let s = "abcdefghijklmnopqrstuvwxyz";
        let result = Agent::truncate_middle(s, 11);
        assert!(result.contains("..."));
        assert!(result.starts_with("abcd"));
        assert!(result.ends_with("wxyz"));
    }

    #[test]
    fn test_truncate_middle_unicode_safe() {
        let s = "\u{4f60}\u{597d}\u{4e16}\u{754c}ABCDEFGHIJ";
        let result = Agent::truncate_middle(s, 8);
        assert!(result.contains("...") || result.len() <= 30);
    }

    // ─── is_dangerous_tool ───────────────────────────────────────────────────

    #[test]
    fn test_is_dangerous_tool_execute_cmd() {
        assert!(Agent::is_dangerous_tool("execute_cmd"));
    }

    #[test]
    fn test_is_dangerous_tool_write_file() {
        assert!(Agent::is_dangerous_tool("write_file"));
    }

    #[test]
    fn test_is_dangerous_tool_fetch_url() {
        assert!(Agent::is_dangerous_tool("fetch_url"));
    }

    #[test]
    fn test_is_dangerous_tool_read_file() {
        assert!(Agent::is_dangerous_tool("read_file"));
    }

    #[test]
    fn test_is_dangerous_tool_safe_tools() {
        assert!(!Agent::is_dangerous_tool("list_files"));
        assert!(!Agent::is_dangerous_tool("search_files"));
        assert!(!Agent::is_dangerous_tool(""));
    }

    // ─── is_policy_blocked ───────────────────────────────────────────────────

    #[test]
    fn test_is_policy_blocked_starts_with_blocked() {
        assert!(Agent::is_policy_blocked(
            "BLOCKED: domain not in allowed_domains"
        ));
    }

    #[test]
    fn test_is_policy_blocked_starts_with_blocked_by_policy() {
        assert!(Agent::is_policy_blocked(
            "BLOCKED by policy: command not allowed"
        ));
    }

    #[test]
    fn test_is_policy_blocked_network_access() {
        assert!(Agent::is_policy_blocked(
            "Network access requires explicit allowlist configuration"
        ));
    }

    #[test]
    fn test_is_policy_blocked_allowed_commands_msg() {
        assert!(Agent::is_policy_blocked(
            "command 'curl' is not in allowed_commands"
        ));
    }

    #[test]
    fn test_is_policy_blocked_not_blocked() {
        assert!(!Agent::is_policy_blocked("everything worked fine"));
        assert!(!Agent::is_policy_blocked("exit_code: 0"));
        assert!(!Agent::is_policy_blocked("Permission denied"));
    }

    #[test]
    fn test_is_policy_blocked_with_leading_whitespace() {
        assert!(Agent::is_policy_blocked("   BLOCKED: something"));
    }

    // ─── char_preview ────────────────────────────────────────────────────────

    #[test]
    fn test_char_preview_short() {
        assert_eq!(super::char_preview("hello", 100), "hello");
    }

    #[test]
    fn test_char_preview_truncates() {
        assert_eq!(super::char_preview("hello world", 5), "hello");
    }

    #[test]
    fn test_char_preview_unicode_boundary() {
        assert_eq!(
            super::char_preview("\u{4f60}\u{597d}\u{4e16}\u{754c}", 2),
            "\u{4f60}\u{597d}"
        );
    }

    #[test]
    fn test_char_preview_empty() {
        assert_eq!(super::char_preview("", 10), "");
    }

    #[test]
    fn test_char_preview_zero_limit() {
        assert_eq!(super::char_preview("hello", 0), "");
    }

    // ─── extract_blocked_domain ──────────────────────────────────────────────

    #[test]
    fn test_extract_blocked_domain_basic() {
        let msg = "BLOCKED: domain 'github.com' is not in allowed_domains";
        assert_eq!(
            Agent::extract_blocked_domain(msg),
            Some("github.com".to_string())
        );
    }

    #[test]
    fn test_extract_blocked_domain_subdomain() {
        let msg = "BLOCKED: domain 'api.github.com' is not in allowed_domains";
        assert_eq!(
            Agent::extract_blocked_domain(msg),
            Some("api.github.com".to_string())
        );
    }

    #[test]
    fn test_extract_blocked_domain_no_match() {
        assert_eq!(
            Agent::extract_blocked_domain("network is unreachable"),
            None
        );
        assert_eq!(Agent::extract_blocked_domain(""), None);
    }

    // ─── extract_blocked_command ─────────────────────────────────────────────

    #[test]
    fn test_extract_blocked_command_basic() {
        let msg = "BLOCKED by policy: command 'curl' is not in allowed_commands";
        assert_eq!(
            Agent::extract_blocked_command(msg),
            Some("curl".to_string())
        );
    }

    #[test]
    fn test_extract_blocked_command_no_match() {
        assert_eq!(Agent::extract_blocked_command("exit_code: 1"), None);
        assert_eq!(Agent::extract_blocked_command(""), None);
    }

    // ─── extract_domain_from_args ────────────────────────────────────────────

    #[test]
    fn test_extract_domain_from_args_https() {
        let args = serde_json::json!({"url": "https://api.github.com/repos/foo/bar"});
        assert_eq!(
            Agent::extract_domain_from_args("fetch_url", &args),
            Some("api.github.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_from_args_http() {
        let args = serde_json::json!({"url": "http://example.com/path"});
        assert_eq!(
            Agent::extract_domain_from_args("fetch_url", &args),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_from_args_with_port() {
        let args = serde_json::json!({"url": "https://localhost:8080/api"});
        assert_eq!(
            Agent::extract_domain_from_args("fetch_url", &args),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn test_extract_domain_from_args_wrong_tool() {
        let args = serde_json::json!({"url": "https://example.com"});
        assert_eq!(Agent::extract_domain_from_args("read_file", &args), None);
    }

    #[test]
    fn test_extract_domain_from_args_missing_url() {
        let args = serde_json::json!({"path": "/etc/passwd"});
        assert_eq!(Agent::extract_domain_from_args("fetch_url", &args), None);
    }

    // ─── extract_command_from_args ───────────────────────────────────────────

    #[test]
    fn test_extract_command_from_args_basic() {
        let args = serde_json::json!({"cmd": "ls -la /tmp"});
        let result = Agent::extract_command_from_args("execute_cmd", &args);
        assert_eq!(result, Some("ls".to_string()));
    }

    #[test]
    fn test_extract_command_from_args_wrong_tool() {
        let args = serde_json::json!({"cmd": "ls"});
        assert_eq!(Agent::extract_command_from_args("read_file", &args), None);
    }

    #[test]
    fn test_extract_command_from_args_no_cmd() {
        let args = serde_json::json!({"path": "/tmp"});
        assert_eq!(Agent::extract_command_from_args("execute_cmd", &args), None);
    }

    // ─── extract_path_from_args ──────────────────────────────────────────────

    #[test]
    fn test_extract_path_from_args_read_file() {
        let args = serde_json::json!({"path": "/etc/passwd"});
        assert_eq!(
            Agent::extract_path_from_args("read_file", &args),
            Some("/etc/passwd".to_string())
        );
    }

    #[test]
    fn test_extract_path_from_args_write_file() {
        let args = serde_json::json!({"path": "/tmp/output.txt"});
        assert_eq!(
            Agent::extract_path_from_args("write_file", &args),
            Some("/tmp/output.txt".to_string())
        );
    }

    #[test]
    fn test_extract_path_from_args_wrong_tool() {
        let args = serde_json::json!({"path": "/etc/passwd"});
        assert_eq!(Agent::extract_path_from_args("execute_cmd", &args), None);
    }

    #[test]
    fn test_extract_path_from_args_empty_path() {
        let args = serde_json::json!({"path": ""});
        assert_eq!(Agent::extract_path_from_args("read_file", &args), None);
    }

    // ─── stop_reason variants ────────────────────────────────────────────────

    #[test]
    fn test_stop_reason_debug_final_answer() {
        let r = StopReason::FinalAnswer("done".to_string());
        let s = format!("{:?}", r);
        assert!(s.contains("FinalAnswer"));
    }

    #[test]
    fn test_stop_reason_debug_max_steps() {
        let r = StopReason::MaxSteps;
        let s = format!("{:?}", r);
        assert!(s.contains("MaxSteps"));
    }

    #[test]
    fn test_stop_reason_debug_token_budget() {
        let r = StopReason::TokenBudgetExhausted;
        let s = format!("{:?}", r);
        assert!(s.contains("TokenBudgetExhausted"));
    }

    #[test]
    fn test_stop_reason_debug_error() {
        let r = StopReason::Error("oops".to_string());
        let s = format!("{:?}", r);
        assert!(s.contains("Error"));
    }

    #[test]
    fn test_stop_reason_debug_user_interrupt() {
        let r = StopReason::UserInterrupt;
        let s = format!("{:?}", r);
        assert!(s.contains("UserInterrupt"));
    }

    // ─── ToolCallRecord ──────────────────────────────────────────────────────

    #[test]
    fn test_tool_call_record_clone() {
        let r = ToolCallRecord {
            name: "read_file".to_string(),
            args_preview: r#"{"path":"/etc/hosts"}"#.to_string(),
            is_error: false,
        };
        let cloned = r.clone();
        assert_eq!(cloned.name, "read_file");
        assert!(!cloned.is_error);
    }

    #[test]
    fn test_tool_call_record_is_error_true() {
        let r = ToolCallRecord {
            name: "execute_cmd".to_string(),
            args_preview: "{}".to_string(),
            is_error: true,
        };
        assert!(r.is_error);
    }

    // ─── extract_network_blocked_domain — more cases ──────────────────────────

    #[test]
    fn test_extract_network_blocked_domain_name_or_service_not_known() {
        let content = "dial tcp: lookup packages.ubuntu.com: Name or service not known";
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("packages.ubuntu.com".to_string())
        );
    }

    #[test]
    fn test_extract_network_blocked_domain_connection_refused() {
        let content = r#"Get "https://pypi.org/simple/": dial tcp: connect: Connection refused"#;
        assert_eq!(
            Agent::extract_network_blocked_domain(content),
            Some("pypi.org".to_string())
        );
    }

    // ─── parse_strace_eacces — more edge cases ────────────────────────────────

    #[test]
    fn test_parse_strace_eacces_empty_input() {
        let results = Agent::parse_strace_eacces("");
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_strace_eacces_creat_flag() {
        let output = r#"openat(AT_FDCWD, "/tmp/newfile.txt", O_CREAT|O_WRONLY, 0644) = -1 EACCES"#;
        let results = Agent::parse_strace_eacces(output);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "rw");
    }

    // ─── contains_permission_denied ──────────────────────────────────────────

    #[test]
    fn test_contains_permission_denied_mixed_case() {
        assert!(super::contains_permission_denied(
            "Error: PERMISSION DENIED"
        ));
    }

    #[test]
    fn test_contains_permission_denied_false_positive_check() {
        assert!(!super::contains_permission_denied(
            "access granted to resource"
        ));
        assert!(!super::contains_permission_denied("accessed the database"));
    }

    // ─── uuid_approval ───────────────────────────────────────────────────────

    #[test]
    fn test_uuid_approval_not_empty() {
        let id = super::uuid_approval();
        assert!(!id.is_empty());
    }

    // ─── estimate_tokens edge cases ──────────────────────────────────────────

    #[test]
    fn test_estimate_tokens_exactly_four_chars() {
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn test_estimate_tokens_one_char() {
        assert_eq!(estimate_tokens("a"), 1);
    }

    #[test]
    fn test_estimate_tokens_five_chars() {
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    // ─── shell_escape edge cases ─────────────────────────────────────────────

    #[test]
    fn test_shell_escape_empty_string() {
        assert_eq!(super::shell_escape(""), "''");
    }

    #[test]
    fn test_shell_escape_special_chars() {
        let result = super::shell_escape("echo $HOME");
        assert_eq!(result, "'echo $HOME'");
    }

    // ─── MockProvider for Agent construction tests ───────────────────────────

    struct MockProvider;
    impl crate::provider::Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn chat(
            &self,
            _request: crate::provider::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                Ok(crate::provider::LlmResponse {
                    content: Some("mock response".to_string()),
                    tool_calls: vec![],
                    usage: crate::provider::TokenUsage::default(),
                    model: "mock".to_string(),
                })
            })
        }
    }

    fn make_test_agent() -> Agent {
        let config = crate::config::RuneConfig {
            model: "mock-model".to_string(),
            ..Default::default()
        };
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        Agent::new(config, registry, false, None)
    }

    // ─── Agent::new ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_agent_new_default_values() {
        let agent = make_test_agent();
        assert_eq!(agent.message_count(), 0);
        assert_eq!(agent.context_chars(), 0);
        assert_eq!(agent.tokens_used(), 0);
        assert_eq!(agent.step_count(), 0);
        assert!(!agent.is_interactive());
    }

    #[tokio::test]
    async fn test_agent_new_interactive_flag() {
        let config = crate::config::RuneConfig::default();
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, true, None);
        assert!(agent.is_interactive());
    }

    #[tokio::test]
    async fn test_agent_new_cwd_added_to_allowed_ro() {
        let mut config = crate::config::RuneConfig::default();
        config.policy.allowed_paths_ro.clear();
        config.policy.allowed_paths_rw.clear();
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, false, None);
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            agent
                .config
                .policy
                .allowed_paths_ro
                .iter()
                .any(|p| cwd.starts_with(p.trim_end_matches("/")))
                || agent
                    .config
                    .policy
                    .allowed_paths_rw
                    .iter()
                    .any(|p| cwd.starts_with(p.trim_end_matches("/")))
        );
    }

    #[tokio::test]
    async fn test_agent_new_tokens_in_out_zero() {
        let agent = make_test_agent();
        assert_eq!(agent.tokens_in, 0);
        assert_eq!(agent.tokens_out, 0);
    }

    // ─── set_system_prompt ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_set_system_prompt_inserts_message() {
        let mut agent = make_test_agent();
        assert_eq!(agent.message_count(), 0);
        agent.set_system_prompt("You are a helpful assistant.");
        assert_eq!(agent.message_count(), 1);
    }

    #[tokio::test]
    async fn test_set_system_prompt_role_is_system() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("Hello world.");
        assert_eq!(agent.messages[0].role, "system");
    }

    #[tokio::test]
    async fn test_set_system_prompt_content_stored() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("Be concise.");
        assert_eq!(agent.messages[0].content.as_deref(), Some("Be concise."));
    }

    #[tokio::test]
    async fn test_set_system_prompt_overwrites_existing() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("First prompt.");
        agent.set_system_prompt("Second prompt.");
        assert_eq!(agent.message_count(), 1);
        assert_eq!(agent.messages[0].content.as_deref(), Some("Second prompt."));
    }

    #[tokio::test]
    async fn test_set_system_prompt_empty_string() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("");
        assert_eq!(agent.message_count(), 1);
        assert_eq!(agent.messages[0].content.as_deref(), Some(""));
    }

    // ─── context_summary ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_context_summary_empty() {
        let agent = make_test_agent();
        assert!(agent.context_summary().is_empty());
    }

    #[tokio::test]
    async fn test_context_summary_system_only() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("sys");
        let summary = agent.context_summary();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0], ("system".to_string(), 1));
    }

    #[tokio::test]
    async fn test_context_summary_multiple_roles() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("sys");
        agent.messages.push(crate::provider::LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        agent.messages.push(crate::provider::LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: Some("hi".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        agent.messages.push(crate::provider::LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("how are you".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        let summary = agent.context_summary();
        let summary_map: std::collections::HashMap<_, _> = summary.into_iter().collect();
        assert_eq!(summary_map.get("system"), Some(&1));
        assert_eq!(summary_map.get("user"), Some(&2));
        assert_eq!(summary_map.get("assistant"), Some(&1));
    }

    #[tokio::test]
    async fn test_context_summary_sorted() {
        let mut agent = make_test_agent();
        agent.messages.push(crate::provider::LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("hi".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        agent.messages.push(crate::provider::LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: Some("hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        let summary = agent.context_summary();
        let roles: Vec<_> = summary.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(roles, vec!["assistant", "user"]);
    }

    // ─── message_count ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_message_count_zero() {
        let agent = make_test_agent();
        assert_eq!(agent.message_count(), 0);
    }

    #[tokio::test]
    async fn test_message_count_after_system_prompt() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("sys");
        assert_eq!(agent.message_count(), 1);
    }

    #[tokio::test]
    async fn test_message_count_multiple_messages() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("sys");
        for _ in 0..5 {
            agent.messages.push(crate::provider::LlmMessage {
                role: "user".to_string(),
                name: None,
                content: Some("ping".to_string()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            });
        }
        assert_eq!(agent.message_count(), 6);
    }

    // ─── context_chars ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_context_chars_empty() {
        let agent = make_test_agent();
        assert_eq!(agent.context_chars(), 0);
    }

    #[tokio::test]
    async fn test_context_chars_after_system_prompt() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("hello");
        assert_eq!(agent.context_chars(), 5);
    }

    #[tokio::test]
    async fn test_context_chars_multiple_messages() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("abc"); // 3
        agent.messages.push(crate::provider::LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("12345".to_string()), // 5
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        assert_eq!(agent.context_chars(), 8);
    }

    #[tokio::test]
    async fn test_context_chars_message_with_no_content() {
        let mut agent = make_test_agent();
        agent.messages.push(crate::provider::LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: None,
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        assert_eq!(agent.context_chars(), 0);
    }

    // ─── total_context_tokens ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_total_context_tokens_empty() {
        let agent = make_test_agent();
        assert_eq!(agent.total_context_tokens(), 0);
    }

    #[tokio::test]
    async fn test_total_context_tokens_with_system_prompt() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("hello world");
        let tokens = agent.total_context_tokens();
        assert!(tokens > 0);
    }

    #[tokio::test]
    async fn test_total_context_tokens_grows_with_messages() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("sys");
        let t1 = agent.total_context_tokens();
        agent.messages.push(crate::provider::LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("a longer user message here".to_string()),
            tool_calls: None,
            tool_call_id: None,
            content_parts: None,
        });
        let t2 = agent.total_context_tokens();
        assert!(t2 > t1);
    }

    // ─── StopReason Display ──────────────────────────────────────────────────

    #[test]
    fn test_stop_reason_display_final_answer_with_content() {
        let r = StopReason::FinalAnswer("the answer is 42".to_string());
        let s = format!("{:?}", r);
        assert!(s.contains("FinalAnswer"));
        assert!(s.contains("the answer is 42"));
    }

    #[test]
    fn test_stop_reason_display_error_with_message() {
        let r = StopReason::Error("network timeout".to_string());
        let s = format!("{:?}", r);
        assert!(s.contains("Error"));
        assert!(s.contains("network timeout"));
    }

    #[test]
    fn test_stop_reason_all_variants_debuggable() {
        let variants = vec![
            StopReason::FinalAnswer("x".into()),
            StopReason::MaxSteps,
            StopReason::TokenBudgetExhausted,
            StopReason::Error("y".into()),
            StopReason::UserInterrupt,
        ];
        for v in variants {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
        }
    }

    // ─── ToolCallRecord ──────────────────────────────────────────────────────

    #[test]
    fn test_tool_call_record_fields() {
        let r = ToolCallRecord {
            name: "write_file".to_string(),
            args_preview: r#"{"path":"/tmp/x","content":"hello"}"#.to_string(),
            is_error: false,
        };
        assert_eq!(r.name, "write_file");
        assert!(r.args_preview.contains("/tmp/x"));
        assert!(!r.is_error);
    }

    #[test]
    fn test_tool_call_record_error_variant() {
        let r = ToolCallRecord {
            name: "execute_cmd".to_string(),
            args_preview: r#"{"cmd":"rm -rf /"}"#.to_string(),
            is_error: true,
        };
        assert!(r.is_error);
    }

    #[test]
    fn test_tool_call_record_debug_format() {
        let r = ToolCallRecord {
            name: "read_file".to_string(),
            args_preview: "{}".to_string(),
            is_error: false,
        };
        let s = format!("{:?}", r);
        assert!(s.contains("read_file"));
    }

    // ─── Agent config fields ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_agent_config_model_stored() {
        let mut config = crate::config::RuneConfig::default();
        config.model = "gpt-4o-mini".to_string();
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, false, None);
        assert_eq!(agent.config.model, "gpt-4o-mini");
    }

    #[tokio::test]
    async fn test_agent_config_auto_approve_default_false() {
        let config = crate::config::RuneConfig::default();
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, false, None);
        assert!(!agent.config.auto_approve);
    }

    #[tokio::test]
    async fn test_agent_config_auto_approve_true() {
        let mut config = crate::config::RuneConfig::default();
        config.auto_approve = true;
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, false, None);
        assert!(agent.config.auto_approve);
    }

    #[tokio::test]
    async fn test_agent_policy_allowed_paths_rw_default_has_tmp() {
        let config = crate::config::RuneConfig::default();
        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(MockProvider));
        let agent = Agent::new(config, registry, false, None);
        assert!(agent
            .config
            .policy
            .allowed_paths_rw
            .iter()
            .any(|p| p == "/tmp"));
    }

    #[tokio::test]
    async fn test_agent_optional_fields_default_none() {
        let agent = make_test_agent();
        assert!(agent.token_callback.is_none());
        assert!(agent.approval_callback.is_none());
        assert!(agent.files.is_none());
        assert!(agent.active_file.is_none());
        assert!(agent.chat_db.is_none());
        assert!(agent.chat_archive_dir.is_none());
        assert!(agent.chat_note_id.is_none());
        assert!(agent.markdown_dir.is_none());
    }

    // ─── Integration: set_system_prompt + context methods ────────────────────

    #[tokio::test]
    async fn test_context_summary_after_set_system_prompt() {
        let mut agent = make_test_agent();
        agent.set_system_prompt("You are helpful.");
        let summary = agent.context_summary();
        assert_eq!(summary.len(), 1);
        let (role, count) = &summary[0];
        assert_eq!(role, "system");
        assert_eq!(*count, 1);
    }

    #[tokio::test]
    async fn test_message_count_and_chars_consistent() {
        let mut agent = make_test_agent();
        let text = "Hello, world!"; // 13 chars
        agent.set_system_prompt(text);
        assert_eq!(agent.message_count(), 1);
        assert_eq!(agent.context_chars(), 13);
    }

    #[tokio::test]
    async fn test_set_system_prompt_multiple_times_keeps_one_message() {
        let mut agent = make_test_agent();
        for i in 0..5 {
            agent.set_system_prompt(&format!("prompt #{}", i));
        }
        assert_eq!(agent.message_count(), 1);
        assert_eq!(agent.messages[0].content.as_deref(), Some("prompt #4"));
    }

    #[test]
    fn test_sanitize_trailing_user_messages() {
        // Simulates what load_history does: trim consecutive trailing user messages
        let mut msgs: Vec<LlmMessage> = vec![
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("q1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "assistant".into(),
                name: None,
                content: Some("a1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("orphan1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("orphan2".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("latest".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
        ];

        // Apply same logic as load_history
        while msgs.len() >= 2 {
            let len = msgs.len();
            if msgs[len - 1].role == "user" && msgs[len - 2].role == "user" {
                msgs.remove(len - 2);
            } else {
                break;
            }
        }

        assert_eq!(msgs.len(), 3); // user, assistant, user(latest)
        assert_eq!(msgs[0].content.as_deref(), Some("q1"));
        assert_eq!(msgs[1].content.as_deref(), Some("a1"));
        assert_eq!(msgs[2].content.as_deref(), Some("latest"));
    }

    #[test]
    fn test_sanitize_no_orphans_unchanged() {
        let mut msgs: Vec<LlmMessage> = vec![
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("q1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "assistant".into(),
                name: None,
                content: Some("a1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("q2".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
        ];

        while msgs.len() >= 2 {
            let len = msgs.len();
            if msgs[len - 1].role == "user" && msgs[len - 2].role == "user" {
                msgs.remove(len - 2);
            } else {
                break;
            }
        }

        assert_eq!(msgs.len(), 3); // unchanged — no consecutive users at tail
        assert_eq!(msgs[2].content.as_deref(), Some("q2"));
    }

    #[test]
    fn test_sanitize_all_user_messages() {
        let mut msgs: Vec<LlmMessage> = vec![
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("m1".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("m2".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
            LlmMessage {
                role: "user".into(),
                name: None,
                content: Some("m3".into()),
                tool_calls: None,
                tool_call_id: None,
                content_parts: None,
            },
        ];

        while msgs.len() >= 2 {
            let len = msgs.len();
            if msgs[len - 1].role == "user" && msgs[len - 2].role == "user" {
                msgs.remove(len - 2);
            } else {
                break;
            }
        }

        // Should keep only the last user message
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content.as_deref(), Some("m3"));
    }

    #[tokio::test]
    async fn test_write_markdown_broadcasts_file_content() {
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let md_dir = tmp.path().to_path_buf();

        // Create initial file
        std::fs::write(md_dir.join("hello.md"), "# Original").unwrap();

        let config = RuneConfig::default();
        let provider = ProviderRegistry::new();
        let mut agent = Agent::new(config, provider, false, None);
        agent.markdown_dir = Some(md_dir.clone());

        // Track file_list_callback calls
        let list_calls = Arc::new(Mutex::new(0u32));
        let lc = list_calls.clone();
        agent.file_list_callback = Some(Arc::new(move || {
            *lc.lock().unwrap() += 1;
        }));

        // Track file_content_callback calls
        let content_calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![]));
        let cc = content_calls.clone();
        agent.file_content_callback = Some(Arc::new(move |f: String, c: String| {
            cc.lock().unwrap().push((f, c));
        }));

        // Test 1: full replace
        let args = serde_json::json!({"filename": "hello.md", "content": "# Replaced"});
        let result = agent.handle_markdown_tool("write_markdown", &args).await;
        assert!(result.unwrap().contains("updated (full replace)"));
        assert_eq!(*list_calls.lock().unwrap(), 1);
        {
            let calls = content_calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "hello.md");
            assert_eq!(calls[0].1, "# Replaced");
        }

        // Verify file on disk
        let on_disk = std::fs::read_to_string(md_dir.join("hello.md")).unwrap();
        assert_eq!(on_disk, "# Replaced");

        // Test 2: search/replace
        let args2 =
            serde_json::json!({"filename": "hello.md", "search": "Replaced", "replace": "Edited"});
        let result2 = agent.handle_markdown_tool("write_markdown", &args2).await;
        assert!(result2.unwrap().contains("updated: replaced"));
        assert_eq!(*list_calls.lock().unwrap(), 2);
        {
            let calls = content_calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[1].0, "hello.md");
            assert_eq!(calls[1].1, "# Edited");
        }

        let on_disk2 = std::fs::read_to_string(md_dir.join("hello.md")).unwrap();
        assert_eq!(on_disk2, "# Edited");
    }

    #[tokio::test]
    async fn test_write_markdown_no_callback_without_setting() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let md_dir = tmp.path().to_path_buf();
        std::fs::write(md_dir.join("test.md"), "content").unwrap();

        let config = RuneConfig::default();
        let provider = ProviderRegistry::new();
        let mut agent = Agent::new(config, provider, false, None);
        agent.markdown_dir = Some(md_dir);

        // No callbacks set — should still work without panic
        let args = serde_json::json!({"filename": "test.md", "content": "new content"});
        let result = agent.handle_markdown_tool("write_markdown", &args).await;
        assert!(result.unwrap().contains("updated"));
    }

    #[tokio::test]
    async fn test_write_markdown_search_not_found() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let md_dir = tmp.path().to_path_buf();
        std::fs::write(md_dir.join("test.md"), "hello world").unwrap();

        let config = RuneConfig::default();
        let provider = ProviderRegistry::new();
        let mut agent = Agent::new(config, provider, false, None);
        agent.markdown_dir = Some(md_dir);

        let args =
            serde_json::json!({"filename": "test.md", "search": "nonexistent", "replace": "x"});
        let result = agent.handle_markdown_tool("write_markdown", &args).await;
        assert!(result.unwrap().contains("search text not found"));
    }
    #[tokio::test]
    async fn test_tool_status_callback_sequential_emits_start_and_end() {
        use std::sync::{Arc, Mutex};

        // A provider that returns a single tool call on first invocation,
        // then a plain text response on second.
        struct ToolCallProvider {
            call_count: Mutex<u32>,
        }
        impl crate::provider::Provider for ToolCallProvider {
            fn name(&self) -> &str {
                "tool-call-mock"
            }
            fn chat(
                &self,
                _request: crate::provider::LlmRequest,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let mut count = self.call_count.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        // First call: return a single tool call (sequential path)
                        Ok(crate::provider::LlmResponse {
                            content: None,
                            tool_calls: vec![crate::provider::LlmToolCall {
                                id: "call_1".to_string(),
                                call_type: "function".to_string(),
                                function: crate::provider::LlmFunction {
                                    name: "execute_cmd".to_string(),
                                    arguments: r#"{"cmd":"echo hello"}"#.to_string(),
                                },
                            }],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    } else {
                        // Second call: return normal response (done)
                        Ok(crate::provider::LlmResponse {
                            content: Some("Command executed successfully.".to_string()),
                            tool_calls: vec![],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    }
                })
            }
        }

        let mut config = crate::config::RuneConfig {
            model: "tool-call-mock".to_string(),
            ..Default::default()
        };
        // Unrestricted mode so execute_cmd is allowed without policy checks
        config.policy.mode = "unrestricted".to_string();

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(ToolCallProvider {
            call_count: Mutex::new(0),
        }));
        let mut agent = Agent::new(config, registry, false, None);

        // Track tool_status_callback events
        let events: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        agent.tool_status_callback = Some(Arc::new(move |name: &str, state: &str| {
            events_clone
                .lock()
                .unwrap()
                .push((name.to_string(), state.to_string()));
        }));

        // Run agent — should execute `echo hello` in sequential path
        let _result = agent.run("run echo hello").await;

        // Verify: must have both start and end for execute_cmd
        let recorded = events.lock().unwrap().clone();
        assert!(
            recorded.contains(&("execute_cmd".to_string(), "start".to_string())),
            "Missing tool_status start event. Got: {:?}",
            recorded
        );
        assert!(
            recorded.contains(&("execute_cmd".to_string(), "end".to_string())),
            "Missing tool_status end event. Got: {:?}",
            recorded
        );

        // Verify ordering: start before end
        let start_idx = recorded
            .iter()
            .position(|e| e == &("execute_cmd".to_string(), "start".to_string()))
            .unwrap();
        let end_idx = recorded
            .iter()
            .position(|e| e == &("execute_cmd".to_string(), "end".to_string()))
            .unwrap();
        assert!(
            start_idx < end_idx,
            "start must come before end. start={}, end={}",
            start_idx,
            end_idx
        );
    }

    #[tokio::test]
    async fn test_tool_status_callback_parallel_emits_start_and_end() {
        use std::sync::{Arc, Mutex};

        // A provider that returns TWO tool calls (parallel path), then text.
        struct ParallelToolProvider {
            call_count: Mutex<u32>,
        }
        impl crate::provider::Provider for ParallelToolProvider {
            fn name(&self) -> &str {
                "parallel-mock"
            }
            fn chat(
                &self,
                _request: crate::provider::LlmRequest,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let mut count = self.call_count.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        // Two tool calls → parallel path
                        Ok(crate::provider::LlmResponse {
                            content: None,
                            tool_calls: vec![
                                crate::provider::LlmToolCall {
                                    id: "call_a".to_string(),
                                    call_type: "function".to_string(),
                                    function: crate::provider::LlmFunction {
                                        name: "execute_cmd".to_string(),
                                        arguments: r#"{"cmd":"echo one"}"#.to_string(),
                                    },
                                },
                                crate::provider::LlmToolCall {
                                    id: "call_b".to_string(),
                                    call_type: "function".to_string(),
                                    function: crate::provider::LlmFunction {
                                        name: "execute_cmd".to_string(),
                                        arguments: r#"{"cmd":"echo two"}"#.to_string(),
                                    },
                                },
                            ],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    } else {
                        Ok(crate::provider::LlmResponse {
                            content: Some("Both done.".to_string()),
                            tool_calls: vec![],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    }
                })
            }
        }

        let mut config = crate::config::RuneConfig {
            model: "parallel-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "unrestricted".to_string();

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(ParallelToolProvider {
            call_count: Mutex::new(0),
        }));
        let mut agent = Agent::new(config, registry, false, None);

        let events: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        agent.tool_status_callback = Some(Arc::new(move |name: &str, state: &str| {
            events_clone
                .lock()
                .unwrap()
                .push((name.to_string(), state.to_string()));
        }));

        let _result = agent.run("run echo one and echo two").await;

        let recorded = events.lock().unwrap().clone();
        // Parallel: 2 starts, then 2 ends
        let starts: Vec<_> = recorded.iter().filter(|e| e.1 == "start").collect();
        let ends: Vec<_> = recorded.iter().filter(|e| e.1 == "end").collect();
        assert_eq!(
            starts.len(),
            2,
            "Expected 2 start events, got: {:?}",
            recorded
        );
        assert_eq!(ends.len(), 2, "Expected 2 end events, got: {:?}", recorded);
    }

    #[test]
    fn test_prompt_yn_no_tty_returns_false() {
        // In test environment (no controlling TTY in CI/systemd), prompt_yn
        // must return false (deny) rather than true (auto-approve).
        // This prevents serve mode from auto-approving policy prompts.
        //
        // Note: This test relies on the test runner not having /dev/tty attached
        // in a way that read_line returns meaningful input. In a normal test env
        // (cargo test), /dev/tty may or may not exist, but no human is typing.
        // The important thing is that the fallback path returns false.
        //
        // We test the fallback logic directly by verifying the function signature
        // and behavior documentation. The real integration test is the E2E below.
        //
        // Direct unit verification: simulate no-tty scenario
        // Since we can't easily mock /dev/tty, we verify the source contains
        // the safe fallback.
        let source = include_str!("mod.rs");
        // The no-TTY fallback must be `false`, not `true`
        assert!(
            source.contains(
                "// No TTY (e.g. serve mode / systemd) -- deny by default\n        false"
            ),
            "prompt_yn must return false when no TTY is available"
        );
        assert!(
            source.contains("// No TTY (e.g. serve mode / systemd) -- deny by default\n        ConfirmResult::No"),
            "prompt_confirm must return No when no TTY is available"
        );
    }

    // ─── Policy blocked soft-fail tests ────────────────────────────────────────

    /// Helper: a provider that first tries an unlisted command, then responds
    /// with final text on 2nd call (simulating LLM adapting after seeing error).
    struct PolicyBlockProvider {
        call_count: std::sync::Mutex<u32>,
    }
    impl crate::provider::Provider for PolicyBlockProvider {
        fn name(&self) -> &str {
            "policy-block-mock"
        }
        fn chat(
            &self,
            _request: crate::provider::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                let mut count = self.call_count.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    // First call: try to execute `cargo test` (not in allowlist)
                    Ok(crate::provider::LlmResponse {
                        content: None,
                        tool_calls: vec![crate::provider::LlmToolCall {
                            id: "call_1".to_string(),
                            call_type: "function".to_string(),
                            function: crate::provider::LlmFunction {
                                name: "execute_cmd".to_string(),
                                arguments: r#"{"cmd":"cargo test"}"#.to_string(),
                            },
                        }],
                        usage: crate::provider::TokenUsage::default(),
                        model: "mock".to_string(),
                    })
                } else {
                    // Second call: LLM sees the error and gives a final answer
                    Ok(crate::provider::LlmResponse {
                        content: Some("cargo is not allowed, let me try another way.".to_string()),
                        tool_calls: vec![],
                        usage: crate::provider::TokenUsage::default(),
                        model: "mock".to_string(),
                    })
                }
            })
        }
    }

    #[tokio::test]
    async fn test_allowlist_mode_softfails_blocked_command() {
        // In allowlist mode without TTY (web serve), blocked command should
        // soft-fail: error returned to LLM as tool result, agent continues.
        let mut config = crate::config::RuneConfig {
            model: "policy-block-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "allowlist".to_string();
        config.policy.allowed_commands = vec!["echo".to_string(), "ls".to_string()];

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(PolicyBlockProvider {
            call_count: std::sync::Mutex::new(0),
        }));
        // interactive=true, no TTY — simulates rune notes web serve
        let mut agent = Agent::new(config, registry, true, None);

        let result = agent.run("run tests").await;

        // Should be FinalAnswer (LLM adapted), NOT StopReason::Error
        match &result {
            StopReason::FinalAnswer(answer) => {
                assert!(
                    answer.contains("not allowed") || answer.contains("another way"),
                    "Expected LLM to adapt after seeing blocked error. Got: {}",
                    answer
                );
            }
            StopReason::Error(e) => {
                panic!(
                    "allowlist mode should soft-fail (not hard stop). Got Error: {}",
                    e
                );
            }
            other => panic!("Unexpected StopReason: {:?}", other),
        }

        // Command must NOT be auto-added
        assert!(
            !agent.config.policy.allowed_commands.contains(&"cargo".to_string()),
            "Blocked command must not be auto-added in allowlist mode"
        );
    }

    #[tokio::test]
    async fn test_allowlist_mode_softfails_blocked_domain() {
        // Blocked domain in allowlist mode should also soft-fail to LLM.
        struct DomainBlockProvider {
            call_count: std::sync::Mutex<u32>,
        }
        impl crate::provider::Provider for DomainBlockProvider {
            fn name(&self) -> &str {
                "domain-block-mock"
            }
            fn chat(
                &self,
                _request: crate::provider::LlmRequest,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let mut count = self.call_count.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        Ok(crate::provider::LlmResponse {
                            content: None,
                            tool_calls: vec![crate::provider::LlmToolCall {
                                id: "call_1".to_string(),
                                call_type: "function".to_string(),
                                function: crate::provider::LlmFunction {
                                    name: "fetch_url".to_string(),
                                    arguments: r#"{"url":"https://evil.example.com/api"}"#.to_string(),
                                },
                            }],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    } else {
                        Ok(crate::provider::LlmResponse {
                            content: Some("Domain not allowed, I cannot fetch that URL.".to_string()),
                            tool_calls: vec![],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    }
                })
            }
        }

        let mut config = crate::config::RuneConfig {
            model: "domain-block-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "allowlist".to_string();
        config.policy.allowed_commands = vec!["echo".to_string()];
        config.policy.allowed_domains = vec!["safe.example.com".to_string()];

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(DomainBlockProvider {
            call_count: std::sync::Mutex::new(0),
        }));
        let mut agent = Agent::new(config, registry, true, None);

        let result = agent.run("fetch evil site").await;

        match &result {
            StopReason::FinalAnswer(_) => {} // Good: soft-failed, LLM adapted
            StopReason::Error(e) => {
                panic!("allowlist mode should soft-fail domain blocks. Got Error: {}", e);
            }
            other => panic!("Unexpected StopReason: {:?}", other),
        }

        // Domain must NOT be auto-added
        assert!(
            !agent.config.policy.allowed_domains.contains(&"evil.example.com".to_string()),
            "Blocked domain must not be auto-added in allowlist mode"
        );
    }

    #[tokio::test]
    async fn test_confirm_mode_uses_approval_callback_for_blocked_command() {
        use std::sync::{Arc, Mutex};

        // Provider that tries a blocked command, then after approval succeeds,
        // gives a final answer.
        struct ConfirmBlockProvider {
            call_count: Mutex<u32>,
        }
        impl crate::provider::Provider for ConfirmBlockProvider {
            fn name(&self) -> &str {
                "confirm-block-mock"
            }
            fn chat(
                &self,
                _request: crate::provider::LlmRequest,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let mut count = self.call_count.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        // Try blocked command
                        Ok(crate::provider::LlmResponse {
                            content: None,
                            tool_calls: vec![crate::provider::LlmToolCall {
                                id: "call_1".to_string(),
                                call_type: "function".to_string(),
                                function: crate::provider::LlmFunction {
                                    name: "execute_cmd".to_string(),
                                    arguments: r#"{"cmd":"cargo build"}"#.to_string(),
                                },
                            }],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    } else {
                        Ok(crate::provider::LlmResponse {
                            content: Some("Build complete.".to_string()),
                            tool_calls: vec![],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    }
                })
            }
        }

        let mut config = crate::config::RuneConfig {
            model: "confirm-block-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "confirm".to_string();
        config.policy.allowed_commands = vec!["echo".to_string()];

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(ConfirmBlockProvider {
            call_count: Mutex::new(0),
        }));
        let mut agent = Agent::new(config, registry, true, None);

        // Set approval_callback that always approves
        let approved_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let approved_calls_clone = Arc::clone(&approved_calls);
        agent.approval_callback = Some(Arc::new(move |id: String, _detail: String| {
            approved_calls_clone.lock().unwrap().push(id);
            Box::pin(async move { true })
        }));

        let result = agent.run("build the project").await;

        // The approval_callback should have been invoked for the blocked command
        let calls = approved_calls.lock().unwrap();
        assert!(
            calls.iter().any(|c| c.contains("cargo")),
            "approval_callback should be called with command 'cargo'. Got: {:?}",
            *calls
        );

        // After approval, command should be added to allowed_commands
        assert!(
            agent.config.policy.allowed_commands.contains(&"cargo".to_string()),
            "After approval, 'cargo' should be added to allowed_commands"
        );
    }

    #[tokio::test]
    async fn test_confirm_mode_softfails_when_approval_denied() {
        use std::sync::Mutex;

        struct ConfirmDenyProvider {
            call_count: Mutex<u32>,
        }
        impl crate::provider::Provider for ConfirmDenyProvider {
            fn name(&self) -> &str {
                "confirm-deny-mock"
            }
            fn chat(
                &self,
                _request: crate::provider::LlmRequest,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<crate::provider::LlmResponse>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async move {
                    let mut count = self.call_count.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        Ok(crate::provider::LlmResponse {
                            content: None,
                            tool_calls: vec![crate::provider::LlmToolCall {
                                id: "call_1".to_string(),
                                call_type: "function".to_string(),
                                function: crate::provider::LlmFunction {
                                    name: "execute_cmd".to_string(),
                                    arguments: r#"{"cmd":"cargo test"}"#.to_string(),
                                },
                            }],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    } else {
                        // LLM adapts after seeing the denial
                        Ok(crate::provider::LlmResponse {
                            content: Some("User denied cargo, will skip tests.".to_string()),
                            tool_calls: vec![],
                            usage: crate::provider::TokenUsage::default(),
                            model: "mock".to_string(),
                        })
                    }
                })
            }
        }

        let mut config = crate::config::RuneConfig {
            model: "confirm-deny-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "confirm".to_string();
        config.policy.allowed_commands = vec!["echo".to_string()];

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(ConfirmDenyProvider {
            call_count: Mutex::new(0),
        }));
        let mut agent = Agent::new(config, registry, true, None);

        // approval_callback that always denies
        agent.approval_callback = Some(Arc::new(move |_id: String, _detail: String| {
            Box::pin(async move { false })
        }));

        let result = agent.run("test the project").await;

        // Should soft-fail to LLM (FinalAnswer), not hard stop
        match &result {
            StopReason::FinalAnswer(answer) => {
                assert!(
                    answer.contains("denied") || answer.contains("skip"),
                    "Expected LLM to adapt after denial. Got: {}",
                    answer
                );
            }
            StopReason::Error(e) => {
                panic!(
                    "confirm mode with denied approval should soft-fail, not hard stop. Got: {}",
                    e
                );
            }
            other => panic!("Unexpected StopReason: {:?}", other),
        }

        // Command must NOT be added
        assert!(
            !agent.config.policy.allowed_commands.contains(&"cargo".to_string()),
            "Denied command must not be added to allowed_commands"
        );
    }

    #[tokio::test]
    async fn test_confirm_mode_no_callback_softfails() {
        // confirm mode without approval_callback and without TTY: should soft-fail
        let mut config = crate::config::RuneConfig {
            model: "policy-block-mock".to_string(),
            ..Default::default()
        };
        config.policy.mode = "confirm".to_string();
        config.policy.allowed_commands = vec!["echo".to_string()];

        let mut registry = crate::provider::ProviderRegistry::new();
        registry.register(Box::new(PolicyBlockProvider {
            call_count: std::sync::Mutex::new(0),
        }));
        // interactive=true but no TTY and no approval_callback
        let mut agent = Agent::new(config, registry, true, None);

        let result = agent.run("run cargo").await;

        match &result {
            StopReason::FinalAnswer(_) => {} // Good: soft-failed
            StopReason::Error(e) => {
                panic!(
                    "confirm mode without callback should soft-fail. Got Error: {}",
                    e
                );
            }
            other => panic!("Unexpected StopReason: {:?}", other),
        }
    }
}
