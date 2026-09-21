use super::cache::ProfileCache;
use super::client::LineClient;
use super::commands::{handle_slash_command, is_slash_command};
use super::signature::verify_signature;
use super::types::WebhookPayload;
use crate::agent::{Agent, StopReason};
use crate::config::LineNotesConfig;
use crate::serve::api::{
    broadcast_file_list, broadcast_note_list, broadcast_to_room,
    build_effective_note_system_prompt, build_embedding, build_provider, build_system_prompt,
    SseMsg,
};
use crate::serve::ServerState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Global or lazily-initialized ProfileCache for LINE user display names.
static PROFILE_CACHE: std::sync::OnceLock<ProfileCache> = std::sync::OnceLock::new();

fn get_profile_cache() -> &'static ProfileCache {
    PROFILE_CACHE.get_or_init(ProfileCache::default)
}

/// HTTP handler for named bot endpoint `POST /webhook/line/{nickname}`.
pub async fn line_webhook_named_handler(
    State(state): State<ServerState>,
    Path(nickname): Path<String>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> Response {
    let bot_cfg = state
        .config
        .notes
        .line
        .iter()
        .find(|b| b.nickname == nickname && !b.channel_secret.is_empty());

    let bot_cfg = match bot_cfg {
        Some(cfg) => cfg.clone(),
        None => {
            warn!(
                "LINE Webhook received for unknown or unconfigured bot nickname: {}",
                nickname
            );
            return (StatusCode::NOT_FOUND, "Bot not found").into_response();
        }
    };

    handle_webhook_for_bot(state, bot_cfg, headers, raw_body).await
}

async fn handle_webhook_for_bot(
    state: ServerState,
    bot_cfg: LineNotesConfig,
    headers: HeaderMap,
    raw_body: Bytes,
) -> Response {
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

    if !verify_signature(&bot_cfg.channel_secret, &raw_body, signature) {
        warn!(
            "LINE Webhook signature verification failed for bot [{}]",
            bot_cfg.nickname
        );
        return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
    }

    handle_webhook_payload_with_bot(state, bot_cfg, raw_body).await
}

async fn handle_webhook_payload_with_bot(
    state: ServerState,
    bot_cfg: LineNotesConfig,
    raw_body: Bytes,
) -> Response {
    let payload: WebhookPayload = match serde_json::from_slice(&raw_body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse LINE Webhook JSON payload: {}", e);
            return (StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)).into_response();
        }
    };

    info!(
        "Received LINE Webhook payload for bot [{}] with {} events",
        bot_cfg.nickname,
        payload.events.len()
    );

    let state_clone = state.clone();
    let cache = get_profile_cache().clone();
    tokio::spawn(async move {
        process_webhook_payload_for_bot(state_clone, bot_cfg, payload, cache).await;
    });

    (StatusCode::OK, "OK").into_response()
}

/// Helper background processor for LINE Webhook events (uses the first active bot).
pub async fn process_webhook_payload(
    state: ServerState,
    payload: WebhookPayload,
    cache: ProfileCache,
) {
    let bot_cfg = state
        .config
        .notes
        .line
        .iter()
        .find(|b| !b.channel_secret.is_empty())
        .cloned();

    if let Some(bot_cfg) = bot_cfg {
        process_webhook_payload_for_bot(state, bot_cfg, payload, cache).await;
    }
}

