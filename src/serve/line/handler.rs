use super::cache::ProfileCache;
use super::client::LineClient;
use super::commands::{handle_slash_command, is_slash_command};
use super::signature::verify_signature;
use super::types::WebhookPayload;
use crate::agent::{Agent, StopReason};
use crate::config::LineNotesConfig;
use crate::serve::api::{
    atomic_write_file, broadcast_file_list, broadcast_note_list, broadcast_to_room,
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
/// Background processor for LINE Webhook events for a specific bot.
pub async fn process_webhook_payload_for_bot(
    state: ServerState,
    line_cfg: LineNotesConfig,
    payload: WebhookPayload,
    cache: ProfileCache,
) {
    let client = LineClient::new(line_cfg.channel_access_token.clone());
    let note_id = resolve_line_note_id(&line_cfg.nickname);

    // Ensure notebook exists in DB and notify frontend WebUI if newly created
    let _ = state.chat_db.ensure_persistent();
    if state.chat_db.create_note(&note_id, &note_id, None).is_ok() {
        let md_dir = state.note_markdown_dir(&note_id);
        let _ = tokio::fs::create_dir_all(&md_dir).await;
        if let Some(parent) = md_dir.parent() {
            let archive_dir = parent.join("archives");
            let _ = tokio::fs::create_dir_all(&archive_dir).await;
        }
        broadcast_note_list(&state).await;
    }

    for event in payload.events {
        let group_id = event.source.as_ref().and_then(|s| s.group_id.as_deref());

        let user_id = event
            .source
            .as_ref()
            .and_then(|s| s.user_id.as_deref())
            .unwrap_or("unknown");

        // Role resolution from standard allowlists
        let is_admin = line_cfg.admins.iter().any(|id| id == user_id);
        let is_user = line_cfg.users.iter().any(|id| id == user_id);
        let is_guest = line_cfg.guests.iter().any(|id| id == user_id);
        let is_in_whitelist = group_id
            .map(|gid| line_cfg.groups.iter().any(|g| g == gid || g == "*"))
            .unwrap_or(false)
            || is_admin
            || is_user
            || is_guest;

        // Resolve display name via Cache / Profile API
        let display_name = match cache.get(user_id).await {
            Some(name) => Some(name),
            None => {
                if user_id != "unknown" && !line_cfg.channel_access_token.is_empty() {
                    let res = if let Some(gid) = group_id {
                        client.get_group_member_profile(gid, user_id).await
                    } else {
                        client.get_profile(user_id).await
                    };
                    match res {
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

        // Resolve group name via Cache / Group Summary API (if group message)
        let group_name = if let Some(gid) = group_id {
            match cache.get_group(gid).await {
                Some(name) => Some(name),
                None => {
                    if !line_cfg.channel_access_token.is_empty() {
                        match client.get_group_summary(gid).await {
                            Ok(summary) => {
                                cache
                                    .insert_group(gid.to_string(), summary.group_name.clone())
                                    .await;
                                Some(summary.group_name)
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to fetch group summary for LINE group {}: {}",
                                    gid, e
                                );
                                None
                            }
                        }
                    } else {
                        None
                    }
                }
            }
        } else {
            None
        };

        let session_id = if let Some(gid) = group_id {
            format!("group:{}", gid)
        } else if user_id != "unknown" {
            format!("user:{}", user_id)
        } else {
            "main".to_string()
        };

        let auto_title = if group_id.is_some() {
            group_name.clone()
        } else if user_id != "unknown" {
            display_name.clone()
        } else {
            None
        };

        // Immediately persist session in DB (with auto title if available) and broadcast to connected WebUI clients
        if session_id != "main" {
            let _ = state
                .chat_db
                .set_session_title_async(note_id.clone(), session_id.clone(), auto_title.clone())
                .await;
            crate::serve::api::broadcast_session_list(&state, &note_id).await;
        }

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

        let nickname = ProfileCache::format_nickname_for_bot(
            &line_cfg.nickname,
            display_name.as_deref(),
            user_id,
        );

        // 1. Configurable Event Logging (Logging Matrix)
        let should_log = line_cfg.log && (line_cfg.anonymous || is_in_whitelist);
        if should_log {
            let source_id = extract_source_id(&event);
            let event_secs = if event.timestamp > 0 {
                (event.timestamp / 1000) as u64
            } else {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            };
            let (tz_offset, tz_label) =
                crate::serve::timezone::resolve_timezone_offset(&line_cfg.timezone, event_secs);
            let today = crate::serve::timezone::format_date_with_tz(event_secs, tz_offset);
            let filename = format!("{}-line-{}.md", today, source_id);
            let md_dir = state.note_markdown_dir(&note_id);
            let _ = std::fs::create_dir_all(&md_dir);
            let file_path = md_dir.join(&filename);

            let now_dt =
                crate::serve::timezone::format_datetime_with_tz(event_secs, tz_offset, &tz_label);

            let formatted_payload =
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    format!(
                        "```json\n{}\n```",
                        serde_json::to_string_pretty(&val).unwrap_or_else(|_| text.clone())
                    )
                } else {
                    format!("```\n{}\n```", text)
                };

            let role_str = if is_admin {
                "admin"
            } else if is_user {
                "user"
            } else if is_guest {
                "guest"
            } else {
                "anonymous"
            };

            let mut metadata_lines = Vec::new();
            if let Some(ref src) = event.source {
                metadata_lines.push(format!("- **Source Type**: `{}`", src.source_type));
                if let Some(ref gid) = src.group_id {
                    metadata_lines.push(format!("- **Group ID**: `{}`", gid));
                }
                if let Some(ref rid) = src.room_id {
                    metadata_lines.push(format!("- **Room ID**: `{}`", rid));
                }
            }
            metadata_lines.push(format!("- **User ID**: `{}`", user_id));
            metadata_lines.push(format!("- **Role**: `{}`", role_str));
            metadata_lines.push(format!("- **Event Type**: `{}`", event.event_type));

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

            let _ = atomic_write_file(&file_path, &full_content).await;

            // Broadcast updated file content to the note room in real-time
            let room = state.get_or_create_room(&note_id).await;
            let fc = SseMsg::FileContent {
                note_id: note_id.clone(),
                filename: filename.clone(),
                content: full_content,
            };
            broadcast_to_room(&room, &fc);
            broadcast_file_list(&state, &note_id).await;
        }

        // 2. RBAC & AI Execution Flow (Decoupled from Logging)
        if let Some(gid) = group_id {
            // Group / Room flow
            if !line_cfg.groups.iter().any(|g| g == gid || g == "*") {
                debug!(
                    "Ignoring LINE event from unauthorized group/room ID: {}",
                    gid
                );
                continue;
            }

            let mut keyword_matched = true;
            if !line_cfg.keywords.is_empty() {
                let lower_text = text.to_lowercase();
                keyword_matched = line_cfg
                    .keywords
                    .iter()
                    .any(|k| lower_text.contains(&k.to_lowercase()));
            }

            if !keyword_matched {
                debug!("Group message did not match any keywords, skipping AI response");
                continue;
            }

            if is_guest {
                debug!("Group sender is guest, skipping AI response");
                continue;
            }
        } else {
            // 1-on-1 private chat flow
            if !is_admin && !is_user && !is_guest {
                warn!(
                    "Rejecting LINE 1-on-1 message from unregistered user: {}",
                    user_id
                );
                if let Some(ref reply_token) = event.reply_token {
                    let denied_msg = line_cfg.access_denied_message.replace("{user_id}", user_id);
                    let _ = client.reply_message(reply_token, &denied_msg).await;
                }
                continue;
            }

            if is_guest {
                debug!("1-on-1 sender is guest, skipping AI response");
                continue;
            }
        }

        // Check if slash command
        if is_slash_command(&text) {
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

        // Interactive AI Chat
        let reply_token = event.reply_token.clone();
        let client_clone = client.clone();
        let state_clone = state.clone();
        let note_id_clone = note_id.clone();
        let nickname_clone = nickname.clone();
        let text_clone = text.clone();
        let session_id = if let Some(gid) = group_id {
            format!("group:{}", gid)
        } else if user_id != "unknown" {
            format!("user:{}", user_id)
        } else {
            "main".to_string()
        };
        let chat_id = if let Some(gid) = group_id {
            gid.to_string()
        } else {
            user_id.to_string()
        };

        let auto_title = if group_id.is_some() {
            group_name.clone()
        } else if user_id != "unknown" {
            display_name.clone()
        } else {
            None
        };

        tokio::spawn(async move {
            // Show loading animation in LINE chat
            if chat_id != "unknown" {
                let _ = client_clone
                    .start_loading_animation(&chat_id, Some(60))
                    .await;
            }

            // Save auto title from LINE API
            if let Some(ref t) = auto_title {
                let _ = state_clone
                    .chat_db
                    .set_session_title_async(
                        note_id_clone.clone(),
                        session_id.clone(),
                        Some(t.clone()),
                    )
                    .await;
            }

            state_clone
                .chat_db
                .insert_session_async(
                    note_id_clone.clone(),
                    session_id.clone(),
                    "user".to_string(),
                    nickname_clone.clone(),
                    text_clone.clone(),
                )
                .await;

            crate::serve::api::broadcast_session_list(&state_clone, &note_id_clone).await;

            // Broadcast user message to room
            let room = state_clone.get_or_create_room(&note_id_clone).await;
            let user_msg = SseMsg::ChatMessage {
                nickname: nickname_clone.clone(),
                content: text_clone.clone(),
                session_id: Some(session_id.clone()),
                session_title: auto_title.clone(),
            };
            broadcast_to_room(&room, &user_msg);

            // Set active status to thinking
            {
                let mut status = room.active_status.write().await;
                *status = "thinking".to_string();
            }

            // Send thinking status to room
            let thinking = SseMsg::Status {
                state: "thinking".to_string(),
            };
            broadcast_to_room(&room, &thinking);

            // Run Agent & reply
            execute_line_agent_and_reply(
                state_clone,
                note_id_clone,
                session_id,
                nickname_clone,
                text_clone,
                reply_token,
                client_clone,
            )
            .await;
        });
    }
}

/// Executes Rune Agent for LINE message and replies via LINE Messaging API.
async fn execute_line_agent_and_reply(
    state: ServerState,
    note_id: String,
    session_id: String,
    nickname: String,
    user_msg: String,
    reply_token: Option<String>,
    line_client: LineClient,
) {
    let config = state.config.clone();
    let active_model = state
        .effective_model_for_session(&note_id, &session_id)
        .await;
    let room = state.get_or_create_room(&note_id).await;

    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let err_msg = format!("⚠️ Provider error: {}", e);
            if let Some(token) = reply_token {
                let _ = line_client.reply_message(&token, &err_msg).await;
            }
            let err = SseMsg::Error {
                message: format!("Provider error: {}", e),
            };
            broadcast_to_room(&room, &err);
            {
                let mut status = room.active_status.write().await;
                *status = "idle".to_string();
            }
            let idle = SseMsg::Status {
                state: "idle".to_string(),
            };
            broadcast_to_room(&room, &idle);
            return;
        }
    };

    let embedding = build_embedding(&config).await;

    // Build agent
    let mut cfg = config.clone();
    cfg.model = active_model.clone();
    let effective_thinking_level = state
        .effective_thinking_for_session(&note_id, &session_id)
        .await;
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
    let sess_id_token = session_id.clone();
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        let msg = SseMsg::ChatToken {
            content: token.to_string(),
            session_id: Some(sess_id_token.clone()),
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

    // Notify UI whenever AI writes/creates a markdown file — broadcast to room
    let state_for_filelist = state.clone();
    let note_id_for_filelist = note_id.clone();
    agent.file_list_callback = Some(Arc::new(move || {
        let s = state_for_filelist.clone();
        let n = note_id_for_filelist.clone();
        tokio::spawn(async move {
            broadcast_file_list(&s, &n).await;
        });
    }));

    // Broadcast file content changes to all users in the room (real-time sync)
    let state_for_content = state.clone();
    let note_id_for_content = note_id.clone();
    agent.file_content_callback = Some(Arc::new(move |filename: String, content: String| {
        let s = state_for_content.clone();
        let n = note_id_for_content.clone();
        tokio::spawn(async move {
            let room = s.get_or_create_room(&n).await;
            let fc = SseMsg::FileContent {
                note_id: n,
                filename,
                content,
            };
            broadcast_to_room(&room, &fc);
        });
    }));

    // Set system prompt: per-note override + optional persona files > global config > default
    let (system_prompt, _loaded_personas) =
        build_effective_note_system_prompt(&state, &note_id).await;
    agent.set_system_prompt(&system_prompt);

    // Load recent history (up to 20 records) for this session
    let history = state
        .chat_db
        .load_recent_session_async(note_id.clone(), session_id.clone(), 20)
        .await;
    let history_without_current: Vec<_> = history
        .into_iter()
        .filter(|r| !(r.role == "user" && r.content == user_msg))
        .collect();
    agent.load_history(&history_without_current);

    // Run agent
    let start_time = std::time::Instant::now();
    let stop_reason = agent.run(&user_msg).await;
    let duration_ms = start_time.elapsed().as_millis() as u64;

    // Clear streaming buffer — response is complete (or failed)
    {
        let mut buf = room.streaming_tokens.write().await;
        buf.clear();
    }

    let done = SseMsg::ChatDone {
        session_id: Some(session_id.clone()),
    };
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
        duration_ms: Some(duration_ms),
        usage,
        session_id: Some(session_id.clone()),
    };
    broadcast_to_room(&room, &meta);

    // Run statistics line
    let total_tokens = agent.tokens_in() + agent.tokens_out();
    let stats_line = format!(
        "⚡ {} steps · {} tokens · {} tool calls · {}",
        agent.step_count(),
        total_tokens,
        agent.tool_call_count(),
        crate::serve::format_duration_ms(duration_ms)
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

    // Save assistant message to Chat DB with session_id
    let meta_model = active_model.clone();
    let meta_thinking = effective_thinking_level.clone().filter(|t| t != "off");
    state
        .chat_db
        .insert_session_with_meta_async(
            note_id.clone(),
            session_id.clone(),
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
            Some(duration_ms),
        )
        .await;

    // Send reply to LINE
    if let Some(ref token) = reply_token {
        if let Err(e) = line_client.reply_message(token, &line_reply_text).await {
            error!("Failed to reply message to LINE: {}", e);
        }
    }

    // Always reset room status to idle and broadcast to room users
    {
        let mut status = room.active_status.write().await;
        *status = "idle".to_string();
    }
    let idle_msg = SseMsg::Status {
        state: "idle".to_string(),
    };
    broadcast_to_room(&room, &idle_msg);

    // Broadcast updated file list to the room
    broadcast_file_list(&state, &note_id).await;
}

fn civil_from_timestamp(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    crate::serve::timezone::civil_from_timestamp(secs)
}

fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    crate::serve::timezone::format_date_with_tz(now, 0)
}

fn chrono_now_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (_, _, _, h, min, s) = crate::serve::timezone::civil_from_timestamp(now);
    format!("{:02}:{:02}:{:02} UTC", h, min, s)
}

fn chrono_now_datetime() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    crate::serve::timezone::format_datetime_with_tz(now, 0, "UTC")
}

