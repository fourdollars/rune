use super::cache::ProfileCache;
use super::client::LineClient;
use super::commands::{handle_slash_command, is_slash_command};
use super::signature::verify_signature;
use super::types::WebhookPayload;
use crate::agent::{Agent, StopReason};
use crate::serve::api::{
    broadcast_file_list, broadcast_to_room, build_embedding, build_provider, build_system_prompt,
    SseMsg,
};
use crate::serve::ServerState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Global or lazily-initialized ProfileCache for LINE user display names.
static PROFILE_CACHE: std::sync::OnceLock<ProfileCache> = std::sync::OnceLock::new();

fn get_profile_cache() -> &'static ProfileCache {
    PROFILE_CACHE.get_or_init(ProfileCache::default)
}

/// HTTP handler for `POST /webhook/line`.
pub async fn line_webhook_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> Response {
    let line_cfg = match state.config.notes.line {
        Some(ref cfg) if cfg.enabled || !cfg.channel_secret.is_empty() => cfg,
        _ => {
            warn!("LINE Webhook received but [notes.line] is not configured/enabled");
            return (StatusCode::BAD_REQUEST, "LINE webhook not configured").into_response();
        }
    };

    // Extract `x-line-signature` header
    let signature = match headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(sig) => sig,
        None => {
            warn!("LINE Webhook missing x-line-signature header");
            return (StatusCode::UNAUTHORIZED, "Missing x-line-signature header").into_response();
        }
    };

    // Verify HMAC-SHA256 signature
    if !verify_signature(&line_cfg.channel_secret, &raw_body, signature) {
        warn!("LINE Webhook HMAC-SHA256 signature verification failed");
        return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
    }

    // Parse payload
    let payload: WebhookPayload = match serde_json::from_slice(&raw_body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse LINE Webhook JSON payload: {}", e);
            return (StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)).into_response();
        }
    };

    info!(
        "Received LINE Webhook payload with {} events",
        payload.events.len()
    );

    // Process events asynchronously
    let state_clone = state.clone();
    let cache = get_profile_cache().clone();
    tokio::spawn(async move {
        process_webhook_payload(state_clone, payload, cache).await;
    });

    // Acknowledge LINE platform immediately
    (StatusCode::OK, "OK").into_response()
}

