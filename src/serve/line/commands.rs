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
    is_admin: bool,
) -> Option<String> {
    let trimmed = text.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "/usage" => {
            if !is_admin {
                Some("⛔ Permission denied: Admin access required.".to_string())
            } else {
                Some(format_usage_command(state).await)
            }
        }
        "/context" => Some(format_context_command(state, note_id).await),
        "/archive" | "/clear" => {
            if !is_admin {
                Some("⛔ Permission denied: Admin access required.".to_string())
            } else {
                Some(format_archive_command(state, note_id).await)
            }
        }
        "/model" => {
            let raw_arg = if parts.len() > 1 {
                parts[1..].join(" ")
            } else {
                String::new()
            };

            if raw_arg.is_empty() {
                Some(format_model_command(state, note_id).await)
            } else if !is_admin {
                Some("⛔ Permission denied: Admin access required.".to_string())
            } else {
                let models = state.models.read().await;
                let global_default = state.global_default_model.read().await;
                let (target_model, requested_thinking) =
                    parse_model_and_thinking(&raw_arg, &parts, &models, &global_default);

                let matched_model = models.iter().find(|m| m.id == target_model);
                if matched_model.is_none()
                    && (!models.is_empty() || target_model != *global_default)
                {
                    let available_str = if !models.is_empty() {
                        format!(
                            "\n\n📋 Allowed Models:\n{}",
                            models
                                .iter()
                                .map(|m| format!("• {}", m.id))
                                .collect::<Vec<_>>()
                                .join("\n")
                        )
                    } else {
                        format!("\n\n📋 Allowed Model:\n• {}", *global_default)
                    };
                    return Some(format!(
                        "⛔ Invalid model '{}'. Please select an allowed model from [notes].{}",
                        target_model, available_str
                    ));
                }

                let effort_list = matched_model
                    .map(|m| m.reasoning_efforts.clone())
                    .unwrap_or_default();
                drop(models);
                drop(global_default);

                // Validate and determine effective thinking level
                let is_auto = target_model.starts_with("openrouter/auto");
                let effective_thinking = if let Some(ref t) = requested_thinking {
                    let t_norm = if t.eq_ignore_ascii_case("none") {
                        "off".to_string()
                    } else {
                        t.to_lowercase()
                    };

                    let is_valid = if is_auto {
                        matches!(
                            t_norm.as_str(),
                            "off" | "low" | "medium" | "high" | "xhigh" | "max"
                        )
                    } else {
                        t_norm == "off"
                            || effort_list.iter().any(|e| e.eq_ignore_ascii_case(&t_norm))
                    };

                    if !is_valid {
                        let supported_desc = if is_auto {
                            "• off\n• low\n• medium\n• high\n• xhigh\n• max".to_string()
                        } else if !effort_list.is_empty() {
                            let mut list = vec!["• off".to_string()];
                            list.extend(effort_list.iter().map(|e| format!("• {}", e)));
                            list.join("\n")
                        } else {
                            "• off (model does not support thinking/reasoning)".to_string()
                        };
                        return Some(format!(
                            "⛔ Invalid thinking level '{}' for model '{}'.\n\n📋 Supported levels:\n{}",
                            t, target_model, supported_desc
                        ));
                    }
                    t_norm
                } else {
                    // Auto-adapt thinking if not explicitly specified
                    let current_effective = state.effective_thinking(note_id).await;
                    if let Some(ref t) = current_effective {
                        if t == "off" && is_auto {
                            "low".to_string()
                        } else if t == "off" || effort_list.contains(t) {
                            t.clone()
                        } else if is_auto {
                            "low".to_string()
                        } else {
                            "off".to_string()
                        }
                    } else if is_auto {
                        "low".to_string()
                    } else {
                        "off".to_string()
                    }
                };

                let room = state.get_or_create_room(note_id).await;
                *room.model_override.write().await = Some(target_model.clone());
                let _ = state.chat_db.set_note_model(note_id, Some(&target_model));
                *room.thinking_override.write().await = Some(effective_thinking.clone());
                let _ = state
                    .chat_db
                    .set_note_thinking(note_id, Some(&effective_thinking));

                let usage = state.provider_registry.read().await.usage();
                broadcast_to_room(
                    &room,
                    &SseMsg::ModelChanged {
                        model: target_model.clone(),
                        thinking: effective_thinking.clone(),
                        usage,
                    },
                );
                Some(format!(
                    "🧠 Switched model for [{}] to [{}] (thinking: {})",
                    note_id, target_model, effective_thinking
                ))
            }
        }
        "/help" => Some(format_help_command(is_admin)),
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

    let (_prompt, active_personas) =
        crate::serve::api::build_effective_note_system_prompt(state, note_id).await;
    let persona_suffix = if !active_personas.is_empty() {
        format!("\n🎭 Persona: {}", active_personas.join(", "))
    } else {
        String::new()
    };

    format!(
        "📊 {:.1}% context used · {:.1}k / {:.1}k (model: {}){}",
        pct, est_k, win_k, active_model, persona_suffix
    )
}

