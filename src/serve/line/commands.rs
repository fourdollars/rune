use crate::serve::api::{broadcast_to_room, SseMsg};
use crate::serve::ServerState;

/// Checks if input text is a slash command.
pub fn is_slash_command(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('/')
}

/// Executes a slash command and returns the reply message string if recognized.
pub async fn handle_slash_command(
    text: &str,
    state: &ServerState,
    note_id: &str,
    _user_id: &str,
) -> Option<String> {
    let trimmed = text.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "/usage" => Some(format_usage_command(state).await),
        "/context" => Some(format_context_command(state, note_id).await),
        "/archive" | "/clear" => Some(format_archive_command(state, note_id).await),
        "/model" => Some(format_model_command(state, note_id).await),
        "/notes" => Some(format_notes_command(state).await),
        "/note" => {
            if arg.is_empty() {
                Some(format!("📁 Active Note: [{}]", note_id))
            } else {
                Some(format!("📁 Switched active note to [{}]", arg))
            }
        }
        "/help" => Some(format_help_command()),
        _ => None,
    }
}

/// Format the `/usage` command output based on ProviderUsageStats.
pub async fn format_usage_command(state: &ServerState) -> String {
    let usage_opt = state.provider_registry.read().await.usage();
    match usage_opt {
        Some(usage) => {
            let mut out = format!("💳 Provider: {} ({})\n", usage.plan_name, usage.provider);

            // OpenRouter / Credits info from details
            if let Some(ref details) = usage.details {
                if let Some(credits) = details.get("total_credits").and_then(|v| v.as_f64()) {
                    let used = details
                        .get("total_usage")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let remaining = (credits - used).max(0.0);
                    out.push_str(&format!(
                        "• Credits: ${:.2} remaining (Total: ${:.2})\n",
                        remaining, credits
                    ));
                } else if let Some(limit) = details.get("limit").and_then(|v| v.as_f64()) {
                    let remaining = details
                        .get("limit_remaining")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(limit);
                    let used = (limit - remaining).max(0.0);
                    let pct = if limit > 0.0 {
                        (used / limit) * 100.0
                    } else {
                        0.0
                    };
                    out.push_str(&format!(
                        "• Key Limit: ${:.2} (Used: ${:.2}, {:.1}%)\n",
                        limit, used, pct
                    ));
                }

                if let Some(budget) = details.get("monthly_budget").and_then(|v| v.as_f64()) {
                    let used = details
                        .get("usage_monthly")
                        .or_else(|| details.get("usage"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let pct = if budget > 0.0 {
                        (used / budget) * 100.0
                    } else {
                        0.0
                    };
                    out.push_str(&format!(
                        "• Monthly Budget: ${:.2} (Used: ${:.2}, {:.1}%)\n",
                        budget, used, pct
                    ));
                }
            }

            // GitHub Copilot Quota info
            if let Some(pct) = usage.quota_percent_remaining {
                if let (Some(rem), Some(ent)) = (usage.quota_remaining, usage.quota_entitlement) {
                    out.push_str(&format!(
                        "• Quota Remaining: {:.1}% ({} / {} units)\n",
                        pct, rem, ent
                    ));
                } else {
                    out.push_str(&format!("• Quota Remaining: {:.1}%\n", pct));
                }
            }

            out.push_str(&format!(
                "• Session Usage: {} tokens ({} requests)",
                usage.session_tokens, usage.session_requests
            ));
            out
        }
        None => "💳 No provider usage statistics available for current session.".to_string(),
    }
}

/// Format the `/context` command output.
pub async fn format_context_command(state: &ServerState, note_id: &str) -> String {
    let history = state
        .chat_db
        .load_recent_async(note_id.to_string(), 20)
        .await;
    let mut total_chars = 0;
    for rec in &history {
        total_chars += rec.content.len();
    }
    // Approximate 1 token ~= 4 characters / 1.5 chars for CJK
    let estimated_tokens = total_chars / 3;

    let active_model = state.effective_model(note_id).await;
    let context_window = {
        let models = state.models.read().await;
        models
            .iter()
            .find(|m| m.id == active_model)
            .and_then(|m| m.context_window)
            .unwrap_or(state.config.context_window as u64)
    };

    let pct = if context_window > 0 {
        ((estimated_tokens as f64) / (context_window as f64) * 100.0).min(100.0)
    } else {
        0.0
    };

    let est_k = (estimated_tokens as f64) / 1000.0;
    let win_k = (context_window as f64) / 1000.0;

    format!(
        "📊 {:.1}% context used · {:.1}k / {:.1}k (model: {})",
        pct, est_k, win_k, active_model
    )
}

/// Format and execute the `/archive` command.
pub async fn format_archive_command(state: &ServerState, note_id: &str) -> String {
    let archive_dir = state
        .note_markdown_dir(note_id)
        .parent()
        .unwrap()
        .join("archives");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("{}.jsonl", ts);
    let archive_path = archive_dir.join(&filename);

    let db = state.chat_db.clone();
    match db.archive_async(note_id.to_string(), archive_path).await {
        Ok(count) => {
            let room = state.get_or_create_room(note_id).await;
            let msg = SseMsg::ArchiveDone {
                filename: filename.clone(),
                count,
            };
            broadcast_to_room(&room, &msg);
            let hist = SseMsg::History { messages: vec![] };
            broadcast_to_room(&room, &hist);
            format!(
                "📦 Chat history archived ({} messages saved to archives/{})",
                count, filename
            )
        }
        Err(e) => format!("⚠️ Archive failed: {}", e),
    }
}

/// Format the `/model` command output.
pub async fn format_model_command(state: &ServerState, note_id: &str) -> String {
    let model = state.effective_model(note_id).await;
    let thinking = state
        .effective_thinking(note_id)
        .await
        .unwrap_or_else(|| "default".to_string());
    format!("🧠 Current Model: {} (thinking: {})", model, thinking)
}

/// Format the `/notes` command output.
pub async fn format_notes_command(state: &ServerState) -> String {
    let notes = state.chat_db.list_notes().unwrap_or_default();
    if notes.is_empty() {
        return "📚 No notebooks available.".to_string();
    }
    let mut out = String::from("📚 Available Notes:\n");
    for note in notes {
        out.push_str(&format!("• {}\n", note.name));
    }
    out.trim_end().to_string()
}

/// Format the `/help` command output.
pub fn format_help_command() -> String {
    r#"💡 Rune LINE Commands:
• /usage — View LLM Provider usage and remaining credits/quota
• /context — Check conversation context token usage and limits
• /archive — Archive and reset chat history for current notebook
• /note <name> — Switch or view current active notebook
• /notes — List all available notebooks
• /model — View current AI model and thinking configuration
• /help — Show this help message"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuneConfig;
    use crate::serve::db::ChatDb;
    use crate::serve::oauth;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};

    fn create_test_state() -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");
        let mut config = RuneConfig::default();
        config.model = "test-model".to_string();

        ServerState {
            config,
            sessions: oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(RwLock::new(vec![])),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new("test-model".to_string())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: std::path::PathBuf::from("/tmp/rune-test-line-cmds"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(RwLock::new(HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        }
    }

    #[test]
    fn test_is_slash_command() {
        assert!(is_slash_command("/usage"));
        assert!(is_slash_command("  /context  "));
        assert!(!is_slash_command("hello /world"));
        assert!(!is_slash_command(""));
    }

    #[test]
    fn test_format_help_command() {
        let help = format_help_command();
        assert!(help.contains("/usage"));
        assert!(help.contains("/context"));
        assert!(help.contains("/archive"));
        assert!(help.contains("/model"));
        assert!(help.contains("/note"));
        assert!(help.contains("/notes"));
    }

    #[tokio::test]
    async fn test_slash_command_model_and_note() {
        let state = create_test_state();
        let reply = handle_slash_command("/model", &state, "AI", "U1234").await;
        assert!(reply.is_some());
        assert!(reply.unwrap().contains("test-model"));

        let reply_note = handle_slash_command("/note Rust", &state, "AI", "U1234").await;
        assert_eq!(
            reply_note,
            Some("📁 Switched active note to [Rust]".to_string())
        );

        let reply_note_current = handle_slash_command("/note", &state, "AI", "U1234").await;
        assert_eq!(reply_note_current, Some("📁 Active Note: [AI]".to_string()));
    }

    #[tokio::test]
    async fn test_slash_command_notes_list() {
        let state = create_test_state();
        state.chat_db.create_note("AI", "AI Notes", None).unwrap();
        state
            .chat_db
            .create_note("Rust", "Rust Notes", None)
            .unwrap();

        let reply = handle_slash_command("/notes", &state, "AI", "U1234").await;
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert!(msg.contains("Available Notes"));
        assert!(msg.contains("AI Notes"));
        assert!(msg.contains("Rust Notes"));
    }

    #[tokio::test]
    async fn test_slash_command_context() {
        let state = create_test_state();
        state
            .chat_db
            .insert_async(
                "AI".to_string(),
                "user".to_string(),
                "Alice".to_string(),
                "Hello AI assistant".to_string(),
            )
            .await;

        let reply = handle_slash_command("/context", &state, "AI", "U1234").await;
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert!(msg.contains("context used"));
    }

    #[tokio::test]
    async fn test_slash_command_archive() {
        let state = create_test_state();
        let _ = std::fs::create_dir_all(state.note_markdown_dir("AI"));
        state
            .chat_db
            .insert_async(
                "AI".to_string(),
                "user".to_string(),
                "Alice".to_string(),
                "Msg 1".to_string(),
            )
            .await;

        let reply = handle_slash_command("/archive", &state, "AI", "U1234").await;
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert!(msg.contains("Chat history archived"));
    }

    #[tokio::test]
    async fn test_slash_command_unknown() {
        let state = create_test_state();
        let reply = handle_slash_command("/unknown_command", &state, "AI", "U1234").await;
        assert_eq!(reply, None);
    }
}