/// Background processor for LINE Webhook events.
pub async fn process_webhook_payload(
    state: ServerState,
    payload: WebhookPayload,
    cache: ProfileCache,
) {
    let line_cfg = match state.config.notes.line {
        Some(ref cfg) => cfg.clone(),
        None => return,
    };

    let client = LineClient::new(line_cfg.channel_access_token.clone());

    for event in payload.events {
        if event.event_type != "message" {
            debug!("Ignoring non-message LINE event: {}", event.event_type);
            continue;
        }

        let message = match event.message {
            Some(ref msg) if msg.message_type == "text" => msg,
            _ => {
                debug!("Ignoring non-text LINE message event");
                continue;
            }
        };

        let text = match message.text {
            Some(ref t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => continue,
        };

        let user_id = event
            .source
            .as_ref()
            .and_then(|s| s.user_id.as_deref())
            .unwrap_or("unknown");

        // Strict User Authorization (Zero-Trust Allowlist)
        let user_cfg = match line_cfg.users.iter().find(|u| u.user_id == user_id) {
            Some(cfg) => cfg,
            None => {
                warn!("Unauthorized LINE message from user_id: {}", user_id);
                if let Some(ref reply_token) = event.reply_token {
                    let deny_msg = format!(
                        "⛔ Access Denied: Unauthorized LINE User ID.\n\nYour User ID: {}\nPlease contact the administrator to add your User ID to [[notes.line.users]] in ~/.rune/rune.toml to enable access.",
                        user_id
                    );
                    let _ = client.reply_message(reply_token, &deny_msg).await;
                }
                continue;
            }
        };

        let is_guest = user_cfg.role.to_lowercase() == "guest";
        let note_id = user_cfg
            .note
            .clone()
            .or_else(|| line_cfg.default_note.clone())
            .unwrap_or_else(|| "Default".to_string());

        // Guests cannot execute interactive AI chat (read-only / log collector)
        let interactive_chat = if is_guest {
            false
        } else {
            user_cfg.interactive_chat
        };

        // Ensure notebook exists in DB
        let _ = state.chat_db.create_note(&note_id, &note_id, None);

        // Resolve display name via Cache / Profile API
        let display_name = match cache.get(user_id).await {
            Some(name) => Some(name),
            None => {
                if user_id != "unknown" && !line_cfg.channel_access_token.is_empty() {
                    match client.get_profile(user_id).await {
                        Ok(profile) => {
                            cache
                                .insert(user_id.to_string(), profile.display_name.clone())
                                .await;
                            Some(profile.display_name)
                        }
                        Err(e) => {
                            warn!("Failed to fetch profile for LINE user {}: {}", user_id, e);
                            None
                        }
                    }
                } else {
                    None
                }
            }
        };

        let nickname = ProfileCache::format_nickname(display_name.as_deref(), user_id);

        // Check if slash command
        if is_slash_command(&text) {
            if is_guest && (text.starts_with("/archive") || text.starts_with("/clear")) {
                if let Some(ref reply_token) = event.reply_token {
                    let _ = client
                        .reply_message(
                            reply_token,
                            "⛔ Guest access is read-only. Cannot archive notebooks.",
                        )
                        .await;
                }
                continue;
            }

            if let Some(reply_text) = handle_slash_command(&text, &state, &note_id, user_id).await {
                if let Some(ref reply_token) = event.reply_token {
                    if let Err(e) = client.reply_message(reply_token, &reply_text).await {
                        error!("Failed to reply to LINE slash command: {}", e);
                    }
                }
            }
            continue;
        }

        // Interactive AI Chat
        if interactive_chat {
            let reply_token = event.reply_token.clone();
            let client_clone = client.clone();
            let state_clone = state.clone();
            let note_id_clone = note_id.clone();
            let nickname_clone = nickname.clone();
            let text_clone = text.clone();
            let user_id_clone = user_id.to_string();

            tokio::spawn(async move {
                // Show loading animation in LINE chat
                if user_id_clone != "unknown" {
                    let _ = client_clone
                        .start_loading_animation(&user_id_clone, Some(60))
                        .await;
                }

                // Broadcast and persist user message
                let room = state_clone.get_or_create_room(&note_id_clone).await;
                let user_msg = SseMsg::ChatMessage {
                    nickname: nickname_clone.clone(),
                    content: text_clone.clone(),
                };
                broadcast_to_room(&room, &user_msg);

                state_clone
                    .chat_db
                    .insert_async(
                        note_id_clone.clone(),
                        "user".to_string(),
                        nickname_clone.clone(),
                        text_clone.clone(),
                    )
                    .await;

                // Send thinking status to room
                let thinking = SseMsg::Status {
                    state: "thinking".to_string(),
                };
                broadcast_to_room(&room, &thinking);

                // Run Agent & reply
                execute_line_agent_and_reply(
                    state_clone,
                    note_id_clone,
                    nickname_clone,
                    text_clone,
                    reply_token,
                    client_clone,
                )
                .await;
            });
        } else {
            // Lint Bot / CI logger mode: persist to Chat DB and save/append to Notebook Markdown file
            info!(
                "LINE Lint Bot message received for note [{}]: {}",
                note_id, text
            );
            state
                .chat_db
                .insert_async(
                    note_id.clone(),
                    "user".to_string(),
                    nickname.clone(),
                    text.clone(),
                )
                .await;

            let room = state.get_or_create_room(&note_id).await;
            let user_msg = SseMsg::ChatMessage {
                nickname: nickname.clone(),
                content: text.clone(),
            };
            broadcast_to_room(&room, &user_msg);

            // Append to lint markdown file
            let md_dir = state.note_markdown_dir(&note_id);
            let _ = std::fs::create_dir_all(&md_dir);
            let today = chrono_now_date();
            let filename = format!("{}-lint-report.md", today);
            let file_path = md_dir.join(&filename);

            let append_text = format!(
                "\n\n### Report from {} ({})\n\n```\n{}\n```\n",
                nickname,
                chrono_now_time(),
                text
            );

            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
            {
                let _ = file.write_all(append_text.as_bytes());
            }

            broadcast_file_list(&state, &note_id).await;
        }
    }
}

/// Executes Rune Agent for LINE message and replies via LINE Messaging API.
async fn execute_line_agent_and_reply(
    state: ServerState,
    note_id: String,
    nickname: String,
    user_msg: String,
    reply_token: Option<String>,
    line_client: LineClient,
) {
    let config = state.config.clone();
    let active_model = state.effective_model(&note_id).await;
    let room = state.get_or_create_room(&note_id).await;

    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let err_msg = format!("⚠️ Provider error: {}", e);
            if let Some(token) = reply_token {
                let _ = line_client.reply_message(&token, &err_msg).await;
            }
            return;
        }
    };

    let embedding = build_embedding(&config).await;

    // Build agent
    let mut cfg = config.clone();
    cfg.model = active_model.clone();
    let effective_thinking_level = state.effective_thinking(&note_id).await;
    cfg.thinking = effective_thinking_level.clone();

    {
        let models = state.models.read().await;
        if let Some(model_info) = models.iter().find(|m| m.id == active_model) {
            if let Some(cw) = model_info.context_window {
                cfg.context_window = cw as usize;
            }
        }
    }

    let mut agent = Agent::new(cfg, provider, true, embedding);
    agent.set_serve_mode(true);
    agent.set_agent_skills(config.notes.agent_skills);

    agent.user_name = Some(nickname.clone());
    agent.markdown_dir = Some(state.note_markdown_dir(&note_id));
    agent.chat_db = Some(state.chat_db.clone());
    agent.chat_note_id = Some(note_id.clone());
    agent.chat_archive_dir = Some(
        state
            .note_markdown_dir(&note_id)
            .parent()
            .unwrap()
            .join("archives"),
    );

    // Set system prompt
    let system_prompt = {
        let room_prompt = room.system_prompt.read().await;
        if let Some(ref p) = *room_prompt {
            p.clone()
        } else {
            build_system_prompt(&config).await
        }
    };
    agent.set_system_prompt(&system_prompt);

    // Load recent history (up to 20 records)
    let history = state.chat_db.load_recent_async(note_id.clone(), 20).await;
    let history_without_current: Vec<_> = history
        .into_iter()
        .filter(|r| !(r.role == "user" && r.content == user_msg))
        .collect();
    agent.load_history(&history_without_current);

    // Run agent
    let stop_reason = agent.run(&user_msg).await;

    let done = SseMsg::ChatDone {};
    broadcast_to_room(&room, &done);

    // Run statistics line
    let total_tokens = agent.tokens_in() + agent.tokens_out();
    let stats_line = format!(
        "⚡ {} steps · {} tokens · {} tool calls",
        agent.step_count(),
        total_tokens,
        agent.tool_call_count()
    );

    // Format final reply and extract raw answer for DB
    let (raw_answer, line_reply_text) = match &stop_reason {
        StopReason::FinalAnswer(ans) => (
            ans.clone(),
            format!("{}\n\n──────────────\n{}", ans, stats_line),
        ),
        StopReason::Error(e) => (
            format!("⚠️ Agent error: {}", e),
            format!("⚠️ Agent error: {}\n\n──────────────\n{}", e, stats_line),
        ),
        StopReason::MaxSteps => (
            "⚠️ Agent reached max steps".to_string(),
            format!(
                "⚠️ Agent reached max steps\n\n──────────────\n{}",
                stats_line
            ),
        ),
        StopReason::TokenBudgetExhausted => (
            "⚠️ Token budget exhausted".to_string(),
            format!(
                "⚠️ Token budget exhausted\n\n──────────────\n{}",
                stats_line
            ),
        ),
        StopReason::UserInterrupt => (
            "⚠️ Interrupted".to_string(),
            format!("⚠️ Interrupted\n\n──────────────\n{}", stats_line),
        ),
    };

    // Save assistant message to Chat DB
    let meta_model = active_model.clone();
    let meta_thinking = effective_thinking_level.clone().filter(|t| t != "off");
    state
        .chat_db
        .insert_with_meta_async(
            note_id.clone(),
            "assistant".to_string(),
            "ᚱᚢᚾᛖ".to_string(),
            raw_answer,
            Some(meta_model.clone()),
            Some(agent.tokens_in() as i32),
            Some(agent.tokens_out() as i32),
            Some(agent.step_count() as i32),
            Some(agent.tool_call_count() as i32),
            meta_thinking,
            Some(agent.total_context_tokens() as i32),
        )
        .await;

    // Send reply to LINE
    if let Some(ref token) = reply_token {
        if let Err(e) = line_client.reply_message(token, &line_reply_text).await {
            error!("Failed to reply message to LINE: {}", e);
        }
    }
}

fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple UTC YYYY-MM-DD estimation or format
    let days = now / 86400;
    // Approximated date for filename
    format!("date-{}", days)
}

fn chrono_now_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_of_day = now % 86400;
    let hours = secs_of_day / 3600;
    let mins = (secs_of_day % 3600) / 60;
    let secs = secs_of_day % 60;
    format!("{:02}:{:02}:{:02} UTC", hours, mins, secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LineNotesConfig, LineUserConfig, RuneConfig};
    use crate::serve::db::ChatDb;
    use crate::serve::line::signature::compute_signature;
    use crate::serve::oauth;
    use axum::body::Bytes;
    use axum::http::HeaderMap;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};

    fn create_test_state_with_line(enabled: bool, secret: &str) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");
        let mut config = RuneConfig::default();
        config.notes.line = Some(LineNotesConfig {
            enabled,
            channel_secret: secret.to_string(),
            channel_access_token: "test_token".to_string(),
            default_note: Some("Default".to_string()),
            users: vec![LineUserConfig {
                user_id: "U12345678".to_string(),
                note: Some("AI".to_string()),
                role: "user".to_string(),
                interactive_chat: true,
            }],
        });

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
            data_dir: std::path::PathBuf::from("/tmp/rune-test-line-handler"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(RwLock::new(HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        }
    }

    #[tokio::test]
    async fn test_line_webhook_missing_signature() {
        let state = create_test_state_with_line(true, "secret123");
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp = line_webhook_handler(State(state), headers, body).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_line_webhook_invalid_signature() {
        let state = create_test_state_with_line(true, "secret123");
        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", "invalid_signature".parse().unwrap());
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp = line_webhook_handler(State(state), headers, body).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_line_webhook_valid_signature() {
        let secret = "my_secret_token";
        let state = create_test_state_with_line(true, secret);
        let body_bytes = br#"{"destination":"U123","events":[]}"#;
        let sig = compute_signature(secret, body_bytes);

        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", sig.parse().unwrap());
        let body = Bytes::from_static(body_bytes);

        let resp = line_webhook_handler(State(state), headers, body).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_line_webhook_not_configured() {
        let state = create_test_state_with_line(false, "");
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp = line_webhook_handler(State(state), headers, body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_process_webhook_unauthorized_user_blocked() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let state = create_test_state_with_line(true, "secret123");
        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                mode: None,
                timestamp: 123456789,
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_STRANGER_999".to_string()),
                    group_id: None,
                    room_id: None,
                }),
                reply_token: Some("dummy_token".to_string()),
                message: Some(EventMessage {
                    id: "msg_1".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Hello".to_string()),
                    quote_token: None,
                }),
                delivery_context: None,
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        // Verify no message was stored in DB because stranger was blocked
        let history = state
            .chat_db
            .load_recent_async("Default".to_string(), 10)
            .await;
        assert!(history.is_empty());
    }
}