/// Format and execute the `/archive` command.
pub async fn format_archive_command(state: &ServerState, note_id: &str) -> String {
    let archive_dir = state
        .note_markdown_dir(note_id)
        .parent()
        .unwrap()
        .join("archives");
    let _ = tokio::fs::create_dir_all(&archive_dir).await;
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
    let models = state.models.read().await;
    let mut out = format!("🧠 Current Model: {} (thinking: {})", model, thinking);
    if !models.is_empty() {
        out.push_str("\n\n📋 Allowed Models:");
        for m in models.iter() {
            let active_marker = if m.id == model { " (active)" } else { "" };
            out.push_str(&format!("\n• {}{}", m.id, active_marker));
        }
    }
    out
}

/// Format the `/help` command output based on user role.
pub fn format_help_command(is_admin: bool) -> String {
    let mut out = String::from("💡 Rune LINE Commands:\n");
    if is_admin {
        out.push_str("• /usage — View LLM Provider usage and remaining credits/quota\n");
        out.push_str("• /archive — Archive and reset chat history for current notebook\n");
        out.push_str("• /clear — Alias for /archive\n");
        out.push_str("• /model <model_name> [thinking,cost] — Switch LLM model and optional thinking/cost tier\n");
    }
    out.push_str("• /context — Check conversation context token usage and limits\n");
    out.push_str("• /model — View current AI model and thinking configuration\n");
    out.push_str("• /help — Show this help message");
    out
}

/// Parses the model name and optional thinking/cost level from user input.
///
/// Supported syntax examples:
/// - `/model openrouter/auto high` -> (`openrouter/auto`, `Some("high")`)
/// - `/model openrouter/auto/high` -> (`openrouter/auto`, `Some("high")`)
/// - `/model openrouter/auto:high` -> (`openrouter/auto`, `Some("high")`)
/// - `/model deepseek/deepseek-chat` -> (`deepseek/deepseek-chat`, `None`)
/// - `/model deepseek/deepseek-chat off` -> (`deepseek/deepseek-chat`, `Some("off")`)
pub fn parse_model_and_thinking(
    arg_full: &str,
    parts: &[&str],
    models: &[crate::serve::ModelInfo],
    global_default: &str,
) -> (String, Option<String>) {
    // Case 1: Multiple arguments like "/model openrouter/auto high"
    if parts.len() >= 3 {
        let model_part = parts[1].trim();
        let thinking_part = parts[2..].join(" ").trim().to_string();
        return (
            model_part.to_string(),
            if thinking_part.is_empty() {
                None
            } else {
                Some(thinking_part)
            },
        );
    }

    let raw = arg_full.trim();

    // Case 2: Exact match in models (e.g. "openrouter/auto")
    if models.iter().any(|m| m.id == raw) || (models.is_empty() && raw == global_default) {
        return (raw.to_string(), None);
    }

    // Case 3: Suffix with ':' (e.g. "openrouter/auto:high")
    if let Some((m, t)) = raw.rsplit_once(':') {
        let m = m.trim();
        let t = t.trim();
        if !m.is_empty() && !t.is_empty() {
            return (m.to_string(), Some(t.to_string()));
        }
    }

    // Case 4: Suffix with '/' (e.g. "openrouter/auto/high" or "deepseek/deepseek-chat/off")
    if let Some((m, t)) = raw.rsplit_once('/') {
        let m = m.trim();
        let t = t.trim();
        if !m.is_empty()
            && !t.is_empty()
            && (models.iter().any(|model| model.id == m)
                || m == global_default
                || is_known_thinking_level(t))
        {
            return (m.to_string(), Some(t.to_string()));
        }
    }

    (raw.to_string(), None)
}