/// Resolve the target Notebook ID for an incoming LINE bot based on `nickname`.
///
/// Returns `{nickname}` (or `"LineBot"` if nickname is empty/whitespace).
pub fn resolve_line_note_id(nickname: &str) -> String {
    let trimmed = nickname.trim();
    if trimmed.is_empty() {
        "LineBot".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extracts the source identifier (groupId > roomId > userId > "unknown") from a WebhookEvent.
pub fn extract_source_id(event: &crate::serve::line::types::WebhookEvent) -> &str {
    if let Some(ref src) = event.source {
        if let Some(ref gid) = src.group_id {
            return gid.as_str();
        }
        if let Some(ref rid) = src.room_id {
            return rid.as_str();
        }
        if let Some(ref uid) = src.user_id {
            return uid.as_str();
        }
    }
    "unknown"
}

/// Asynchronously resolves missing friendly titles for LINE sessions (e.g. from 1-on-1 or groups)
/// and updates `chat_sessions` table and broadcasts updated list if any title was resolved.
pub async fn resolve_missing_session_titles(state: &ServerState, note_id: &str) {
    let bot_cfg = match state.config.notes.line.iter().find(|b| {
        resolve_line_note_id(&b.nickname) == note_id && !b.channel_access_token.is_empty()
    }) {
        Some(cfg) => cfg.clone(),
        None => return,
    };

    let meta_list = match state
        .chat_db
        .list_chat_sessions_meta_async(note_id.to_string())
        .await
    {
        Ok(list) => list,
        Err(_) => return,
    };

    let client = LineClient::new(bot_cfg.channel_access_token.clone());
    let cache = get_profile_cache().clone();
    let mut updated = false;

    for meta in meta_list {
        if meta.title.is_some() || meta.custom_title.is_some() {
            continue;
        }

        if meta.session_id.starts_with("user:") {
            let user_id = &meta.session_id["user:".len()..];
            if user_id.starts_with('U') && user_id.len() >= 8 {
                let name = match cache.get(user_id).await {
                    Some(n) => Some(n),
                    None => match client.get_profile(user_id).await {
                        Ok(p) => {
                            cache
                                .insert(user_id.to_string(), p.display_name.clone())
                                .await;
                            Some(p.display_name)
                        }
                        Err(e) => {
                            warn!("Lazy title resolve failed for LINE user {}: {}", user_id, e);
                            None
                        }
                    },
                };
                if let Some(t) = name {
                    let _ = state
                        .chat_db
                        .set_session_title_async(
                            note_id.to_string(),
                            meta.session_id.clone(),
                            Some(t),
                        )
                        .await;
                    updated = true;
                }
            }
        } else if meta.session_id.starts_with("group:") {
            let group_id = &meta.session_id["group:".len()..];
            if group_id.starts_with('C') && group_id.len() >= 8 {
                let name = match cache.get_group(group_id).await {
                    Some(n) => Some(n),
                    None => match client.get_group_summary(group_id).await {
                        Ok(s) => {
                            cache
                                .insert_group(group_id.to_string(), s.group_name.clone())
                                .await;
                            Some(s.group_name)
                        }
                        Err(e) => {
                            warn!(
                                "Lazy title resolve failed for LINE group {}: {}",
                                group_id, e
                            );
                            None
                        }
                    },
                };
                if let Some(t) = name {
                    let _ = state
                        .chat_db
                        .set_session_title_async(
                            note_id.to_string(),
                            meta.session_id.clone(),
                            Some(t),
                        )
                        .await;
                    updated = true;
                }
            }
        }
    }

    if updated {
        crate::serve::api::broadcast_session_list(state, note_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LineNotesConfig, RuneConfig};
    use crate::serve::db::ChatDb;
    use crate::serve::line::signature::compute_signature;
    use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent, WebhookPayload};
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
                log: false,
                anonymous: false,
                timezone: "UTC".to_string(),
                access_denied_message: "⛔ Access denied. User ID: {user_id}".to_string(),
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
            running_cron_jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())),
        }
    }

    #[test]
    fn test_resolve_line_note_id() {
        assert_eq!(resolve_line_note_id("LineBot"), "LineBot");
        assert_eq!(resolve_line_note_id(" TeamBot "), "TeamBot");
        assert_eq!(resolve_line_note_id(""), "LineBot");
        assert_eq!(resolve_line_note_id("   "), "LineBot");
    }

    #[test]
    fn test_extract_source_id() {
        use crate::serve::line::types::{EventSource, WebhookEvent};

        let event_group = WebhookEvent {
            source: Some(EventSource {
                source_type: "group".to_string(),
                group_id: Some("C123".to_string()),
                user_id: Some("U456".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(extract_source_id(&event_group), "C123");

        let event_room = WebhookEvent {
            source: Some(EventSource {
                source_type: "room".to_string(),
                room_id: Some("R789".to_string()),
                user_id: Some("U456".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(extract_source_id(&event_room), "R789");

        let event_user = WebhookEvent {
            source: Some(EventSource {
                source_type: "user".to_string(),
                user_id: Some("U456".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(extract_source_id(&event_user), "U456");

        let event_none = WebhookEvent::default();
        assert_eq!(extract_source_id(&event_none), "unknown");
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
            .load_recent_async("LineBot".to_string(), 10)
            .await;
        assert_eq!(history.len(), 0);

        // Verify no log file was created (since log = false by default)
        let md_dir = state.note_markdown_dir("LineBot");
        let log_file = md_dir.join(format!("{}-line-U_STRANGER_999.md", chrono_now_date()));
        assert!(!log_file.exists());
    }

    #[tokio::test]
    async fn test_process_webhook_logging_matrix() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let temp_dir = tempfile::tempdir().unwrap();

        // 1. log = false: No files created for any event
        {
            let mut state = create_test_state_with_line("secret123");
            state.data_dir = temp_dir.path().to_path_buf();
            if let Some(ref mut bot) = state.config.notes.line.first_mut() {
                bot.log = false;
                bot.anonymous = false;
                bot.guests = vec!["U_GUEST_1".to_string()];
            }

            let payload = WebhookPayload {
                destination: Some("U_BOT".to_string()),
                events: vec![WebhookEvent {
                    event_type: "message".to_string(),
                    source: Some(EventSource {
                        source_type: "user".to_string(),
                        user_id: Some("U_GUEST_1".to_string()),
                        ..Default::default()
                    }),
                    reply_token: Some("tok1".to_string()),
                    message: Some(EventMessage {
                        id: "m1".to_string(),
                        message_type: "text".to_string(),
                        text: Some("guest data".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            };

            process_webhook_payload(state.clone(), payload, ProfileCache::default()).await;
            let log_file = state
                .note_markdown_dir("LineBot")
                .join(format!("{}-line-U_GUEST_1.md", chrono_now_date()));
            assert!(!log_file.exists(), "log=false should not create log file");
        }

        // 2. log = true, anonymous = false: Whitelisted generates file, Stranger does not
        {
            let mut state = create_test_state_with_line("secret123");
            state.data_dir = temp_dir.path().to_path_buf();
            if let Some(ref mut bot) = state.config.notes.line.first_mut() {
                bot.log = true;
                bot.anonymous = false;
                bot.guests = vec!["U_GUEST_2".to_string()];
            }

            let payload = WebhookPayload {
                destination: Some("U_BOT".to_string()),
                events: vec![
                    WebhookEvent {
                        event_type: "message".to_string(),
                        source: Some(EventSource {
                            source_type: "user".to_string(),
                            user_id: Some("U_GUEST_2".to_string()),
                            ..Default::default()
                        }),
                        reply_token: Some("tok2".to_string()),
                        message: Some(EventMessage {
                            id: "m2".to_string(),
                            message_type: "text".to_string(),
                            text: Some("whitelisted guest message".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    WebhookEvent {
                        event_type: "message".to_string(),
                        source: Some(EventSource {
                            source_type: "user".to_string(),
                            user_id: Some("U_STRANGER_2".to_string()),
                            ..Default::default()
                        }),
                        reply_token: Some("tok3".to_string()),
                        message: Some(EventMessage {
                            id: "m3".to_string(),
                            message_type: "text".to_string(),
                            text: Some("stranger message".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
            };

            process_webhook_payload(state.clone(), payload, ProfileCache::default()).await;
            let guest_log = state
                .note_markdown_dir("LineBot")
                .join(format!("{}-line-U_GUEST_2.md", chrono_now_date()));
            let stranger_log = state
                .note_markdown_dir("LineBot")
                .join(format!("{}-line-U_STRANGER_2.md", chrono_now_date()));
            assert!(
                guest_log.exists(),
                "whitelisted guest should generate log file"
            );
            assert!(
                !stranger_log.exists(),
                "stranger should not generate log file when anonymous=false"
            );
        }

        // 3. log = true, anonymous = true: Stranger generates log file
        {
            let mut state = create_test_state_with_line("secret123");
            state.data_dir = temp_dir.path().to_path_buf();
            if let Some(ref mut bot) = state.config.notes.line.first_mut() {
                bot.log = true;
                bot.anonymous = true;
            }

            let payload = WebhookPayload {
                destination: Some("U_BOT".to_string()),
                events: vec![WebhookEvent {
                    event_type: "message".to_string(),
                    source: Some(EventSource {
                        source_type: "user".to_string(),
                        user_id: Some("U_STRANGER_ANON".to_string()),
                        ..Default::default()
                    }),
                    reply_token: Some("tok4".to_string()),
                    message: Some(EventMessage {
                        id: "m4".to_string(),
                        message_type: "text".to_string(),
                        text: Some("stranger anonymous message".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            };

            process_webhook_payload(state.clone(), payload, ProfileCache::default()).await;
            let stranger_log = state
                .note_markdown_dir("LineBot")
                .join(format!("{}-line-U_STRANGER_ANON.md", chrono_now_date()));
            assert!(
                stranger_log.exists(),
                "stranger should generate log file when anonymous=true"
            );
            let content = std::fs::read_to_string(&stranger_log).unwrap();
            assert!(content.contains("- **Role**: `anonymous`"));
            assert!(content.contains("stranger anonymous message"));
        }
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

        let temp_dir = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_line("secret123");
        state.data_dir = temp_dir.path().to_path_buf();
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.log = true;
            bot.guests = vec!["U_GUEST_123".to_string()];
        }

        let room = state.get_or_create_room("LineBot").await;
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

        // Check that FileContent and FileList were broadcast to the centralized room
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

        let temp_dir = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_line("secret123");
        state.data_dir = temp_dir.path().to_path_buf();
        if let Some(ref mut line_cfg) = state.config.notes.line.first_mut() {
            line_cfg.log = true;
            line_cfg.groups = vec!["C_ALLOWED_GROUP".to_string()];
            line_cfg.keywords = vec!["@bot".to_string(), "rune".to_string()];
            line_cfg.admins = vec!["U_ADMIN_USER".to_string()];
        }

        // 1. Event from unauthorized group (with anonymous = false) -> no log file and no chat DB
        let unauthorized_payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "group".to_string(),
                    group_id: Some("C_BLOCKED_GROUP".to_string()),
                    user_id: Some("U_UNKNOWN".to_string()),
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

        let blocked_log = state
            .note_markdown_dir("LineBot")
            .join(format!("{}-line-C_BLOCKED_GROUP.md", chrono_now_date()));
        assert!(!blocked_log.exists());

        // 2. Event from authorized group without keywords -> logged to file, but no AI chat DB
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

        let allowed_log = state
            .note_markdown_dir("LineBot")
            .join(format!("{}-line-C_ALLOWED_GROUP.md", chrono_now_date()));
        assert!(allowed_log.exists());
        let content = std::fs::read_to_string(&allowed_log).unwrap();
        assert!(content.contains("Casual group chat without keyword"));
    }

    #[tokio::test]
    async fn test_process_webhook_data_collection_prepend_order() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let temp_dir = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_line("secret123");
        state.data_dir = temp_dir.path().to_path_buf();
        if let Some(ref mut line_cfg) = state.config.notes.line.first_mut() {
            line_cfg.log = true;
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

        let md_dir = state.note_markdown_dir("LineBot");
        let filename = "1970-01-01-line-C_GROUP_ORDER.md";
        let file_path = md_dir.join(filename);

        let content = std::fs::read_to_string(&file_path).unwrap();
        let pos_first = content.find("First event content").unwrap();
        let pos_second = content.find("Second event content").unwrap();

        // Second (newer) event must be located BEFORE the first event (Prepend / Newest First)
        assert!(
            pos_second < pos_first,
            "Newer event should be prepended before older event in markdown file"
        );
    }

    #[tokio::test]
    async fn test_process_webhook_custom_access_denied_message() {
        use crate::serve::line::types::{EventMessage, EventSource, WebhookEvent};

        let mut state = create_test_state_with_line("secret123");
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.access_denied_message = "Sorry {user_id}, you do not have permission.".to_string();
        }

        let denied_msg = state.config.notes.line[0]
            .access_denied_message
            .replace("{user_id}", "U_STRANGER_123");
        assert_eq!(
            denied_msg,
            "Sorry U_STRANGER_123, you do not have permission."
        );

        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_STRANGER_123".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy_token".to_string()),
                message: Some(EventMessage {
                    id: "msg_custom_denied".to_string(),
                    message_type: "text".to_string(),
                    text: Some("hi".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        };

        let cache = ProfileCache::default();
        process_webhook_payload(state.clone(), payload, cache).await;

        let history = state
            .chat_db
            .load_recent_async("LineBot".to_string(), 10)
            .await;
        assert_eq!(history.len(), 0);
    }

    #[tokio::test]
    async fn test_execute_line_agent_status_reset_to_idle() {
        let state = create_test_state_with_line("secret123");
        let room = state.get_or_create_room("LineBot").await;
        *room.active_status.write().await = "thinking".to_string();

        let client = LineClient::new("dummy_token".to_string());
        execute_line_agent_and_reply(
            state.clone(),
            "LineBot".to_string(),
            "main".to_string(),
            "User".to_string(),
            "hello".to_string(),
            None,
            client,
        )
        .await;

        let status = room.active_status.read().await;
        assert_eq!(*status, "idle");
    }

    #[tokio::test]
    async fn test_line_user_and_group_session_isolation() {
        let state = create_test_state_with_line("secret123");
        state
            .chat_db
            .create_note("LineBot", "LineBot", None)
            .unwrap();

        // Simulate user U123 message
        state
            .chat_db
            .insert_session_async(
                "LineBot".to_string(),
                "user:U123".to_string(),
                "user".to_string(),
                "Alice".to_string(),
                "Alice private query".to_string(),
            )
            .await;

        // Simulate user U456 message
        state
            .chat_db
            .insert_session_async(
                "LineBot".to_string(),
                "user:U456".to_string(),
                "user".to_string(),
                "Bob".to_string(),
                "Bob private query".to_string(),
            )
            .await;

        // Simulate group C789 message
        state
            .chat_db
            .insert_session_async(
                "LineBot".to_string(),
                "group:C789".to_string(),
                "user".to_string(),
                "Charlie".to_string(),
                "Group topic".to_string(),
            )
            .await;

        // Check isolation
        let alice_history = state
            .chat_db
            .load_recent_session_async("LineBot".to_string(), "user:U123".to_string(), 10)
            .await;
        assert_eq!(alice_history.len(), 1);
        assert_eq!(alice_history[0].content, "Alice private query");

        let bob_history = state
            .chat_db
            .load_recent_session_async("LineBot".to_string(), "user:U456".to_string(), 10)
            .await;
        assert_eq!(bob_history.len(), 1);
        assert_eq!(bob_history[0].content, "Bob private query");

        let group_history = state
            .chat_db
            .load_recent_session_async("LineBot".to_string(), "group:C789".to_string(), 10)
            .await;
        assert_eq!(group_history.len(), 1);
        assert_eq!(group_history[0].content, "Group topic");

        let main_history = state
            .chat_db
            .load_recent_session_async("LineBot".to_string(), "main".to_string(), 10)
            .await;
        assert_eq!(main_history.len(), 0);

        let sessions = state
            .chat_db
            .list_sessions_for_note_async("LineBot".to_string())
            .await
            .unwrap();
        assert_eq!(
            sessions,
            vec!["main", "group:C789", "user:U123", "user:U456"]
        );
    }

    #[tokio::test]
    async fn test_line_webhook_auto_session_title() {
        let state = create_test_state_with_line("secret123");
        state
            .chat_db
            .create_note("LineBot", "LineBot", None)
            .unwrap();

        let cache = ProfileCache::default();
        cache
            .insert("U12345678".to_string(), "Alice".to_string())
            .await;
        cache
            .insert_group("C12345678".to_string(), "DevOps Team".to_string())
            .await;

        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![
                WebhookEvent {
                    event_type: "message".to_string(),
                    source: Some(EventSource {
                        source_type: "user".to_string(),
                        user_id: Some("U12345678".to_string()),
                        ..Default::default()
                    }),
                    reply_token: Some("dummy1".to_string()),
                    message: Some(EventMessage {
                        id: "msg1".to_string(),
                        message_type: "text".to_string(),
                        text: Some("Hello".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                WebhookEvent {
                    event_type: "message".to_string(),
                    source: Some(EventSource {
                        source_type: "group".to_string(),
                        group_id: Some("C12345678".to_string()),
                        user_id: Some("U12345678".to_string()),
                        ..Default::default()
                    }),
                    reply_token: Some("dummy2".to_string()),
                    message: Some(EventMessage {
                        id: "msg2".to_string(),
                        message_type: "text".to_string(),
                        text: Some("@bot hello group".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        };

        process_webhook_payload(state.clone(), payload, cache).await;

        // Yield to let spawned tasks complete DB inserts
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let meta_list = state
            .chat_db
            .list_chat_sessions_meta_async("LineBot".to_string())
            .await
            .unwrap();

        let user_session = meta_list
            .iter()
            .find(|s| s.session_id == "user:U12345678")
            .unwrap();
        assert_eq!(user_session.title, Some("Alice".to_string()));
        assert_eq!(user_session.custom_title, None);

        let group_session = meta_list
            .iter()
            .find(|s| s.session_id == "group:C12345678")
            .unwrap();
        assert_eq!(group_session.title, Some("DevOps Team".to_string()));
        assert_eq!(group_session.custom_title, None);
    }

    #[tokio::test]
    async fn test_event_logging_with_timezone() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_line("secret123");
        state.data_dir = temp_dir.path().to_path_buf();
        if let Some(ref mut bot) = state.config.notes.line.first_mut() {
            bot.log = true;
            bot.timezone = "+08:00".to_string();
            bot.admins = vec!["U_TZ_ADMIN".to_string()];
        }
        let cache = get_profile_cache().clone();

        // UTC timestamp: 2026-09-24 23:30:00 UTC (1790292600 s = 1790292600000 ms)
        // With +08:00 timezone, local time is 2026-09-25 07:30:00
        let payload = WebhookPayload {
            destination: Some("U_BOT".to_string()),
            events: vec![WebhookEvent {
                event_type: "message".to_string(),
                source: Some(EventSource {
                    source_type: "user".to_string(),
                    user_id: Some("U_TZ_ADMIN".to_string()),
                    ..Default::default()
                }),
                reply_token: Some("dummy".to_string()),
                message: Some(EventMessage {
                    id: "msg_tz".to_string(),
                    message_type: "text".to_string(),
                    text: Some("Good morning Taipei".to_string()),
                    ..Default::default()
                }),
                timestamp: 1790292600000,
                ..Default::default()
            }],
        };

        process_webhook_payload(state.clone(), payload, cache).await;

        let md_dir = state.note_markdown_dir("LineBot");
        let expected_file = md_dir.join("2026-09-25-line-U_TZ_ADMIN.md");
        let wrong_utc_file = md_dir.join("2026-09-24-line-U_TZ_ADMIN.md");

        assert!(
            expected_file.exists(),
            "Expected 2026-09-25-line-U_TZ_ADMIN.md to exist under +08:00 timezone"
        );
        assert!(
            !wrong_utc_file.exists(),
            "Wrong UTC date file 2026-09-24-line-U_TZ_ADMIN.md should not exist"
        );

        let content = std::fs::read_to_string(&expected_file).unwrap();
        assert!(
            content.contains("2026-09-25 07:30:00 +08:00"),
            "Content missing expected formatted datetime: {}",
            content
        );
    }
}