/// Background processor for LINE Webhook events for a specific bot.
pub async fn process_webhook_payload_for_bot(
    state: ServerState,
    line_cfg: LineNotesConfig,
    payload: WebhookPayload,
    cache: ProfileCache,
) {
    let client = LineClient::new(line_cfg.channel_access_token.clone());

    for event in payload.events {
        let text = if event.event_type == "message" {
            if let Some(ref msg) = event.message {
                if let Some(ref t) = msg.text {
                    t.trim().to_string()
                } else if let Some(ref file_name) = msg.file_name {
                    format!(
                        "[LINE File: {} ({:?} bytes)]",
                        file_name,
                        msg.file_size.unwrap_or(0)
                    )
                } else {
                    debug!(
                        "Ignoring unsupported LINE message type: {}",
                        msg.message_type
                    );
                    continue;
                }
            } else {
                continue;
            }
        } else if event.event_type == "postback" {
            if let Some(ref pb) = event.postback {
                pb.data.trim().to_string()
            } else {
                continue;
            }
        } else {
            debug!(
                "Ignoring non-message/postback LINE event: {}",
                event.event_type
            );
            continue;
        };

        if text.is_empty() {
            continue;
        }

        // Group allowlist filtering: if the message originates from a group or room,
        // the groupId/roomId must be present in `line_cfg.groups`.
        let group_id = event
            .source
            .as_ref()
            .and_then(|s| s.group_id.as_deref().or(s.room_id.as_deref()));

        if let Some(gid) = group_id {
            if !line_cfg.groups.iter().any(|g| g == gid) {
                warn!(
                    "Ignoring LINE event from unauthorized group/room ID: {}",
                    gid
                );
                continue;
            }
        }

        let user_id = event
            .source
            .as_ref()
            .and_then(|s| s.user_id.as_deref())
            .unwrap_or("unknown");

        // Role resolution from standard allowlists:
        // - admins / users: interactive AI chat & commands enabled
        // - guests: read-only data collection into resolved note, no AI chat / commands
        // - unregistered (1-on-1): prompt rejection and drop
        let is_admin = line_cfg.admins.iter().any(|id| id == user_id);
        let is_user = line_cfg.users.iter().any(|id| id == user_id);
        let is_guest = line_cfg.guests.iter().any(|id| id == user_id);

        // 1-on-1 stranger rejection: if not in a group, sender MUST be in admins, users, or guests
        if group_id.is_none() && !is_admin && !is_user && !is_guest {
            warn!(
                "Rejecting LINE 1-on-1 message from unregistered user: {}",
                user_id
            );
            if let Some(ref reply_token) = event.reply_token {
                let denied_msg = format!(
                    "⛔ Access denied. Your User ID is `{}`. Please contact an administrator to be added to the allowlist.",
                    user_id
                );
                let _ = client.reply_message(reply_token, &denied_msg).await;
            }
            continue;
        }

        let note_id = resolve_line_note_id(&line_cfg.nickname, group_id, user_id);

        // Group keyword filtering: if from a group and keywords are configured, text must match at least one keyword
        let mut keyword_matched = true;
        if group_id.is_some() && !line_cfg.keywords.is_empty() {
            let lower_text = text.to_lowercase();
            keyword_matched = line_cfg
                .keywords
                .iter()
                .any(|k| lower_text.contains(&k.to_lowercase()));
        }

        let interactive_chat = (is_admin || is_user) && keyword_matched;

        // Ensure notebook exists in DB and notify frontend WebUI if newly created
        if state.chat_db.create_note(&note_id, &note_id, None).is_ok() {
            let md_dir = state.note_markdown_dir(&note_id);
            let _ = tokio::fs::create_dir_all(&md_dir).await;
            broadcast_note_list(&state).await;
        }

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

        let nickname = ProfileCache::format_nickname_for_bot(
            &line_cfg.nickname,
            display_name.as_deref(),
            user_id,
        );

        // Check if slash command
        if is_slash_command(&text) {
            if is_admin || is_user {
                if let Some(reply_text) =
                    handle_slash_command(&text, &state, &note_id, user_id, is_admin).await
                {
                    if let Some(ref reply_token) = event.reply_token {
                        if let Err(e) = client.reply_message(reply_token, &reply_text).await {
                            error!("Failed to reply to LINE slash command: {}", e);
                        }
                    }
                }
                continue;
            }
            // If guest or unregistered in group, fall through to pure data collection mode
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
            // Line Bot logger mode: persist to Chat DB and prepend (Newest First) to Notebook Markdown file
            info!("LINE Bot message received for note [{}]: {}", note_id, text);
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

            // Prepend to bot markdown report file (Newest First)
            let md_dir = state.note_markdown_dir(&note_id);
            let _ = std::fs::create_dir_all(&md_dir);
            let today = chrono_now_date();
            let filename = format!("{}-line-webhook-events.md", today);
            let file_path = md_dir.join(&filename);

            let now_dt = if event.timestamp > 0 {
                let secs = (event.timestamp / 1000) as u64;
                let (y, m, d, h, min, s) = civil_from_timestamp(secs);
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                    y, m, d, h, min, s
                )
            } else {
                chrono_now_datetime()
            };

            // Format payload block nicely (pretty print if valid JSON)
            let formatted_payload =
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    format!(
                        "```json\n{}\n```",
                        serde_json::to_string_pretty(&val).unwrap_or_else(|_| text.clone())
                    )
                } else {
                    format!("```\n{}\n```", text)
                };

            let mut metadata_lines = vec![
                format!("- **User ID**: `{}`", user_id),
                format!("- **Event Type**: `{}`", event.event_type),
            ];
            if let Some(ref src) = event.source {
                if let Some(ref gid) = src.group_id {
                    metadata_lines.push(format!("- **Group ID**: `{}`", gid));
                }
                if let Some(ref rid) = src.room_id {
                    metadata_lines.push(format!("- **Room ID**: `{}`", rid));
                }
            }

            let raw_event_json = serde_json::to_string_pretty(&event).unwrap_or_default();

            let new_entry = format!(
                "### Report from {} ({})\n\n{}\n\n{}\n\n<details>\n<summary>Raw Event Payload</summary>\n\n```json\n{}\n```\n</details>",
                nickname,
                now_dt,
                metadata_lines.join("\n"),
                formatted_payload,
                raw_event_json
            );

            // Prepend new entry: newest content at top, previous content below
            let full_content = if let Ok(existing) = std::fs::read_to_string(&file_path) {
                let trimmed = existing.trim();
                if trimmed.is_empty() {
                    new_entry
                } else {
                    format!("{}\n\n{}", new_entry, trimmed)
                }
            } else {
                new_entry
            };

            let _ = std::fs::write(&file_path, &full_content);

            // Broadcast updated file content to the note room in real-time
            let fc = SseMsg::FileContent {
                note_id: note_id.clone(),
                filename: filename.clone(),
                content: full_content,
            };
            broadcast_to_room(&room, &fc);
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

    // Token streaming callback — sends to room + accumulates for mid-stream reconnect
    let room_for_token = Arc::clone(&room);
    let streaming_buf = Arc::clone(&room.streaming_tokens);
    let status_for_token = Arc::clone(&room.active_status);
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        let msg = SseMsg::ChatToken {
            content: token.to_string(),
        };
        broadcast_to_room(&room_for_token, &msg);
        // Accumulate for clients that reconnect mid-stream
        if let Ok(mut buf) = streaming_buf.try_write() {
            // Update status to "typing" on first token
            if buf.is_empty() {
                if let Ok(mut s) = status_for_token.try_write() {
                    *s = "typing".to_string();
                }
            }
            buf.push_str(token);
        }
    });
    agent.token_callback = Some(token_callback);

    // Tool status callback: broadcast tool start/end to room for UI indicator
    let room_for_tool = Arc::clone(&room);
    let status_for_tool = Arc::clone(&room.active_status);
    agent.tool_status_callback = Some(Arc::new(move |tool_name: &str, state: &str| {
        let msg = SseMsg::ToolStatus {
            tool: tool_name.to_string(),
            state: state.to_string(),
        };
        broadcast_to_room(&room_for_tool, &msg);
        // Update active_status for reconnect recovery
        if state == "start" {
            if let Ok(mut s) = status_for_tool.try_write() {
                *s = format!("tool:{}", tool_name);
            }
        } else if state == "end" {
            if let Ok(mut s) = status_for_tool.try_write() {
                *s = "thinking".to_string();
            }
        }
    }));

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

    // Set system prompt: per-note override + optional persona files > global config > default
    let (system_prompt, _loaded_personas) =
        build_effective_note_system_prompt(&state, &note_id).await;
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

    // Clear streaming buffer — response is complete (or failed)
    {
        let mut buf = room.streaming_tokens.write().await;
        buf.clear();
    }

    let done = SseMsg::ChatDone {};
    broadcast_to_room(&room, &done);

    // Broadcast run statistics to room
    let meta_model = active_model.clone();
    let meta_thinking = effective_thinking_level.clone().filter(|t| t != "off");
    let usage = state.provider_registry.read().await.usage();
    let meta = SseMsg::ChatMeta {
        model: active_model.clone(),
        thinking: meta_thinking.clone(),
        tokens_in: agent.tokens_in() as u32,
        tokens_out: agent.tokens_out() as u32,
        context_tokens: agent.total_context_tokens() as u32,
        context_window: state
            .models
            .read()
            .await
            .iter()
            .find(|m| m.id == meta_model)
            .and_then(|m| m.context_window)
            .unwrap_or(agent.config.context_window as u64) as u32,
        steps: agent.step_count() as u32,
        tool_calls: agent.tool_call_count() as u32,
        usage,
    };
    broadcast_to_room(&room, &meta);

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