fn is_known_thinking_level(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "off" | "none" | "low" | "medium" | "high" | "xhigh" | "max" | "minimal"
    )
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
            running_cron_jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
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
    fn test_format_help_command_admin_vs_user() {
        let help_admin = format_help_command(true);
        assert!(help_admin.contains("/usage"));
        assert!(help_admin.contains("/archive"));
        assert!(help_admin.contains("/model <model_name> [thinking,cost]"));
        assert!(help_admin.contains("/context"));

        let help_user = format_help_command(false);
        assert!(!help_user.contains("/usage"));
        assert!(!help_user.contains("/archive"));
        assert!(help_user.contains("/context"));
        assert!(help_user.contains("/model"));
    }

    #[tokio::test]
    async fn test_slash_command_model_query_and_switch() {
        let state = create_test_state();
        state.chat_db.create_note("AI", "AI Notes", None).unwrap();

        // Populate allowed models list (e.g. from [notes].model)
        *state.models.write().await = vec![
            crate::serve::ModelInfo {
                id: "test-model".to_string(),
                provider: None,
                context_window: Some(128000),
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            },
            crate::serve::ModelInfo {
                id: "gpt-5".to_string(),
                provider: None,
                context_window: Some(200000),
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            },
        ];

        // Query model (allowed for user)
        let reply = handle_slash_command("/model", &state, "AI", "U1234", false).await;
        assert!(reply.is_some());
        let reply_str = reply.unwrap();
        assert!(reply_str.contains("test-model"));
        assert!(reply_str.contains("Allowed Models:"));
        assert!(reply_str.contains("• gpt-5"));

        // Switch model as user (forbidden)
        let reply_user_switch =
            handle_slash_command("/model gpt-5", &state, "AI", "U1234", false).await;
        assert_eq!(
            reply_user_switch,
            Some("⛔ Permission denied: Admin access required.".to_string())
        );

        // Switch to invalid/unauthorized model as admin (forbidden)
        let reply_invalid_switch =
            handle_slash_command("/model unauthorized-model-xyz", &state, "AI", "U1234", true)
                .await;
        assert!(reply_invalid_switch.is_some());
        let invalid_msg = reply_invalid_switch.unwrap();
        assert!(invalid_msg.contains("⛔ Invalid model 'unauthorized-model-xyz'"));
        assert!(invalid_msg.contains("• test-model"));
        assert!(invalid_msg.contains("• gpt-5"));

        // Switch to valid allowed model as admin (allowed)
        let reply_admin_switch =
            handle_slash_command("/model gpt-5", &state, "AI", "U1234", true).await;
        assert!(reply_admin_switch.is_some());
        assert!(reply_admin_switch.unwrap().contains("Switched model"));
        assert_eq!(
            state.chat_db.get_note_model("AI"),
            Some("gpt-5".to_string())
        );
    }

    #[tokio::test]
    async fn test_slash_command_model_with_thinking_tier() {
        let state = create_test_state();
        state.chat_db.create_note("AI", "AI Notes", None).unwrap();

        *state.models.write().await = vec![
            crate::serve::ModelInfo {
                id: "openrouter/auto".to_string(),
                provider: None,
                context_window: Some(128000),
                reasoning_efforts: vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                    "xhigh".to_string(),
                    "max".to_string(),
                ],
                supported_endpoints: vec![],
            },
            crate::serve::ModelInfo {
                id: "deepseek/deepseek-chat".to_string(),
                provider: None,
                context_window: Some(64000),
                reasoning_efforts: vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                ],
                supported_endpoints: vec![],
            },
            crate::serve::ModelInfo {
                id: "simple-model".to_string(),
                provider: None,
                context_window: Some(32000),
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            },
        ];

        // 1. Switch using slash separator: /model openrouter/auto/high
        let reply =
            handle_slash_command("/model openrouter/auto/high", &state, "AI", "U1234", true).await;
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert!(msg.contains("Switched model for [AI] to [openrouter/auto] (thinking: high)"));
        assert_eq!(
            state.chat_db.get_note_model("AI"),
            Some("openrouter/auto".to_string())
        );
        assert_eq!(
            state.chat_db.get_note_thinking("AI"),
            Some("high".to_string())
        );

        // 2. Switch using colon separator: /model deepseek/deepseek-chat:medium
        let reply_colon = handle_slash_command(
            "/model deepseek/deepseek-chat:medium",
            &state,
            "AI",
            "U1234",
            true,
        )
        .await;
        assert!(reply_colon.is_some());
        let msg_colon = reply_colon.unwrap();
        assert!(msg_colon
            .contains("Switched model for [AI] to [deepseek/deepseek-chat] (thinking: medium)"));
        assert_eq!(
            state.chat_db.get_note_model("AI"),
            Some("deepseek/deepseek-chat".to_string())
        );
        assert_eq!(
            state.chat_db.get_note_thinking("AI"),
            Some("medium".to_string())
        );

        // 3. Switch using space separator: /model deepseek/deepseek-chat off
        let reply_space = handle_slash_command(
            "/model deepseek/deepseek-chat off",
            &state,
            "AI",
            "U1234",
            true,
        )
        .await;
        assert!(reply_space.is_some());
        let msg_space = reply_space.unwrap();
        assert!(msg_space
            .contains("Switched model for [AI] to [deepseek/deepseek-chat] (thinking: off)"));
        assert_eq!(
            state.chat_db.get_note_thinking("AI"),
            Some("off".to_string())
        );

        // 4. Invalid thinking level for openrouter/auto
        let reply_invalid_tier = handle_slash_command(
            "/model openrouter/auto/super_ultra",
            &state,
            "AI",
            "U1234",
            true,
        )
        .await;
        assert!(reply_invalid_tier.is_some());
        let msg_invalid = reply_invalid_tier.unwrap();
        assert!(msg_invalid.contains("⛔ Invalid thinking level 'super_ultra'"));
        assert!(msg_invalid.contains("• high"));

        // 5. Model without reasoning support reject non-off thinking
        let reply_no_reason =
            handle_slash_command("/model simple-model/high", &state, "AI", "U1234", true).await;
        assert!(reply_no_reason.is_some());
        let msg_no_reason = reply_no_reason.unwrap();
        assert!(msg_no_reason.contains("⛔ Invalid thinking level 'high'"));
        assert!(msg_no_reason.contains("does not support thinking"));
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

        let reply = handle_slash_command("/context", &state, "AI", "U1234", false).await;
        assert!(reply.is_some());
        let msg = reply.unwrap();
        assert!(msg.contains("context used"));
    }

    #[tokio::test]
    async fn test_slash_command_archive_admin_only() {
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

        // User forbidden
        let reply_user = handle_slash_command("/archive", &state, "AI", "U1234", false).await;
        assert_eq!(
            reply_user,
            Some("⛔ Permission denied: Admin access required.".to_string())
        );

        // Admin allowed
        let reply_admin = handle_slash_command("/archive", &state, "AI", "U1234", true).await;
        assert!(reply_admin.is_some());
        let msg = reply_admin.unwrap();
        assert!(msg.contains("Chat history archived"));
    }

    #[tokio::test]
    async fn test_slash_command_unknown() {
        let state = create_test_state();
        let reply = handle_slash_command("/unknown_command", &state, "AI", "U1234", true).await;
        assert_eq!(reply, None);
    }
}