fn civil_from_timestamp(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let secs_of_day = (secs % 86400) as u32;
    let hours = secs_of_day / 3600;
    let mins = (secs_of_day % 3600) / 60;
    let secs = secs_of_day % 60;

    // Howard Hinnant's algorithm (civil date from days since 1970-01-01)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y } as i32;

    (year, m, d, hours, mins, secs)
}

fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d, _, _, _) = civil_from_timestamp(now);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn chrono_now_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (_, _, _, h, min, s) = civil_from_timestamp(now);
    format!("{:02}:{:02}:{:02} UTC", h, min, s)
}

fn chrono_now_datetime() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d, h, min, s) = civil_from_timestamp(now);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        y, m, d, h, min, s
    )
}

/// Resolve the target Notebook ID for an incoming LINE event based on `nickname`
/// and the event source (group ID or user ID).
///
/// - If message has a group/room ID: `{nickname}-{GROUP ID}` (or `{GROUP ID}` if nickname is empty)
/// - If message is 1-on-1 (user ID): `{nickname}-{USER ID}` (or `{USER ID}` if nickname is empty)
pub fn resolve_line_note_id(nickname: &str, group_id: Option<&str>, user_id: &str) -> String {
    let target_id = group_id.unwrap_or(user_id);
    let trimmed = nickname.trim();
    if trimmed.is_empty() {
        target_id.to_string()
    } else {
        format!("{}-{}", trimmed, target_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LineNotesConfig, RuneConfig};
    use crate::serve::db::ChatDb;
    use crate::serve::line::signature::compute_signature;
    use crate::serve::oauth;
    use axum::body::Bytes;
    use axum::extract::Path;
    use axum::http::HeaderMap;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};

    fn create_test_state_with_line(secret: &str) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");
        let mut config = RuneConfig::default();
        if !secret.is_empty() {
            config.notes.line = vec![LineNotesConfig {
                nickname: "LineBot".to_string(),
                channel_secret: secret.to_string(),
                channel_access_token: "test_token".to_string(),
                keywords: vec![],
                groups: vec![],
                admins: vec!["U12345678".to_string()],
                users: vec![],
                guests: vec![],
            }];
        }

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

    #[test]
    fn test_resolve_line_note_id_combinations() {
        // 1. Nickname provided
        assert_eq!(
            resolve_line_note_id("LineBot", Some("C12345678"), "U12345678"),
            "LineBot-C12345678"
        );
        assert_eq!(
            resolve_line_note_id("LineBot", None, "U12345678"),
            "LineBot-U12345678"
        );
        assert_eq!(
            resolve_line_note_id(" Team ", Some("C999"), "U111"),
            "Team-C999"
        );

        // 2. Empty nickname
        assert_eq!(
            resolve_line_note_id("", Some("C12345678"), "U12345678"),
            "C12345678"
        );
        assert_eq!(resolve_line_note_id("", None, "U12345678"), "U12345678");
        assert_eq!(resolve_line_note_id("   ", None, "U12345678"), "U12345678");
    }

    #[tokio::test]
    async fn test_line_webhook_named_handler_success() {
        let secret = "my_named_secret";
        let mut state = create_test_state_with_line(secret);
        state.config.notes.line.push(LineNotesConfig {
            nickname: "CIBot".to_string(),
            channel_secret: "ci_secret".to_string(),
            channel_access_token: "ci_token".to_string(),
            ..Default::default()
        });

        let body_bytes = br#"{"destination":"U_CI_BOT","events":[]}"#;
        let sig = compute_signature("ci_secret", body_bytes);

        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", sig.parse().unwrap());
        let body = Bytes::from_static(body_bytes);

        let resp =
            line_webhook_named_handler(State(state), Path("CIBot".to_string()), headers, body)
                .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_line_webhook_named_handler_unknown_bot() {
        let state = create_test_state_with_line("secret123");
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp = line_webhook_named_handler(
            State(state),
            Path("NonExistentBot".to_string()),
            headers,
            body,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_line_webhook_missing_signature() {
        let state = create_test_state_with_line("secret123");
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp =
            line_webhook_named_handler(State(state), Path("LineBot".to_string()), headers, body)
                .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_line_webhook_invalid_signature() {
        let state = create_test_state_with_line("secret123");
        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", "invalidsig".parse().unwrap());
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp =
            line_webhook_named_handler(State(state), Path("LineBot".to_string()), headers, body)
                .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_line_webhook_valid_signature() {
        let secret = "my_secret_token";
        let state = create_test_state_with_line(secret);
        let body_bytes = br#"{"destination":"U123","events":[]}"#;
        let sig = compute_signature(secret, body_bytes);

        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", sig.parse().unwrap());
        let body = Bytes::from_static(body_bytes);

        let resp =
            line_webhook_named_handler(State(state), Path("LineBot".to_string()), headers, body)
                .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_line_webhook_not_configured() {
        let state = create_test_state_with_line("");
        let mut headers = HeaderMap::new();
        headers.insert("x-line-signature", "dummy_sig".parse().unwrap());
        let body = Bytes::from(r#"{"destination":"U123","events":[]}"#);

        let resp =
            line_webhook_named_handler(State(state), Path("LineBot".to_string()), headers, body)
                .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_process_webhook_1on1_stranger_rejected() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let state = create_test_state_with_line("secret123");
        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_STRANGER_999".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy_token".to_string()),
                message: Some(EventMessage {
                    id: "msg_1".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Hello stranger".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        // Verify stranger message was NOT stored in DB
        let history = state
            .chat_db
            .load_recent_async("LineBot-U_STRANGER_999".to_string(), 10)
            .await;
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn test_process_webhook_1on1_guest_collected() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.guests = vec!["U_GUEST_999".to_string()];
        }

        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_GUEST_999".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy_token".to_string()),
                message: Some(EventMessage {
                    id: "msg_1".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Build succeeded with 0 warnings".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        // Verify message was stored in "LineBot-U_GUEST_999" for data collection
        let history = state
            .chat_db
            .load_recent_async("LineBot-U_GUEST_999".to_string(), 10)
            .await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Build succeeded with 0 warnings");
        assert_eq!(history[0].role, "user");
    }

    #[tokio::test]
    async fn test_process_webhook_postback_collected() {
        use crate::serve::line::types::{EventPostback, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.guests = vec!["U_BOT_CLIENT".to_string()];
        }

        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "postback".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_BOT_CLIENT".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy_token".to_string()),
                postback: Some(EventPostback {
                    data: r#"{"action":"ci_result","status":"passed"}"#.to_string(),
                    params: None,
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        let history = state
            .chat_db
            .load_recent_async("LineBot-U_BOT_CLIENT".to_string(), 10)
            .await;
        assert_eq!(history.len(), 1);
        assert!(history[0].content.contains(r#"{"action":"ci_result""#));
    }

    #[test]
    fn test_civil_from_timestamp_accuracy() {
        // Epoch: 1970-01-01 00:00:00 UTC
        assert_eq!(civil_from_timestamp(0), (1970, 1, 1, 0, 0, 0));

        // Leap day: 2000-02-29 12:30:45 UTC (951827445)
        assert_eq!(civil_from_timestamp(951827445), (2000, 2, 29, 12, 30, 45));

        // 2026-09-20 16:30:00 UTC (1789862400 + 16*3600 + 30*60 = 1789921800)
        assert_eq!(civil_from_timestamp(1789921800), (2026, 9, 20, 16, 30, 0));

        // 2026-09-21 00:00:00 UTC (20717 * 86400 = 1789948800)
        assert_eq!(civil_from_timestamp(1789948800), (2026, 9, 21, 0, 0, 0));
    }

    #[tokio::test]
    async fn test_process_webhook_broadcasts_file_content_and_file_list() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.guests = vec!["U_GUEST_123".to_string()];
        }

        let room = state.get_or_create_room("LineBot-U_GUEST_123").await;
        let mut rx = room.broadcast_tx.subscribe();

        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_GUEST_123".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy_token".to_string()),
                message: Some(EventMessage {
                    id: "msg_2".to_string(),
                    message_type: "text".to_string(),
                    text: Some(r#"{"status":"ready"}"#.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        let mut received_messages = Vec::new();
        while let Ok(msg_str) = rx.try_recv() {
            received_messages.push(msg_str);
        }

        // Check that ChatMessage, FileContent, and FileList were all broadcast to the room
        assert!(
            received_messages.iter().any(|m| m.contains("chat_message")),
            "Expected chat_message broadcast, got: {:?}",
            received_messages
        );
        assert!(
            received_messages.iter().any(|m| m.contains("file_content")),
            "Expected file_content broadcast, got: {:?}",
            received_messages
        );
        assert!(
            received_messages.iter().any(|m| m.contains("file_list")),
            "Expected file_list broadcast, got: {:?}",
            received_messages
        );
    }

    #[tokio::test]
    async fn test_process_webhook_group_allowlist_and_keywords_filtering() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut line_cfg) = state.config.notes.line.first_mut() {
            line_cfg.groups = vec!["C_ALLOWED_GROUP".to_string()];
            line_cfg.keywords = vec!["@bot".to_string(), "rune".to_string()];
            line_cfg.admins = vec!["U_ADMIN_USER".to_string()];
        }

        // 1. Event from unauthorized group
        let unauthorized_payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "group".to_string(),
                    group_id: Some("C_BLOCKED_GROUP".to_string()),
                    user_id: Some("U_ADMIN_USER".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("token1".to_string()),
                message: Some(EventMessage {
                    id: "msg_blocked".to_string(),
                    message_type: "text".to_string(),
                    text: Some("@bot hello from blocked group".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), unauthorized_payload, cache.clone()).await;

        let history = state
            .chat_db
            .load_recent_async("LineBot-C_ALLOWED_GROUP".to_string(), 10)
            .await;
        assert_eq!(history.len(), 0, "Blocked group should not be recorded");

        // 2. Event from authorized group without keywords -> recorded as data collection (no AI chat)
        let unkeyworded_payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "group".to_string(),
                    group_id: Some("C_ALLOWED_GROUP".to_string()),
                    user_id: Some("U_ADMIN_USER".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("token2".to_string()),
                message: Some(EventMessage {
                    id: "msg_unkeyworded".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Casual group chat without keyword".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        process_webhook_payload(state.clone(), unkeyworded_payload, cache.clone()).await;

        let history = state
            .chat_db
            .load_recent_async("LineBot-C_ALLOWED_GROUP".to_string(), 10)
            .await;
        assert_eq!(history.len(), 1, "Unkeyworded group chat should be logged");
        assert_eq!(history[0].content, "Casual group chat without keyword");
    }

    #[tokio::test]
    async fn test_process_webhook_data_collection_prepend_order() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut line_cfg) = state.config.notes.line.first_mut() {
            line_cfg.groups = vec!["C_GROUP_ORDER".to_string()];
        }

        let payload_1 = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "group".to_string(),
                    group_id: Some("C_GROUP_ORDER".to_string()),
                    user_id: Some("U_MEMBER_1".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("token1".to_string()),
                message: Some(EventMessage {
                    id: "msg_first".to_string(),
                    message_type: "text".to_string(),
                    text: Some("First event content".to_string()),
                    ..Default::default()
                }),
                timestamp: 1000000,
                ..Default::default()
            }],
        };

        let payload_2 = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "group".to_string(),
                    group_id: Some("C_GROUP_ORDER".to_string()),
                    user_id: Some("U_MEMBER_2".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("token2".to_string()),
                message: Some(EventMessage {
                    id: "msg_second".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Second event content".to_string()),
                    ..Default::default()
                }),
                timestamp: 2000000,
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload_1, cache.clone()).await;
        process_webhook_payload(state.clone(), payload_2, cache.clone()).await;

        let md_dir = state.note_markdown_dir("LineBot-C_GROUP_ORDER");
        let today = chrono_now_date();
        let filename = format!("{}-line-webhook-events.md", today);
        let file_path = md_dir.join(&filename);

        let content = std::fs::read_to_string(&file_path).unwrap();
        let pos_first = content.find("First event content").unwrap();
        let pos_second = content.find("Second event content").unwrap();

        // Second (newer) event must be located BEFORE the first event (Prepend / Newest First)
        assert!(
            pos_second < pos_first,
            "Newer event should be prepended before older event in markdown file"
        );
    }
}
