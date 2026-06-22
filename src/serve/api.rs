//! SSE + REST API handlers for Rune serve.
//!
//! Architecture:
//!   - GET /api/events — SSE stream for server→client push
//!   - POST /api/* — REST endpoints for client→server operations

use crate::agent::{Agent, StopReason};
use crate::config::RuneConfig;
use crate::embedding::EmbeddingEngine;
use crate::loop_engine::LoopModeAdapter;
use crate::provider::{
    CopilotProvider, GeminiProvider, ModelInfo, OpenAiProvider, ProviderRegistry,
};
use crate::serve::ServerState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, RwLock as TokioRwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

// ─── Online user counter ───────────────────────────────────────────────────

static ONLINE_COUNT: AtomicU32 = AtomicU32::new(0);

// ─── Per-note isolation ────────────────────────────────────────────────────

/// Each note gets its own isolated "chat room" with independent SSE channel,
/// model override, and AI task lifecycle.
pub struct NoteRoom {
    pub note_id: String,
    /// Per-note SSE broadcast channel (replaces global broadcast_tx for chat events)
    pub broadcast_tx: broadcast::Sender<String>,
    /// Cancel token for the currently running AI task (Cancel & Replace)
    pub cancel_token: Mutex<Option<CancellationToken>>,
    /// Per-note model override; None = fall back to global default
    pub model_override: TokioRwLock<Option<String>>,
    /// Per-note system prompt override; None = fall back to global default
    pub system_prompt: TokioRwLock<Option<String>>,
    /// Accumulated streaming tokens for the current AI response (cleared on chat_done).
    /// Allows clients reconnecting mid-stream to recover partial output.
    pub streaming_tokens: Arc<TokioRwLock<String>>,
    /// Current AI task status for this room ("idle", "thinking", "typing").
    /// Used by SSE reconnect to restore correct status even when streaming_tokens is empty.
    pub active_status: Arc<TokioRwLock<String>>,
    /// Per-note thinking override; None = fall back to config.thinking
    pub thinking_override: TokioRwLock<Option<String>>,
    pub goal_condition: TokioRwLock<Option<String>>,
    pub goal_status: TokioRwLock<Option<String>>,
    pub goal_model: TokioRwLock<Option<String>>,
}

impl NoteRoom {
    pub fn new(note_id: String) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            note_id,
            broadcast_tx,
            cancel_token: Mutex::new(None),
            model_override: TokioRwLock::new(None),
            system_prompt: TokioRwLock::new(None),
            streaming_tokens: Arc::new(TokioRwLock::new(String::new())),
            active_status: Arc::new(TokioRwLock::new("idle".to_string())),
            thinking_override: TokioRwLock::new(None),
            goal_condition: TokioRwLock::new(None),
            goal_status: TokioRwLock::new(None),
            goal_model: TokioRwLock::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<String>,
}

// ─── SSE Event types (server→client) ──────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum SseMsg {
    #[serde(rename = "chat_token")]
    ChatToken { content: String },
    #[serde(rename = "chat_done")]
    ChatDone {},
    #[serde(rename = "chat_meta")]
    ChatMeta {
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        tokens_in: u32,
        tokens_out: u32,
        context_tokens: u32,
        context_window: u32,
        steps: u32,
        tool_calls: u32,
    },
    #[serde(rename = "chat_message")]
    ChatMessage { nickname: String, content: String },
    #[serde(rename = "status")]
    Status { state: String },
    #[serde(rename = "tool_status")]
    ToolStatus { tool: String, state: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "users_update")]
    UsersUpdate { count: u32 },
    #[serde(rename = "history")]
    History {
        messages: Vec<crate::serve::db::ChatRecord>,
    },
    #[serde(rename = "auth_result")]
    AuthResult {
        ok: bool,
        is_admin: bool,
        is_guest: bool,
        login: String,
    },
    #[serde(rename = "file_list")]
    FileList {
        files: Vec<FileEntry>,
        active: String,
    },
    #[serde(rename = "file_content")]
    FileContent {
        note_id: String,
        filename: String,
        content: String,
    },
    #[serde(rename = "file_deleted")]
    FileDeleted { filename: String },
    #[serde(rename = "note_list")]
    NoteList {
        notes: Vec<NoteListEntry>,
        active: String,
    },
    #[serde(rename = "note_switched")]
    NoteSwitched { note_id: String },
    #[serde(rename = "model_list")]
    ModelList {
        models: Vec<ModelListEntry>,
        active: String,
        thinking: String,
    },
    #[serde(rename = "model_changed")]
    ModelChanged { model: String, thinking: String },
    #[serde(rename = "thinking_changed")]
    ThinkingChanged { thinking: String },
    #[serde(rename = "approval_request")]
    ApprovalRequest { id: String, detail: String },
    #[serde(rename = "archive_done")]
    ArchiveDone { filename: String, count: usize },
    #[serde(rename = "search_results")]
    SearchResults {
        query: String,
        results: Vec<crate::serve::db::ChatRecord>,
    },
    #[serde(rename = "dir_browse_result")]
    DirBrowseResult {
        path: String,
        parent: Option<String>,
        entries: Vec<DirEntry>,
    },
    #[serde(rename = "loop_iteration")]
    LoopIteration {
        iteration: u32,
        record: crate::loop_engine::state::IterationRecord,
    },
    #[serde(rename = "loop_done")]
    LoopDone { status: String, output: String },
    #[serde(rename = "goal_status")]
    GoalStatus {
        note_id: String,
        condition: Option<String>,
        status: Option<String>,
        model: Option<String>,
    },
    #[serde(rename = "goal_achieved")]
    GoalAchieved { output: String },
}

#[derive(Debug, Serialize, Clone)]
pub struct NoteListEntry {
    pub id: String,
    pub name: String,
    pub files: Vec<String>,
    pub public: bool,
    /// Which files are publicly visible (only names that are public=true)
    pub public_files: Vec<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct FileEntry {
    pub name: String,
    pub public: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

// ─── Request types (client→server) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Optional nickname override (falls back to GitHub login from session).
    pub nickname: Option<String>,
    /// Required: the note to subscribe to.
    pub note_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatReq {
    pub note_id: String,
    pub content: String,
    pub nickname: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoalSetReq {
    pub note_id: String,
    pub condition: String,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoalClearReq {
    pub note_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatCancelReq {
    pub note_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FileCreateReq {
    pub note_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileDeleteReq {
    pub note_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileRenameReq {
    pub note_id: String,
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileSwitchReq {
    pub note_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileUpdateReq {
    pub note_id: String,
    pub filename: Option<String>,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct NoteCreateReq {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct NoteRenameReq {
    pub note_id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NoteDeleteReq {
    pub note_id: String,
}

#[derive(Debug, Deserialize)]
pub struct NoteSwitchReq {
    pub note_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelSwitchReq {
    pub model: String,
    /// If provided, sets per-note override; otherwise sets global default.
    pub note_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ThinkingSwitchReq {
    pub note_id: String,
    pub thinking: String, // "off"|"low"|"medium"|"high"
}

#[derive(Debug, Deserialize)]
pub struct ArchiveReq {
    pub note_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchReq {
    pub note_id: String,
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct ApprovalReq {
    pub id: String,
    pub approved: bool,
    /// Route approval response through the note's room channel.
    pub note_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SystemPromptReq {
    pub note_id: String,
    /// If None/empty, clears the per-note override (falls back to global).
    pub prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NoteVisibilityReq {
    pub note_id: String,
    pub public: bool,
}

#[derive(Debug, Deserialize)]
pub struct FileVisibilityReq {
    pub note_id: String,
    pub filename: String,
    pub public: bool,
}

#[derive(Debug, Deserialize)]
pub struct DirBrowseReq {
    pub path: String,
}

// ─── Response type ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ApiResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ApiResponse {
    pub fn success() -> Self {
        Self {
            ok: true,
            error: None,
            data: None,
        }
    }
    pub fn with_data(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: None,
        }
    }
}

// Auth functions are now handled by the OAuth session middleware in serve/mod.rs
// and the session lookup in events_handler below.
// See src/serve/oauth.rs for role resolution logic.

pub fn is_valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".md")
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

// ─── Helper: build session list ────────────────────────────────────────────

pub async fn build_note_list(state: &ServerState) -> Vec<NoteListEntry> {
    let notes = state.chat_db.list_notes().unwrap_or_default();
    let mut entries = Vec::new();
    for s in notes {
        let md_dir = state.note_markdown_dir(&s.id);
        let mut files = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    files.push(name);
                }
            }
        }
        files.sort();
        let public_files = state.chat_db.list_public_files(&s.id);
        entries.push(NoteListEntry {
            id: s.id.clone(),
            name: s.name.clone(),
            files,
            public: s.public,
            public_files,
            icon: s.icon.clone(),
        });
    }
    entries
}

/// Broadcast a message to a specific note room only.
/// Subscribers to other rooms will NOT receive this message.
pub fn broadcast_to_room(room: &NoteRoom, msg: &SseMsg) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = room.broadcast_tx.send(json);
    }
}

// ─── SSE endpoint ──────────────────────────────────────────────────────────

pub async fn events_handler(
    State(state): State<ServerState>,
    Query(params): Query<EventsQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Resolve session from HttpOnly cookie
    let sid = crate::serve::oauth::get_cookie(&headers, "rune_sid");
    let session = match sid {
        Some(ref id) => state.sessions.get(id).await,
        None => None,
    };

    // Auth check — must have a valid session
    let session = match session {
        Some(s) => s,
        None => {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(
                    Event::default()
                        .event("auth_error")
                        .data(r#"{"type":"auth_error","message":"Authentication required. Please sign in via GitHub OAuth."}"#.to_string()),
                )
            });
            return Sse::new(err_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }
    };

    let is_admin = session.is_admin();
    let is_guest = session.is_guest();
    let login = session.login.clone();
    // Nickname: prefer explicit param, fall back to GitHub login
    let nickname = params.nickname.unwrap_or_else(|| login.clone());

    // note_id is REQUIRED per spec
    let note_id = match params.note_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(
                    Event::default()
                        .event("error")
                        .data(r#"{"type":"error","message":"note_id is required"}"#.to_string()),
                )
            });
            return Sse::new(err_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }
    };

    let notes = build_note_list(&state).await;

    // Verify note exists
    if !notes.iter().any(|n| n.id == note_id) {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data(r#"{"type":"error","message":"Note not found"}"#.to_string()),
            )
        });
        return Sse::new(err_stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // Guest + private note check
    if is_guest {
        let note_public = notes
            .iter()
            .find(|n| n.id == note_id)
            .map(|n| n.public)
            .unwrap_or(false);
        if !note_public {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(
                    Event::default().event("auth_error").data(
                        r#"{"type":"auth_error","message":"Guests cannot access private notes"}"#
                            .to_string(),
                    ),
                )
            });
            return Sse::new(err_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
        }
    }

    // Subscribe to room channel
    let room = state.get_or_create_room(&note_id).await;
    let mut room_rx = room.broadcast_tx.subscribe();

    // Increment online count
    let count = ONLINE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

    // Build initial messages to send
    let mut init_msgs = Vec::new();

    // Auth result
    init_msgs.push(SseMsg::AuthResult {
        ok: true,
        is_admin,
        is_guest,
        login: login.clone(),
    });

    // Model list — show effective model and metadata for this note
    let effective = state.effective_model(&note_id).await;
    let thinking = state
        .effective_thinking(&note_id)
        .await
        .unwrap_or_else(|| "off".to_string());
    let model_entries: Vec<ModelListEntry> = state
        .models
        .read()
        .await
        .iter()
        .map(|m| ModelListEntry {
            id: m.id.clone(),
            provider: m.provider.clone(),
            context_window: m.context_window,
            reasoning_efforts: m.reasoning_efforts.clone(),
        })
        .collect();
    init_msgs.push(SseMsg::ModelList {
        models: model_entries,
        active: effective,
        thinking,
    });

    // Note list — guests only see public notes
    let visible_notes = if is_guest {
        notes.into_iter().filter(|n| n.public).collect()
    } else {
        notes
    };
    init_msgs.push(SseMsg::NoteList {
        notes: visible_notes,
        active: note_id.clone(),
    });

    // Users update (not sent to guests per spec)
    if !is_guest {
        init_msgs.push(SseMsg::UsersUpdate { count });
    }

    // Goal status update
    {
        let condition = room.goal_condition.read().await.clone();
        let status = room.goal_status.read().await.clone();
        let model = room.goal_model.read().await.clone();
        init_msgs.push(SseMsg::GoalStatus {
            note_id: note_id.clone(),
            condition,
            status,
            model,
        });
    }

    // Streaming recovery: restore AI task status on reconnect
    {
        let status = room.active_status.read().await;
        if *status != "idle" {
            // Room has an active AI task — send current status
            if let Some(tool_name) = status.strip_prefix("tool:") {
                // Currently executing a tool — send thinking + tool_status
                init_msgs.push(SseMsg::Status {
                    state: "thinking".to_string(),
                });
                init_msgs.push(SseMsg::ToolStatus {
                    tool: tool_name.to_string(),
                    state: "start".to_string(),
                });
            } else {
                init_msgs.push(SseMsg::Status {
                    state: status.clone(),
                });
            }
            // If there are accumulated tokens, send them for partial display
            let buf = room.streaming_tokens.read().await;
            if !buf.is_empty() {
                init_msgs.push(SseMsg::ChatToken {
                    content: buf.clone(),
                });
            }
        }
    }

    // System join message — broadcast to the room
    let join_msg = SseMsg::System {
        content: format!("{} joined", nickname),
    };
    broadcast_to_room(&room, &join_msg);

    let nickname_clone = nickname.clone();
    let state_clone = state.clone();
    let room_clone = Arc::clone(&room);

    let stream = async_stream::stream! {
        // Send initial messages
        for msg in init_msgs {
            if let Ok(json) = serde_json::to_string(&msg) {
                let event_type = extract_event_type(&json);
                yield Ok::<_, Infallible>(Event::default().event(event_type).data(json));
            }
        }

        // Stream room-specific broadcast messages
        loop {
            match room_rx.recv().await {
                Ok(json) => {
                    let event_type = extract_event_type(&json);
                    // Guest filter: skip events not in allowlist
                    if is_guest && !is_guest_allowed_event(&event_type) {
                        continue;
                    }
                    yield Ok::<_, Infallible>(Event::default().event(event_type).data(json));
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("SSE client lagged {} messages", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }

        // Client disconnected — decrement count
        let count = ONLINE_COUNT.fetch_sub(1, Ordering::Relaxed) - 1;
        let leave_msg = SseMsg::System { content: format!("{} left", nickname_clone) };
        broadcast_to_room(&room_clone, &leave_msg);
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Check if an event type is allowed for guest users.
pub fn is_guest_allowed_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "chat_token"
            | "chat_done"
            | "chat_message"
            | "chat_meta"
            | "file_content"
            | "file_list"
            | "note_list"
            | "auth_result"
            | "model_list"
            | "model_changed"
    )
}

/// Extract event type from JSON (reads "type" field).
fn extract_event_type(json: &str) -> String {
    // Quick extraction without full parse
    if let Some(start) = json.find(r#""type":""#) {
        let rest = &json[start + 8..];
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    "message".to_string()
}

// ─── POST handlers ─────────────────────────────────────────────────────────

pub async fn chat_handler(
    State(state): State<ServerState>,
    Json(req): Json<ChatReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    if req.content.trim().is_empty() {
        return Json(ApiResponse::err("Empty message"));
    }

    let preview: String = req.content.chars().take(50).collect();
    info!("Chat message: {}", preview);

    // Get (or create) the room for this note
    let room = state.get_or_create_room(&req.note_id).await;

    // Broadcast user message to the room
    let nickname = req.nickname.clone().unwrap_or_else(|| "user".to_string());
    let user_msg = SseMsg::ChatMessage {
        nickname: nickname.clone(),
        content: req.content.clone(),
    };
    broadcast_to_room(&room, &user_msg);

    // Persist user message
    state
        .chat_db
        .insert_async(
            req.note_id.clone(),
            "user".to_string(),
            nickname,
            req.content.clone(),
        )
        .await;

    // Send thinking status to the room
    let thinking = SseMsg::Status {
        state: "thinking".to_string(),
    };
    broadcast_to_room(&room, &thinking);

    // Mark room as active (for SSE reconnect recovery)
    {
        let mut status = room.active_status.write().await;
        *status = "thinking".to_string();
    }

    // Cancel & Replace: cancel any existing AI task for this note
    let new_token = CancellationToken::new();
    {
        let mut guard = room.cancel_token.lock().unwrap();
        if let Some(old) = guard.replace(new_token.clone()) {
            old.cancel();
        }
    }
    // Clear accumulated tokens from the cancelled task
    {
        let mut buf = room.streaming_tokens.write().await;
        buf.clear();
    }

    // Spawn agent task with cancellation
    let state_clone = state.clone();
    let note_id = req.note_id.clone();
    let content = req.content.clone();
    let nick = req.nickname.clone().unwrap_or_else(|| "user".to_string());
    let cancel = new_token.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Silently exit — new message replaced this one
            }
            _ = handle_chat_message(content, state_clone, note_id, nick) => {}
        }
    });

    Json(ApiResponse::success())
}

pub async fn file_create_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileCreateReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    if !is_valid_filename(&req.name) {
        return Json(ApiResponse::err(format!("Invalid filename: {}", req.name)));
    }

    let md_dir = state.note_markdown_dir(&req.note_id);
    let file_path = md_dir.join(&req.name);
    if file_path.exists() {
        return Json(ApiResponse::err(format!(
            "File already exists: {}",
            req.name
        )));
    }

    let empty = format!("# {}\n\n", req.name.trim_end_matches(".md"));
    let _ = tokio::fs::create_dir_all(&md_dir).await;
    if let Err(e) = tokio::fs::write(&file_path, &empty).await {
        return Json(ApiResponse::err(format!("Failed to create file: {}", e)));
    }

    // Broadcast updated file list
    broadcast_file_list(&state, &req.note_id).await;

    // Broadcast file content to the room
    let room = state.get_or_create_room(&req.note_id).await;
    let fc = SseMsg::FileContent {
        note_id: req.note_id.clone(),
        filename: req.name,
        content: empty,
    };
    broadcast_to_room(&room, &fc);

    Json(ApiResponse::success())
}

pub async fn file_delete_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileDeleteReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }

    let md_dir = state.note_markdown_dir(&req.note_id);
    let file_path = md_dir.join(&req.name);
    tokio::fs::remove_file(&file_path).await.ok();

    let room = state.get_or_create_room(&req.note_id).await;
    let del = SseMsg::FileDeleted { filename: req.name };
    broadcast_to_room(&room, &del);
    broadcast_file_list(&state, &req.note_id).await;

    Json(ApiResponse::success())
}

pub async fn file_rename_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileRenameReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    if !is_valid_filename(&req.new_name) {
        return Json(ApiResponse::err(format!(
            "Invalid filename: {}",
            req.new_name
        )));
    }

    let md_dir = state.note_markdown_dir(&req.note_id);
    let new_path = md_dir.join(&req.new_name);
    if new_path.exists() {
        return Json(ApiResponse::err(format!(
            "File already exists: {}",
            req.new_name
        )));
    }

    let old_path = md_dir.join(&req.old_name);
    if old_path.exists() {
        tokio::fs::rename(&old_path, &new_path).await.ok();
    }

    broadcast_file_list(&state, &req.note_id).await;
    Json(ApiResponse::success())
}

pub async fn file_switch_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileSwitchReq>,
) -> Json<serde_json::Value> {
    if req.note_id.is_empty() {
        return Json(serde_json::json!({ "ok": false, "error": "No note selected" }));
    }

    let file_path = state.note_markdown_dir(&req.note_id).join(&req.name);
    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => {
            // Per-client action: return content via HTTP only, no broadcast.
            // SSE file_content is reserved for actual file mutations (update/create).
            Json(serde_json::json!({ "ok": true, "content": content, "filename": req.name }))
        }
        Err(e) => {
            tracing::warn!(
                "file/switch failed: note_id={:?} name={:?} path={:?} err={}",
                req.note_id,
                req.name,
                file_path,
                e
            );
            Json(
                serde_json::json!({ "ok": false, "error": format!("File not found: {}", req.name) }),
            )
        }
    }
}

pub async fn file_update_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileUpdateReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    let fname = req.filename.unwrap_or_default();
    if fname.is_empty() || !is_valid_filename(&fname) {
        return Json(ApiResponse::err("Invalid or missing filename"));
    }

    let file_path = state.note_markdown_dir(&req.note_id).join(&fname);
    if let Err(e) = tokio::fs::write(&file_path, &req.content).await {
        return Json(ApiResponse::err(format!("Failed to write: {}", e)));
    }

    let room = state.get_or_create_room(&req.note_id).await;
    let fc = SseMsg::FileContent {
        note_id: req.note_id.clone(),
        filename: fname,
        content: req.content,
    };
    broadcast_to_room(&room, &fc);
    Json(ApiResponse::success())
}

pub async fn note_create_handler(
    State(state): State<ServerState>,
    Json(req): Json<NoteCreateReq>,
) -> Json<ApiResponse> {
    if req.name.is_empty() {
        return Json(ApiResponse::err("Note name required"));
    }

    // Persist DB on first session creation
    if let Err(e) = state.chat_db.ensure_persistent() {
        warn!("Failed to persist DB: {}", e);
    }

    let id = req.name.clone();
    match state.chat_db.create_note(&id, &req.name, None) {
        Ok(_) => {
            info!("Note '{}' created", id);
            let md_dir = state.note_markdown_dir(&id);
            let _ = tokio::fs::create_dir_all(&md_dir).await;
            broadcast_note_list(&state).await;
            Json(ApiResponse::success())
        }
        Err(e) => Json(ApiResponse::err(format!("Failed to create note: {}", e))),
    }
}

pub async fn note_rename_handler(
    State(state): State<ServerState>,
    Json(req): Json<NoteRenameReq>,
) -> Json<ApiResponse> {
    match state
        .chat_db
        .rename_note(&req.note_id, &req.name, req.icon.as_deref())
    {
        Ok(Some(new_id)) => {
            let old_dir = state.data_dir.join("notes").join(&req.note_id);
            let new_dir = state.data_dir.join("notes").join(&new_id);
            if old_dir.exists() && old_dir != new_dir {
                if !new_dir.exists() {
                    // Simple rename: no target directory, just mv
                    let _ = tokio::fs::rename(&old_dir, &new_dir).await;
                } else {
                    // Merge: move archives and markdown files into existing target directory
                    merge_note_dirs(&old_dir, &new_dir).await;
                    let _ = tokio::fs::remove_dir_all(&old_dir).await;
                }
            }
            broadcast_note_list(&state).await;
            Json(ApiResponse::success())
        }
        Ok(None) => Json(ApiResponse::err("Note not found")),
        Err(e) => Json(ApiResponse::err(format!("Failed: {}", e))),
    }
}

/// Merge contents of `src` note directory into `dst`.
/// - archives/*.jsonl  → moved as-is (no filename collision expected, named by timestamp)
/// - markdown/*        → moved; on name collision, src file renamed to <stem>.from-<src_note>.md
async fn merge_note_dirs(src: &std::path::Path, dst: &std::path::Path) {
    let src_note = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    for subdir in &["archives", "markdown"] {
        let src_sub = src.join(subdir);
        let dst_sub = dst.join(subdir);
        if !src_sub.exists() {
            continue;
        }
        let _ = tokio::fs::create_dir_all(&dst_sub).await;
        let mut rd = match tokio::fs::read_dir(&src_sub).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let fname = entry.file_name();
            let dst_path = dst_sub.join(&fname);
            let src_path = src_sub.join(&fname);
            if dst_path.exists() {
                // Conflict: rename src file to <stem>.from-<src_note>.<ext>
                let fname_str = fname.to_string_lossy();
                let (stem, ext) = if let Some(dot) = fname_str.rfind('.') {
                    (&fname_str[..dot], &fname_str[dot..])
                } else {
                    (fname_str.as_ref(), "")
                };
                let new_name = format!("{}.from-{}{}", stem, src_note, ext);
                let _ = tokio::fs::rename(&src_path, dst_sub.join(&new_name)).await;
            } else {
                let _ = tokio::fs::rename(&src_path, &dst_path).await;
            }
        }
    }
}

pub async fn note_delete_handler(
    State(state): State<ServerState>,
    Json(req): Json<NoteDeleteReq>,
) -> Json<ApiResponse> {
    match state.chat_db.delete_note(&req.note_id) {
        Ok(true) => {
            // Cancel any running AI task in the room, then remove the room
            {
                let rooms = state.rooms.read().await;
                if let Some(room) = rooms.get(&req.note_id) {
                    let guard = room.cancel_token.lock().unwrap();
                    if let Some(ref token) = *guard {
                        token.cancel();
                    }
                }
            }
            // Remove room from map
            state.rooms.write().await.remove(&req.note_id);

            // Remove the note directory if markdown/ is empty (or absent)
            let note_dir = state.data_dir.join("notes").join(&req.note_id);
            let md_dir = note_dir.join("markdown");
            let md_empty = {
                match tokio::fs::read_dir(&md_dir).await {
                    Err(_) => true,
                    Ok(mut rd) => rd.next_entry().await.unwrap_or(None).is_none(),
                }
            };
            if md_empty {
                let _ = tokio::fs::remove_dir_all(&note_dir).await;
            }
            broadcast_note_list(&state).await;
            Json(ApiResponse::success())
        }
        Ok(false) => Json(ApiResponse::err("Note not found")),
        Err(e) => Json(ApiResponse::err(format!("Failed: {}", e))),
    }
}

pub async fn note_switch_handler(
    State(state): State<ServerState>,
    Json(req): Json<NoteSwitchReq>,
) -> Json<serde_json::Value> {
    // Load history for response (per-client, not broadcast)
    let history = state
        .chat_db
        .load_recent_async(req.note_id.clone(), 100)
        .await;

    // Load file list
    let md_dir = state.note_markdown_dir(&req.note_id);
    let mut files = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                files.push(name);
            }
        }
    }
    files.sort();

    // Load first file content
    let first_file = files.first().cloned();
    let first_content = if let Some(ref fname) = first_file {
        tokio::fs::read_to_string(md_dir.join(fname)).await.ok()
    } else {
        None
    };

    // Return all data in response (no broadcast — switch is per-client)
    let current_model = state.effective_model(&req.note_id).await;
    Json(serde_json::json!({
        "ok": true,
        "note_id": req.note_id,
        "history": history,
        "files": files,
        "current_file": first_file,
        "file_content": first_content,
        "current_model": current_model,
    }))
}

pub async fn model_switch_handler(
    State(state): State<ServerState>,
    Json(req): Json<ModelSwitchReq>,
) -> Json<ApiResponse> {
    let models = state.models.read().await;
    let new_model_info = models.iter().find(|m| m.id == req.model);
    if new_model_info.is_none() {
        return Json(ApiResponse::err(format!("Unknown model: {}", req.model)));
    }
    let new_efforts = new_model_info.unwrap().reasoning_efforts.clone();
    drop(models);

    if let Some(ref note_id) = req.note_id {
        let room = state.get_or_create_room(note_id).await;
        *room.model_override.write().await = Some(req.model.clone());
        let _ = state.chat_db.set_note_model(note_id, Some(&req.model));

        // Thinking fallback: use effective thinking (per-note override > config), check if supported
        let current_effective = state.effective_thinking(note_id).await;
        let effective_thinking = if let Some(ref t) = current_effective {
            if t == "off" || new_efforts.contains(t) {
                t.clone()
            } else {
                // Not supported by new model — fallback to off
                *room.thinking_override.write().await = Some("off".to_string());
                state.chat_db.set_note_thinking(note_id, Some("off"));
                "off".to_string()
            }
        } else {
            "off".to_string()
        };

        let msg = SseMsg::ModelChanged {
            model: req.model.clone(),
            thinking: effective_thinking,
        };
        broadcast_to_room(&room, &msg);
    } else {
        // Global default model
        *state.global_default_model.write().await = req.model.clone();
        let rooms = state.rooms.read().await;
        for room in rooms.values() {
            let current_effective = state.effective_thinking(&room.note_id).await;
            let effective_thinking = if let Some(ref t) = current_effective {
                if t == "off" || new_efforts.contains(t) {
                    t.clone()
                } else {
                    *room.thinking_override.write().await = Some("off".to_string());
                    state.chat_db.set_note_thinking(&room.note_id, Some("off"));
                    "off".to_string()
                }
            } else {
                "off".to_string()
            };
            let msg = SseMsg::ModelChanged {
                model: req.model.clone(),
                thinking: effective_thinking,
            };
            broadcast_to_room(room, &msg);
        }
    }
    Json(ApiResponse::success())
}

pub async fn thinking_switch_handler(
    State(state): State<ServerState>,
    Json(req): Json<ThinkingSwitchReq>,
) -> Json<serde_json::Value> {
    let room = state.get_or_create_room(&req.note_id).await;
    let thinking_val = Some(req.thinking.clone());

    // Update room
    *room.thinking_override.write().await = thinking_val.clone();

    // Persist to DB
    state
        .chat_db
        .set_note_thinking(&req.note_id, thinking_val.as_deref());

    // Broadcast to room
    let msg = SseMsg::ThinkingChanged {
        thinking: req.thinking.clone(),
    };
    broadcast_to_room(&room, &msg);

    Json(serde_json::json!({"ok": true, "thinking": req.thinking}))
}

pub async fn archive_handler(
    State(state): State<ServerState>,
    Json(req): Json<ArchiveReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    let archive_dir = state
        .note_markdown_dir(&req.note_id)
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
    match db.archive_async(req.note_id.clone(), archive_path).await {
        Ok(count) => {
            let room = state.get_or_create_room(&req.note_id).await;
            let msg = SseMsg::ArchiveDone { filename, count };
            broadcast_to_room(&room, &msg);
            // Send empty history to the room
            let hist = SseMsg::History { messages: vec![] };
            broadcast_to_room(&room, &hist);
            Json(ApiResponse::success())
        }
        Err(e) => Json(ApiResponse::err(format!("Archive failed: {}", e))),
    }
}

pub async fn search_handler(
    State(state): State<ServerState>,
    Json(req): Json<SearchReq>,
) -> Json<ApiResponse> {
    if req.query.is_empty() {
        return Json(ApiResponse::err("Empty query"));
    }
    let archive_dir = state
        .note_markdown_dir(&req.note_id)
        .parent()
        .unwrap()
        .join("archives");
    let results = state
        .chat_db
        .search_async(req.note_id.clone(), req.query.clone(), archive_dir)
        .await;
    let room = state.get_or_create_room(&req.note_id).await;
    let msg = SseMsg::SearchResults {
        query: req.query,
        results,
    };
    broadcast_to_room(&room, &msg);
    Json(ApiResponse::success())
}

pub async fn approval_handler(
    State(state): State<ServerState>,
    Json(req): Json<ApprovalReq>,
) -> Json<ApiResponse> {
    let msg = if req.approved {
        format!("__approval_granted__{}", req.id)
    } else {
        format!("__approval_denied__{}", req.id)
    };
    // Route approval response through the note's room channel
    if let Some(ref note_id) = req.note_id {
        let room = state.get_or_create_room(note_id).await;
        let _ = room.broadcast_tx.send(msg);
    } else {
        // Fallback: broadcast to all rooms (shouldn't happen in normal flow)
        let rooms = state.rooms.read().await;
        for room in rooms.values() {
            let _ = room.broadcast_tx.send(msg.clone());
        }
    }
    Json(ApiResponse::success())
}

pub async fn system_prompt_handler(
    State(state): State<ServerState>,
    Json(req): Json<SystemPromptReq>,
) -> Json<serde_json::Value> {
    if req.note_id.is_empty() {
        return Json(serde_json::json!({ "ok": false, "error": "note_id required" }));
    }
    let room = state.get_or_create_room(&req.note_id).await;
    match req.prompt {
        Some(ref p) if !p.is_empty() => {
            *room.system_prompt.write().await = Some(p.clone());
        }
        _ => {
            *room.system_prompt.write().await = None;
        }
    }
    let current = room.system_prompt.read().await.clone();
    Json(serde_json::json!({
        "ok": true,
        "note_id": req.note_id,
        "system_prompt": current,
    }))
}

pub async fn system_prompt_get_handler(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let note_id = params.get("note_id").cloned().unwrap_or_default();
    if note_id.is_empty() {
        // Return global system prompt
        let global = build_system_prompt(&state.config).await;
        return Json(serde_json::json!({
            "ok": true,
            "note_id": null,
            "system_prompt": global,
            "is_override": false,
        }));
    }
    let room = state.get_or_create_room(&note_id).await;
    let override_prompt = room.system_prompt.read().await.clone();
    let effective = if let Some(ref p) = override_prompt {
        p.clone()
    } else {
        build_system_prompt(&state.config).await
    };
    Json(serde_json::json!({
        "ok": true,
        "note_id": note_id,
        "system_prompt": effective,
        "is_override": override_prompt.is_some(),
    }))
}

pub async fn dir_browse_handler(
    State(_state): State<ServerState>,
    Json(req): Json<DirBrowseReq>,
) -> Json<ApiResponse> {
    let browse_path = std::path::Path::new(&req.path);
    let canonical = tokio::fs::canonicalize(browse_path)
        .await
        .unwrap_or_else(|_| browse_path.to_path_buf());
    let parent = canonical.parent().map(|p| p.to_string_lossy().to_string());
    let mut entries = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&canonical).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if let Ok(meta) = entry.metadata().await {
                if meta.is_dir() {
                    entries.push(DirEntry { name, is_dir: true });
                }
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let data = serde_json::to_value(SseMsg::DirBrowseResult {
        path: canonical.to_string_lossy().to_string(),
        parent,
        entries,
    })
    .unwrap_or_default();

    Json(ApiResponse::with_data(data))
}

// ─── Helpers ───────────────────────────────────────────────────────────────

pub async fn note_visibility_handler(
    State(state): State<ServerState>,
    Json(req): Json<NoteVisibilityReq>,
) -> Json<ApiResponse> {
    match state.chat_db.set_note_public(&req.note_id, req.public) {
        Ok(_) => {
            broadcast_note_list(&state).await;
            Json(ApiResponse::success())
        }
        Err(e) => Json(ApiResponse::err(format!("Failed: {}", e))),
    }
}

pub async fn file_visibility_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileVisibilityReq>,
) -> Json<ApiResponse> {
    match state
        .chat_db
        .set_file_public(&req.note_id, &req.filename, req.public)
    {
        Ok(_) => {
            broadcast_file_list(&state, &req.note_id).await;
            Json(ApiResponse::success())
        }
        Err(e) => Json(ApiResponse::err(format!("Failed: {}", e))),
    }
}

struct NoteLoopAdapter {
    note_id: String,
    state: ServerState,
    cancel_token: CancellationToken,
}

impl crate::loop_engine::LoopModeAdapter for NoteLoopAdapter {
    fn on_loop_start(&self, _loop_id: &str, _goal: &str) {
        let note_id = self.note_id.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let room = state.get_or_create_room(&note_id).await;
            {
                let mut status_guard = room.goal_status.write().await;
                *status_guard = Some("Running".to_string());
            }
            let condition = room.goal_condition.read().await.clone();
            let status = room.goal_status.read().await.clone();
            let model = room.goal_model.read().await.clone();
            broadcast_to_room(
                &room,
                &SseMsg::GoalStatus {
                    note_id,
                    condition,
                    status,
                    model,
                },
            );
        });
    }

    fn on_iteration_start(&self, _iteration: u32, _max_iterations: u32) {}

    fn on_iteration_complete(
        &self,
        iteration: u32,
        record: &crate::loop_engine::state::IterationRecord,
    ) {
        let note_id = self.note_id.clone();
        let state = self.state.clone();
        let record = record.clone();
        tokio::spawn(async move {
            let room = state.get_or_create_room(&note_id).await;
            broadcast_to_room(&room, &SseMsg::LoopIteration { iteration, record });
        });
    }

    fn check_cancellation(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    fn request_human_input<'a>(
        &'a self,
        _prompt: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

pub async fn goal_set_handler(
    State(state): State<ServerState>,
    Json(req): Json<GoalSetReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    if req.condition.trim().is_empty() {
        return Json(ApiResponse::err("Empty goal condition"));
    }

    let room = state.get_or_create_room(&req.note_id).await;

    // Update goal fields on room
    {
        *room.goal_condition.write().await = Some(req.condition.clone());
        *room.goal_status.write().await = Some("Running".to_string());
        *room.goal_model.write().await = req.model.clone();
    }

    // Cancel any running task in the room
    let new_token = CancellationToken::new();
    {
        let mut guard = room.cancel_token.lock().unwrap();
        if let Some(old) = guard.replace(new_token.clone()) {
            old.cancel();
        }
    }
    {
        let mut buf = room.streaming_tokens.write().await;
        buf.clear();
    }

    // Broadcast the new goal status to the room
    broadcast_to_room(
        &room,
        &SseMsg::GoalStatus {
            note_id: req.note_id.clone(),
            condition: Some(req.condition.clone()),
            status: Some("Running".to_string()),
            model: req.model.clone(),
        },
    );

    // Send thinking status to the room
    let thinking = SseMsg::Status {
        state: "thinking".to_string(),
    };
    broadcast_to_room(&room, &thinking);

    // Mark room as active (for SSE reconnect recovery)
    {
        let mut status = room.active_status.write().await;
        *status = "thinking".to_string();
    }

    // Spawn the LoopEngine run in the background
    let state_clone = state.clone();
    let note_id = req.note_id.clone();
    let condition = req.condition.clone();
    let cancel = new_token.clone();
    let model_override = req.model.clone();

    tokio::spawn(async move {
        let repo_path = match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                let err_msg = format!("Failed to get current directory: {}", e);
                let room = state_clone.get_or_create_room(&note_id).await;
                {
                    *room.goal_status.write().await = Some("Failed".to_string());
                    let mut status_guard = room.active_status.write().await;
                    *status_guard = "idle".to_string();
                }
                broadcast_to_room(
                    &room,
                    &SseMsg::GoalStatus {
                        note_id: note_id.clone(),
                        condition: Some(condition),
                        status: Some("Failed".to_string()),
                        model: model_override,
                    },
                );
                broadcast_to_room(&room, &SseMsg::Error { message: err_msg });
                broadcast_to_room(
                    &room,
                    &SseMsg::Status {
                        state: "idle".to_string(),
                    },
                );
                return;
            }
        };

        // Create the LoopEngine
        let loops_dir = state_clone.data_dir.join("loops");
        let mut loop_cfg = state_clone.config.clone();
        if let Some(ref m) = model_override {
            loop_cfg.model = m.clone();
        }

        let engine = crate::loop_engine::LoopEngine::new(loop_cfg, loops_dir);
        let adapter = NoteLoopAdapter {
            note_id: note_id.clone(),
            state: state_clone.clone(),
            cancel_token: cancel,
        };

        let run_result = engine
            .run_loop(&note_id, &condition, &repo_path, &adapter)
            .await;

        let room = state_clone.get_or_create_room(&note_id).await;
        {
            let mut status_guard = room.active_status.write().await;
            *status_guard = "idle".to_string();
        }
        broadcast_to_room(
            &room,
            &SseMsg::Status {
                state: "idle".to_string(),
            },
        );

        match run_result {
            Ok(output) => {
                {
                    *room.goal_status.write().await = Some("Complete".to_string());
                }
                let condition = room.goal_condition.read().await.clone();
                let model = room.goal_model.read().await.clone();
                broadcast_to_room(
                    &room,
                    &SseMsg::GoalStatus {
                        note_id: note_id.clone(),
                        condition,
                        status: Some("Complete".to_string()),
                        model,
                    },
                );
                broadcast_to_room(
                    &room,
                    &SseMsg::GoalAchieved {
                        output: output.clone(),
                    },
                );
                broadcast_to_room(
                    &room,
                    &SseMsg::LoopDone {
                        status: "goal".to_string(),
                        output,
                    },
                );
            }
            Err(e) => {
                let status_str = if adapter.check_cancellation() {
                    "Paused".to_string()
                } else {
                    "Failed".to_string()
                };
                {
                    *room.goal_status.write().await = Some(status_str.clone());
                }
                let condition = room.goal_condition.read().await.clone();
                let model = room.goal_model.read().await.clone();
                broadcast_to_room(
                    &room,
                    &SseMsg::GoalStatus {
                        note_id: note_id.clone(),
                        condition,
                        status: Some(status_str.clone()),
                        model,
                    },
                );
                let err_msg = format!("Goal loop error: {}", e);
                broadcast_to_room(
                    &room,
                    &SseMsg::Error {
                        message: err_msg.clone(),
                    },
                );
                broadcast_to_room(
                    &room,
                    &SseMsg::LoopDone {
                        status: if status_str == "Paused" {
                            "cancel".to_string()
                        } else {
                            "error".to_string()
                        },
                        output: err_msg,
                    },
                );
            }
        }
    });

    Json(ApiResponse::success())
}

pub async fn goal_clear_handler(
    State(state): State<ServerState>,
    Json(req): Json<GoalClearReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }

    let room = state.get_or_create_room(&req.note_id).await;

    // Cancel current task
    {
        let mut guard = room.cancel_token.lock().unwrap();
        if let Some(old) = guard.take() {
            old.cancel();
        }
    }

    // Reset active status to idle
    {
        let mut status_guard = room.active_status.write().await;
        *status_guard = "idle".to_string();
    }
    broadcast_to_room(
        &room,
        &SseMsg::Status {
            state: "idle".to_string(),
        },
    );

    // Clear goal fields on room
    {
        *room.goal_condition.write().await = None;
        *room.goal_status.write().await = None;
        *room.goal_model.write().await = None;
    }

    // Broadcast the cleared goal status to the room
    broadcast_to_room(
        &room,
        &SseMsg::GoalStatus {
            note_id: req.note_id.clone(),
            condition: None,
            status: None,
            model: None,
        },
    );

    Json(ApiResponse::success())
}

pub async fn chat_cancel_handler(
    State(state): State<ServerState>,
    Json(req): Json<ChatCancelReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }

    let room = state.get_or_create_room(&req.note_id).await;

    // Cancel current task
    {
        let mut guard = room.cancel_token.lock().unwrap();
        if let Some(old) = guard.replace(CancellationToken::new()) {
            old.cancel();
        }
    }

    // Reset active status to idle
    {
        let mut status_guard = room.active_status.write().await;
        *status_guard = "idle".to_string();
    }
    broadcast_to_room(
        &room,
        &SseMsg::Status {
            state: "idle".to_string(),
        },
    );

    // Broadcast loop_done if status was running
    let was_running = {
        let status = room.goal_status.read().await;
        status.as_deref() == Some("Running")
    };
    if was_running {
        {
            *room.goal_status.write().await = Some("Paused".to_string());
        }
        let condition = room.goal_condition.read().await.clone();
        let model = room.goal_model.read().await.clone();
        broadcast_to_room(
            &room,
            &SseMsg::GoalStatus {
                note_id: req.note_id.clone(),
                condition,
                status: Some("Paused".to_string()),
                model,
            },
        );
        broadcast_to_room(
            &room,
            &SseMsg::LoopDone {
                status: "cancel".to_string(),
                output: "Loop paused by user".to_string(),
            },
        );
    }

    Json(ApiResponse::success())
}

// ─── Public (no-auth) handlers ──────────────────────────────────────────────

const PUBLIC_PREVIEW_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta name="color-scheme" content="light dark">
<title>{{TITLE}}</title>
<link rel="stylesheet" href="/assets/style.css">
<link rel="stylesheet" href="/assets/highlight-dark.min.css" media="(prefers-color-scheme: dark)">
<link rel="stylesheet" href="/assets/highlight-light.min.css" media="(prefers-color-scheme: light)">
<link rel="stylesheet" href="/assets/katex.min.css">
<style>
  /* Override SPA body rules from style.css */
  body {
    margin: 0 !important;
    padding: 20px !important;
    background: var(--bg-primary) !important;
    color: var(--text-primary) !important;
    font-family: var(--font-sans) !important;
    height: auto !important;
    overflow: auto !important;
  }
  .public-container {
    max-width: 860px;
    margin: 0 auto;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 32px 40px;
  }
  .meta { font-size: 12px; opacity: 0.5; margin-bottom: 24px; }
  #loading { text-align: center; padding: 40px; opacity: 0.5; }
  footer {
    margin-top: 32px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
    opacity: 0.4;
    text-align: center;
    font-size: 12px;
  }
  footer a { color: inherit; text-decoration: underline; }
</style>
</head>
<body>
<div class="public-container">
  <div class="meta">
    <a href="/public/{{NOTE}}/" style="color:inherit;text-decoration:none;opacity:1"
       onmouseover="this.style.textDecoration='underline'"
       onmouseout="this.style.textDecoration='none'">{{NOTE_LABEL}}</a> / {{FILE_LABEL}}
  </div>
  <div id="loading">Loading…</div>
  <div id="preview" style="display:none"></div>
</div>
<script src="/assets/katex.min.js"></script>
<script src="/assets/katex-auto-render.min.js"></script>
<script src="/assets/marked.min.js"></script>
<script src="/assets/highlight.min.js"></script>
<script src="/assets-bin/mermaid.min.js"></script>
<script>
(async function() {
  const rawUrl = '/api/public/raw/{{NOTE}}/{{FILE}}';
  try {
    const resp = await fetch(rawUrl);
    if (!resp.ok) { document.getElementById('loading').textContent = 'Not found or not public.'; return; }
    const md = await resp.text();
    const renderer = new marked.Renderer();
    renderer.code = function({text, lang}) {
      if (lang && lang.toLowerCase() === 'mermaid') {
        const id = 'mermaid-' + Math.random().toString(36).slice(2);
        return '<div class="mermaid" id="' + id + '" data-src="' + text.replace(/"/g,'&quot;') + '"></div>';
      }
      if (typeof hljs !== 'undefined') {
        const language = lang && hljs.getLanguage(lang) ? lang : null;
        const highlighted = language ? hljs.highlight(text, {language}).value : hljs.highlightAuto(text).value;
        return '<pre class="hljs-pre"><code class="hljs">' + highlighted + '</code></pre>';
      }
      return '<pre><code>' + text.replace(/</g,'&lt;') + '</code></pre>';
    };
    marked.use({ renderer });
    const html = marked.parse(md);
    const content = document.getElementById('preview');
    content.innerHTML = html;
    if (typeof renderMathInElement !== 'undefined') {
      renderMathInElement(content, {
        delimiters: [
          {left: '$$', right: '$$', display: true},
          {left: '$', right: '$', display: false},
          {left: '\\(', right: '\\)', display: false},
          {left: '\\[', right: '\\]', display: true}
        ],
        throwOnError: false
      });
    }
    document.getElementById('loading').style.display = 'none';
    content.style.display = '';
    if (typeof mermaid !== 'undefined') {
      const renderMermaidDiagrams = () => {
        document.querySelectorAll('.mermaid').forEach(async (el) => {
          const src = el.dataset.src || '';
          if (!src) return;
          const uid = 'mermaid-' + Math.random().toString(36).slice(2);
          el.id = uid;
          try { const {svg} = await mermaid.render(uid + '-svg', src); el.innerHTML = svg; } catch(e) {}
        });
      };
      mermaid.initialize({
        startOnLoad: false,
        theme: window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'default'
      });
      renderMermaidDiagrams();
      window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function(e) {
        mermaid.initialize({ startOnLoad: false, theme: e.matches ? 'dark' : 'default' });
        renderMermaidDiagrams();
      });
    }
  } catch(e) {
    document.getElementById('loading').textContent = 'Error: ' + e.message;
  }
})();
</script>
<footer>Wrought by <a href="https://fourdollars.github.io/rune/">ᚱᚢᚾᛖ</a></footer>
</body>
</html>"#;

pub async fn public_notes_list_handler(
    State(state): State<ServerState>,
) -> impl axum::response::IntoResponse {
    let notes = state.chat_db.list_notes().unwrap_or_default();
    let mut items = String::new();
    let mut any = false;
    for note in &notes {
        if !note.public {
            continue;
        }
        let public_files = state.chat_db.list_public_files(&note.id);
        if public_files.is_empty() {
            continue;
        }
        any = true;
        items.push_str(&format!(
            "<div class='note-section'><h3>&#128193; {}</h3><ul>",
            html_escape(&note.name)
        ));
        for fname in &public_files {
            let slug = fname.strip_suffix(".md").unwrap_or(fname);
            let url = format!("/public/{}/{}", url_encode(&note.id), url_encode(slug));
            items.push_str(&format!(
                "<li><a href='{}'>{}</a></li>",
                url,
                html_escape(fname)
            ));
        }
        items.push_str("</ul></div>");
    }
    if !any {
        items.push_str("<p class='empty'>No public notes available.</p>");
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Public Notes</title>
<style>
  :root {{ color-scheme: light dark; }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #1e1e2e; color: #cdd6f4; }}
    .container {{ background: #181825; border: 1px solid #313244; }}
    a {{ color: #89b4fa; }}
    h1,h3 {{ color: #cba6f7; }}
    .note-section {{ border-bottom-color: #313244; }}
    footer {{ border-top-color: #313244; }}
  }}
  @media (prefers-color-scheme: light) {{
    body {{ background: #f6f8fa; color: #24292e; }}
    .container {{ background: #fff; border: 1px solid #e1e4e8; }}
    a {{ color: #0366d6; }}
    h1,h3 {{ color: #24292e; }}
    .note-section {{ border-bottom-color: #eaecef; }}
    footer {{ border-top-color: #eaecef; }}
  }}
  * {{ box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif; margin: 0; padding: 20px; }}
  .container {{ max-width: 860px; margin: 0 auto; padding: 32px 40px; border-radius: 8px; }}
  h1 {{ font-size: 2em; margin-top: 0; margin-bottom: 24px; font-weight: 600; line-height: 1.25; padding-bottom: .3em; border-bottom: 1px solid; }}
  h3 {{ font-size: 1.25em; margin: 0 0 8px; font-weight: 600; }}
  .note-section {{ margin-bottom: 20px; padding-bottom: 16px; border-bottom: 1px solid; }}
  .note-section:last-of-type {{ border-bottom: none; padding-bottom: 0; margin-bottom: 0; }}
  ul {{ list-style: none; padding: 0; margin: 0; }}
  li {{ margin: 6px 0; padding-left: 16px; }}
  a {{ text-decoration: none; font-size: 15px; line-height: 1.6; }}
  a:hover {{ text-decoration: underline; }}
  .empty {{ opacity: 0.5; font-style: italic; }}
  footer {{ margin-top: 32px; padding-top: 16px; border-top: 1px solid; opacity: 0.4; text-align: center; font-size: 12px; }}
  footer a {{ color: inherit; text-decoration: underline; }}
</style>
</head>
<body>
<div class="container">
  <h1>Public Notes</h1>
  {}
  <footer>Wrought by <a href="https://fourdollars.github.io/rune/">ᚱᚢᚾᛖ</a></footer>
</div>
</body>
</html>"#,
        items
    );
    axum::response::Html(html)
}

pub async fn public_note_index_handler(
    State(state): State<ServerState>,
    axum::extract::Path(note_id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    // Note must exist and be public
    let note = state
        .chat_db
        .list_notes()
        .unwrap_or_default()
        .into_iter()
        .find(|n| n.id == note_id);
    let note = match note {
        Some(n) if n.public => n,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                axum::response::Html("<h1>404 Not Found</h1>".to_string()),
            )
                .into_response()
        }
    };

    let public_files = state.chat_db.list_public_files(&note_id);
    let mut items = String::new();
    for fname in &public_files {
        let slug = fname.strip_suffix(".md").unwrap_or(fname);
        let url = format!("/public/{}/{}", url_encode(&note_id), url_encode(slug));
        items.push_str(&format!(
            "<li><a href='{}'>{}</a></li>",
            url,
            html_escape(fname)
        ));
    }
    if items.is_empty() {
        items.push_str("<p class=\'empty\'>No public files in this note.</p>");
    } else {
        items = format!("<ul>{}</ul>", items);
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{name}</title>
<style>
  :root {{ color-scheme: light dark; }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #1e1e2e; color: #cdd6f4; }}
    .container {{ background: #181825; border: 1px solid #313244; }}
    a {{ color: #89b4fa; }}
    h1 {{ color: #cba6f7; }}
    .back {{ color: #a6e3a1; }}
    footer {{ border-top-color: #313244; }}
  }}
  @media (prefers-color-scheme: light) {{
    body {{ background: #f6f8fa; color: #24292e; }}
    .container {{ background: #fff; border: 1px solid #e1e4e8; }}
    a {{ color: #0366d6; }}
    h1 {{ color: #24292e; }}
    footer {{ border-top-color: #eaecef; }}
  }}
  * {{ box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, \'Segoe UI\', Helvetica, Arial, sans-serif; margin: 0; padding: 20px; }}
  .container {{ max-width: 860px; margin: 0 auto; padding: 32px 40px; border-radius: 8px; }}
  h1 {{ font-size: 2em; margin-top: 0; margin-bottom: 24px; font-weight: 600; line-height: 1.25; padding-bottom: .3em; border-bottom: 1px solid; }}
  .back {{ display:inline-block; margin-bottom: 16px; font-size: 14px; opacity: 0.7; text-decoration: none; }}
  .back:hover {{ opacity: 1; text-decoration: underline; }}
  ul {{ list-style: none; padding: 0; margin: 0; }}
  li {{ margin: 6px 0; padding-left: 16px; }}
  a {{ text-decoration: none; font-size: 15px; line-height: 1.6; }}
  a:hover {{ text-decoration: underline; }}
  .empty {{ opacity: 0.5; font-style: italic; }}
  footer {{ margin-top: 32px; padding-top: 16px; border-top: 1px solid; opacity: 0.4; text-align: center; font-size: 12px; }}
  footer a {{ color: inherit; text-decoration: underline; }}
</style>
</head>
<body>
<div class="container">
  <a href="/public/" class="back">← All Notes</a>
  <h1>&#128193; {name}</h1>
  {items}
  <footer>Wrought by <a href="https://fourdollars.github.io/rune/">ᚱᚢᚾᛖ</a></footer>
</div>
</body>
</html>"#,
        name = html_escape(&note.name),
        items = items
    );
    (StatusCode::OK, axum::response::Html(html)).into_response()
}

pub async fn public_preview_handler(
    State(state): State<ServerState>,
    axum::extract::Path((note_id, file_slug)): axum::extract::Path<(String, String)>,
) -> impl axum::response::IntoResponse {
    // Accept both "OpenAI" and "OpenAI.md"
    let filename = if file_slug.ends_with(".md") {
        file_slug.clone()
    } else {
        format!("{}.md", file_slug)
    };
    let note_public = state
        .chat_db
        .list_notes()
        .unwrap_or_default()
        .iter()
        .find(|n| n.id == note_id)
        .map(|n| n.public)
        .unwrap_or(false);
    let file_public = state.chat_db.is_file_public(&note_id, &filename);
    if !note_public || !file_public {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html("<h1>404 Not Found</h1>".to_string()),
        )
            .into_response();
    }
    let file_label = filename.strip_suffix(".md").unwrap_or(&filename);
    let title = format!("{} / {}", note_id, file_label);
    let page = PUBLIC_PREVIEW_HTML
        .replace("{{TITLE}}", &html_escape(&title))
        .replace("{{NOTE}}", &url_encode(&note_id))
        .replace("{{FILE}}", &url_encode(&filename))
        .replace("{{NOTE_LABEL}}", &html_escape(&note_id))
        .replace("{{FILE_LABEL}}", &html_escape(file_label));
    (StatusCode::OK, axum::response::Html(page)).into_response()
}

pub async fn public_raw_handler(
    State(state): State<ServerState>,
    axum::extract::Path((note_id, file_slug)): axum::extract::Path<(String, String)>,
) -> impl axum::response::IntoResponse {
    let filename = if file_slug.ends_with(".md") {
        file_slug.clone()
    } else {
        format!("{}.md", file_slug)
    };
    let note_public = state
        .chat_db
        .list_notes()
        .unwrap_or_default()
        .iter()
        .find(|n| n.id == note_id)
        .map(|n| n.public)
        .unwrap_or(false);
    let file_public = state.chat_db.is_file_public(&note_id, &filename);
    if !note_public || !file_public {
        return (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            "".to_string(),
        )
            .into_response();
    }
    let file_path = state.note_markdown_dir(&note_id).join(&filename);
    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8",
            )],
            content,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            "".to_string(),
        )
            .into_response(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn url_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

async fn broadcast_file_list(state: &ServerState, note_id: &str) {
    let md_dir = state.note_markdown_dir(note_id);
    let mut file_names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                file_names.push(name);
            }
        }
    }
    file_names.sort();
    let visibility = state.chat_db.get_file_visibility(note_id);
    let files: Vec<FileEntry> = file_names
        .iter()
        .map(|name| {
            let public = visibility
                .iter()
                .find(|(f, _)| f == name)
                .map(|(_, p)| *p)
                .unwrap_or(false);
            FileEntry {
                name: name.clone(),
                public,
            }
        })
        .collect();
    let active = files.first().map(|f| f.name.clone()).unwrap_or_default();
    let msg = SseMsg::FileList { files, active };
    // Send file_list to the note's own room
    let room = state.get_or_create_room(note_id).await;
    broadcast_to_room(&room, &msg);
    // Also broadcast note_list to ALL rooms so clients subscribed to other notes
    // can update the sidebar visibility state for this note
    broadcast_note_list(state).await;
}

async fn broadcast_note_list(state: &ServerState) {
    let notes = build_note_list(state).await;
    let msg = SseMsg::NoteList {
        notes,
        active: String::new(),
    };
    // Note list is global — broadcast to all rooms
    let rooms = state.rooms.read().await;
    for room in rooms.values() {
        broadcast_to_room(room, &msg);
    }
}

/// GET /api/notes — JSON list of notes (authenticated users only)
pub async fn notes_list_json_handler(State(state): State<ServerState>) -> Json<serde_json::Value> {
    let notes = build_note_list(&state).await;
    Json(serde_json::json!({ "ok": true, "notes": notes }))
}

/// GET /api/me — return current authenticated user info from session cookie.
pub async fn me_handler(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::serve::oauth::get_cookie;
    let sid = get_cookie(&headers, "rune_sid");
    let session = match sid {
        Some(ref id) => state.sessions.get(id).await,
        None => None,
    };
    match session {
        Some(s) => Json(serde_json::json!({
            "ok": true,
            "login": s.login,
            "role": s.role.as_str(),
            "avatar_url": s.avatar_url,
        }))
        .into_response(),
        None => (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"ok": false, "error": "Not authenticated"})),
        )
            .into_response(),
    }
}

// ─── Agent chat handler ────────────────────────────────────────────────────

async fn handle_chat_message(
    user_msg: String,
    state: ServerState,
    note_id: String,
    nickname: String,
) {
    let config = state.config.clone();
    // Use per-note effective model (override > global default)
    let active_model = state.effective_model(&note_id).await;

    // Get the room for per-note broadcasting
    let room = state.get_or_create_room(&note_id).await;

    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let err = SseMsg::Error {
                message: format!("Provider error: {}", e),
            };
            broadcast_to_room(&room, &err);
            let idle = SseMsg::Status {
                state: "idle".to_string(),
            };
            broadcast_to_room(&room, &idle);
            return;
        }
    };

    // Build embedding
    let embedding = build_embedding(&config).await;

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

    // Approval callback — requests go to room, responses come back via room
    let room_for_approval = Arc::clone(&room);
    let approval_callback: Arc<
        dyn Fn(String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
            + Send
            + Sync,
    > = Arc::new(move |id: String, detail: String| {
        let room = Arc::clone(&room_for_approval);
        Box::pin(async move {
            // Send approval request to the room
            let msg = SseMsg::ApprovalRequest {
                id: id.clone(),
                detail,
            };
            broadcast_to_room(&room, &msg);
            // Wait for approval response via room channel
            let mut rx = room.broadcast_tx.subscribe();
            let approve_key = format!("__approval_granted__{}", id);
            let deny_key = format!("__approval_denied__{}", id);
            loop {
                match tokio::time::timeout(std::time::Duration::from_secs(300), rx.recv()).await {
                    Ok(Ok(msg)) => {
                        if msg == approve_key {
                            return true;
                        }
                        if msg == deny_key {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
        })
    });

    // Build agent
    let mut cfg = config.clone();
    cfg.model = active_model.clone();
    let effective_thinking_level = state.effective_thinking(&note_id).await;
    cfg.thinking = effective_thinking_level.clone();
    // Use the model actual context window from ModelInfo (dynamic, from provider API).
    // This ensures compact threshold aligns with real model capability.
    // Falls back to cfg.context_window (config/default) when ModelInfo is unavailable.
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
    agent.token_callback = Some(token_callback);
    agent.approval_callback = Some(approval_callback);

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

    agent.user_name = if !nickname.is_empty() && nickname != "user" {
        Some(nickname)
    } else {
        None
    };
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

    // Set system prompt: per-note override > global config > default
    let system_prompt = {
        let room_prompt = room.system_prompt.read().await;
        if let Some(ref p) = *room_prompt {
            p.clone()
        } else {
            build_system_prompt(&config).await
        }
    };
    agent.set_system_prompt(&system_prompt);

    // Load chat history into agent context.
    // Fetch 200 records so token-aware trimming inside load_history has
    // enough material; it will drop oldest pairs to fit within 40% of
    // context_window automatically.
    let history = state.chat_db.load_recent_async(note_id.clone(), 200).await;
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

    // Broadcast run statistics
    let meta_model = active_model.clone();
    let meta_thinking = effective_thinking_level.clone().filter(|t| t != "off");
    let meta = SseMsg::ChatMeta {
        model: active_model,
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
    };
    broadcast_to_room(&room, &meta);

    // Process result
    match &stop_reason {
        StopReason::FinalAnswer(answer) => {
            // Save assistant response with run statistics
            state
                .chat_db
                .insert_with_meta_async(
                    note_id.clone(),
                    "assistant".to_string(),
                    "ᚱᚢᚾᛖ".to_string(),
                    answer.clone(),
                    Some(meta_model.clone()),
                    Some(agent.tokens_in() as i32),
                    Some(agent.tokens_out() as i32),
                    Some(agent.step_count() as i32),
                    Some(agent.tool_call_count() as i32),
                    meta_thinking,
                    Some(agent.total_context_tokens() as i32),
                )
                .await;
        }
        StopReason::Error(e) => {
            let err = SseMsg::Error {
                message: format!("Agent error: {}", e),
            };
            broadcast_to_room(&room, &err);
        }
        StopReason::MaxSteps => {
            let err = SseMsg::Error {
                message: "Agent reached max steps".to_string(),
            };
            broadcast_to_room(&room, &err);
        }
        StopReason::TokenBudgetExhausted => {
            let err = SseMsg::Error {
                message: "Token budget exhausted".to_string(),
            };
            broadcast_to_room(&room, &err);
        }
        _ => {}
    }

    // Broadcast updated file list to the room
    broadcast_file_list(&state, &note_id).await;

    // Mark room as idle (for SSE reconnect recovery)
    {
        let mut status = room.active_status.write().await;
        *status = "idle".to_string();
    }

    let idle = SseMsg::Status {
        state: "idle".to_string(),
    };
    broadcast_to_room(&room, &idle);
}

// ─── Provider/Embedding builders (from ws.rs) ──────────────────────────────

/// Public wrapper for build_provider (used by serve startup for model discovery).
pub fn build_provider_pub(config: &RuneConfig) -> anyhow::Result<ProviderRegistry> {
    build_provider(config)
}

fn build_provider(config: &RuneConfig) -> anyhow::Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();

    let key = config
        .api_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No API key configured. Run `rune init` first."))?;

    let provider_name = config.provider.as_deref().unwrap_or_else(|| {
        if key.starts_with("ghu_")
            || key.starts_with("ghp_")
            || config
                .base_url
                .as_deref()
                .map(|u| u.contains("githubcopilot"))
                .unwrap_or(false)
        {
            "github-copilot"
        } else if key.starts_with("AIza")
            || config
                .base_url
                .as_deref()
                .map(|u| u.contains("generativelanguage.googleapis.com"))
                .unwrap_or(false)
        {
            "gemini"
        } else if key.starts_with("sk-or-") {
            "openrouter"
        } else {
            "openai"
        }
    });

    match provider_name {
        "github-copilot" | "copilot" => {
            registry.register(Box::new(CopilotProvider::new(key)));
        }
        "gemini" | "google" => {
            registry.register(Box::new(GeminiProvider::new(
                key,
                Some(config.model.clone()),
                config.base_url.clone(),
            )));
        }
        other => {
            registry.register(Box::new(OpenAiProvider::new(
                other.to_string(),
                key,
                config.base_url.clone(),
            )));
        }
    }

    if registry.is_empty() {
        anyhow::bail!("No providers configured");
    }
    Ok(registry)
}
async fn build_embedding(config: &RuneConfig) -> Option<EmbeddingEngine> {
    let api_key = config.api_key.clone().unwrap_or_default();
    if api_key.is_empty() {
        return None;
    }
    let mut emb_config = config.embedding.clone();
    if emb_config.api_key.is_none() {
        emb_config.api_key = Some(api_key.clone());
    }

    let is_copilot = config.provider.as_deref() == Some("github-copilot")
        || config.provider.as_deref() == Some("copilot")
        || api_key.starts_with("ghu_")
        || api_key.starts_with("ghp_");

    let is_gemini = config.provider.as_deref() == Some("gemini")
        || config.provider.as_deref() == Some("google")
        || api_key.starts_with("AIza");

    if is_copilot {
        if emb_config.base_url.is_none() {
            emb_config.base_url = Some("https://api.githubcopilot.com".to_string());
        }
        Some(EmbeddingEngine::new_copilot(emb_config, api_key))
    } else if is_gemini {
        if emb_config.base_url.is_none() {
            emb_config.base_url =
                Some("https://generativelanguage.googleapis.com/v1beta/openai".to_string());
        }
        if emb_config.model.is_none() {
            emb_config.model = Some("gemini-embedding-2".to_string());
        }
        Some(EmbeddingEngine::new(emb_config))
    } else {
        if emb_config.base_url.is_none() {
            let is_openrouter =
                config.provider.as_deref() == Some("openrouter") || api_key.starts_with("sk-or-");
            let default_url = if is_openrouter {
                Some("https://openrouter.ai/api/v1".to_string())
            } else {
                config.base_url.clone()
            };
            emb_config.base_url = config.base_url.clone().or(default_url);
        }
        Some(EmbeddingEngine::new(emb_config))
    }
}

async fn build_system_prompt(config: &RuneConfig) -> String {
    if let Some(ref prompt) = config.system_prompt {
        if !prompt.is_empty() {
            return prompt.clone();
        }
    }
    r#"You are Rune, an AI assistant embedded in a collaborative markdown notebook system.

## Your Environment

You are operating inside a **Rune Notes** notebook. Each notebook contains one or more markdown files. The user can view and edit these files in real time through a web interface.

## Notebook Tools (Highest Priority)

You have three dedicated notebook tools: `list_markdown`, `read_markdown`, and `write_markdown`. **Always use these instead of generic file or shell tools for notebook content.**

## Rules

1. **Check first**: call `list_markdown` if you do not already know which files exist.
2. **Write back**: when the user asks you to write, update, or summarise something, save the result to the notebook with `write_markdown`.
3. **Notebook only**: never use `read_file`, `write_file`, or `execute_cmd` for notebook content — those bypass the notebook and the user will not see the changes.
4. Treat the markdown files as the single source of truth for all notebook content.

## Style

Be concise, accurate, and collaborative. When you change a file, briefly describe what you changed and why."#.to_string()
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_filename() {
        assert!(is_valid_filename("spec.md"));
        assert!(is_valid_filename("my-doc.md"));
        assert!(is_valid_filename("arch_v2.md"));
        assert!(is_valid_filename("CAPS.md"));
        assert!(!is_valid_filename(""));
        assert!(!is_valid_filename("file.txt"));
        assert!(!is_valid_filename("../etc/passwd.md"));
        assert!(!is_valid_filename("file name.md"));
        assert!(!is_valid_filename("file;rm.md"));
        assert!(!is_valid_filename("a".repeat(65).as_str()));
    }

    #[tokio::test]
    async fn test_build_embedding_fallback() {
        let mut config = RuneConfig::default();
        config.api_key = Some("root_api_key".to_string());
        config.embedding.enabled = true;
        config.embedding.api_key = None;

        let engine = build_embedding(&config).await;
        assert!(engine.is_some());

        config.api_key = None;
        let engine_none = build_embedding(&config).await;
        assert!(engine_none.is_none());
    }

    // ─── Session-based auth tests (replaces old token auth tests) ────────────

    #[tokio::test]
    async fn test_session_auth_no_session_rejects() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        // No session inserted — lookup should return None
        assert!(store.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_session_auth_valid_session() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        let session = Session {
            id: "test-sid".into(),
            login: "alice".into(),
            role: Role::Admin,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(session).await;
        let found = store.get("test-sid").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().login, "alice");
    }

    #[tokio::test]
    async fn test_session_auth_expired_session_rejects() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        let session = Session {
            id: "expired-sid".into(),
            login: "bob".into(),
            role: Role::User,
            avatar_url: "".into(),
            expires_at: Instant::now() - Duration::from_secs(1),
        };
        store.insert(session).await;
        assert!(store.get("expired-sid").await.is_none());
    }

    #[tokio::test]
    async fn test_session_auth_guest_role() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        let session = Session {
            id: "guest-sid".into(),
            login: "guest-user".into(),
            role: Role::Guest,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(session).await;
        let found = store.get("guest-sid").await.unwrap();
        assert!(found.is_guest());
        assert!(!found.is_admin());
    }

    #[tokio::test]
    async fn test_session_auth_admin_role() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        let session = Session {
            id: "admin-sid".into(),
            login: "superuser".into(),
            role: Role::Admin,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(session).await;
        let found = store.get("admin-sid").await.unwrap();
        assert!(found.is_admin());
        assert!(!found.is_guest());
    }

    #[tokio::test]
    async fn test_session_store_isolation() {
        use crate::serve::oauth::{Role, Session, SessionStore};
        use std::time::{Duration, Instant};

        let store = SessionStore::new();
        let s1 = Session {
            id: "s1".into(),
            login: "user1".into(),
            role: Role::User,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        let s2 = Session {
            id: "s2".into(),
            login: "user2".into(),
            role: Role::Admin,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(s1).await;
        store.insert(s2).await;
        store.remove("s1").await;
        assert!(store.get("s1").await.is_none());
        assert!(store.get("s2").await.is_some());
    }

    #[test]
    fn test_api_response_success() {
        let resp = ApiResponse::success();
        assert!(resp.ok);
        assert!(resp.error.is_none());
        assert!(resp.data.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let resp = ApiResponse::err("something broke");
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("something broke"));
    }

    #[test]
    fn test_api_response_with_data() {
        let resp = ApiResponse::with_data(serde_json::json!({"key": "val"}));
        assert!(resp.ok);
        assert!(resp.data.is_some());
    }

    #[test]
    fn test_extract_event_type() {
        let json = r#"{"type":"chat_token","content":"hi"}"#;
        assert_eq!(extract_event_type(json), "chat_token");

        let json2 = r#"{"type":"note_list","sessions":[]}"#;
        assert_eq!(extract_event_type(json2), "note_list");

        let no_type = r#"{"content":"hi"}"#;
        assert_eq!(extract_event_type(no_type), "message");
    }

    #[test]
    fn test_sse_msg_serialization() {
        let msg = SseMsg::ChatToken {
            content: "hello".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"chat_token""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn test_sse_msg_error_serialization() {
        let msg = SseMsg::Error {
            message: "oops".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"error""#));
        assert!(json.contains(r#""message":"oops""#));
    }

    #[test]
    fn test_sse_msg_note_list() {
        let msg = SseMsg::NoteList {
            notes: vec![NoteListEntry {
                id: "test".into(),
                name: "Test".into(),
                files: vec!["spec.md".into()],
                public: false,
                public_files: vec![],
                icon: None,
            }],
            active: "test".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"note_list""#));
        assert!(json.contains(r#""id":"test""#));
    }

    #[test]
    fn test_sse_msg_chat_meta() {
        let msg = SseMsg::ChatMeta {
            model: "gpt-4".into(),
            thinking: Some("high".into()),
            tokens_in: 100,
            tokens_out: 50,
            context_tokens: 1000,
            context_window: 128000,
            steps: 3,
            tool_calls: 2,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"chat_meta""#));
        assert!(json.contains(r#""tokens_in":100"#));
    }

    #[test]
    fn test_sse_msg_file_list() {
        let msg = SseMsg::FileList {
            files: vec![
                FileEntry {
                    name: "a.md".into(),
                    public: false,
                },
                FileEntry {
                    name: "b.md".into(),
                    public: false,
                },
            ],
            active: "a.md".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"file_list""#));
        assert!(json.contains(r#""active":"a.md""#));
    }

    #[test]
    fn test_sse_msg_users_update() {
        let msg = SseMsg::UsersUpdate { count: 5 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"users_update""#));
        assert!(json.contains(r#""count":5"#));
    }

    #[test]
    fn test_sse_msg_model_list() {
        let msg = SseMsg::ModelList {
            models: vec![
                ModelListEntry {
                    id: "gpt-4".into(),
                    provider: Some("openai".into()),
                    context_window: Some(128000),
                    reasoning_efforts: vec![],
                },
                ModelListEntry {
                    id: "claude".into(),
                    provider: Some("openrouter".into()),
                    context_window: Some(200000),
                    reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()],
                },
            ],
            active: "gpt-4".into(),
            thinking: "off".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"model_list""#));
        assert!(json.contains(r#""provider":"openai""#));
        assert!(json.contains(r#""provider":"openrouter""#));
        assert!(json.contains(r#""context_window":128000"#));
        assert!(json.contains(r#""reasoning_efforts":["low","medium","high"]"#));
        assert!(json.contains(r#""thinking":"off""#));
    }

    #[test]
    fn test_sse_msg_dir_browse() {
        let msg = SseMsg::DirBrowseResult {
            path: "/home".into(),
            parent: Some("/".into()),
            entries: vec![DirEntry {
                name: "user".into(),
                is_dir: true,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"dir_browse_result""#));
        assert!(json.contains(r#""is_dir":true"#));
    }

    #[test]
    fn test_note_list_entry_serialize() {
        let entry = NoteListEntry {
            id: "s1".into(),
            name: "Session One".into(),
            files: vec!["readme.md".into()],
            public: false,
            public_files: vec![],
            icon: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains(r#""id":"s1""#));
    }

    #[test]
    fn test_request_deserialization() {
        let json = r#"{"note_id":"abc","content":"hello"}"#;
        let req: ChatReq = serde_json::from_str(json).unwrap();
        assert_eq!(req.note_id, "abc");
        assert_eq!(req.content, "hello");
    }

    #[test]
    fn test_file_create_req() {
        let json = r#"{"note_id":"s1","name":"new.md"}"#;
        let req: FileCreateReq = serde_json::from_str(json).unwrap();
        assert_eq!(req.note_id, "s1");
        assert_eq!(req.name, "new.md");
    }

    #[test]
    fn test_file_rename_req() {
        let json = r#"{"note_id":"s1","old_name":"a.md","new_name":"b.md"}"#;
        let req: FileRenameReq = serde_json::from_str(json).unwrap();
        assert_eq!(req.old_name, "a.md");
        assert_eq!(req.new_name, "b.md");
    }

    // Helper to create a minimal ServerState for tests
    fn mock_state(_token: Option<String>, _admin_token: Option<String>) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(16);
        ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "test-model".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("test-model".into())),
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
        }
    }
}

#[cfg(test)]
mod integration_tests {
    //! Integration tests for SSE + REST API handlers.
    //! Uses tower::ServiceExt to call axum Router directly without network.

    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::{get, post},
        Router,
    };
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{broadcast, RwLock};
    use tower::ServiceExt;

    use crate::config::RuneConfig;
    use crate::serve::api::*;
    use crate::serve::db::ChatDb;
    use crate::serve::ServerState;

    /// Build a test app with all routes and a fresh temp state.
    fn test_app() -> (Router, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp);
        let app = Router::new()
            .route("/api/events", get(events_handler))
            .route("/api/chat", post(chat_handler))
            .route("/api/file/create", post(file_create_handler))
            .route("/api/file/delete", post(file_delete_handler))
            .route("/api/file/rename", post(file_rename_handler))
            .route("/api/file/switch", post(file_switch_handler))
            .route("/api/file/update", post(file_update_handler))
            .route("/api/session/create", post(note_create_handler))
            .route("/api/session/rename", post(note_rename_handler))
            .route("/api/session/delete", post(note_delete_handler))
            .route("/api/session/switch", post(note_switch_handler))
            .route("/api/model/switch", post(model_switch_handler))
            .route("/api/chat/archive", post(archive_handler))
            .route("/api/chat/search", post(search_handler))
            .route("/api/approval", post(approval_handler))
            .route("/api/dir/browse", post(dir_browse_handler))
            .with_state(state);
        (app, tmp)
    }

    fn test_state(tmp: &TempDir) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db_path = tmp.path().join("test.db");
        let db = ChatDb::open(&db_path).unwrap();
        ServerState {
            config: RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![
                ModelInfo {
                    provider: None,
                    id: "gpt-5-mini".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
                ModelInfo {
                    provider: None,
                    id: "claude-sonnet-4.6".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
            ])),
            rooms: Arc::new(RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(RwLock::new("gpt-5-mini".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        }
    }

    fn test_state_with_token(tmp: &TempDir) -> ServerState {
        test_state(tmp)
    }

    async fn post_json(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: Value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
        (status, val)
    }

    // ─── Session CRUD tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_session_create() {
        let (app, _tmp) = test_app();
        let (status, body) = post_json(
            &app,
            "/api/session/create",
            json!({
                "name": "test-session",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_session_create_empty_name() {
        let (app, _tmp) = test_app();
        let (status, body) = post_json(
            &app,
            "/api/session/create",
            json!({
                "name": "",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("required"));
    }

    #[tokio::test]
    async fn test_session_create_duplicate() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "dup"})).await;
        let (_, body) = post_json(&app, "/api/session/create", json!({"name": "dup"})).await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_session_delete() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "del-me"})).await;
        let (_, body) = post_json(&app, "/api/session/delete", json!({"note_id": "del-me"})).await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_session_delete_nonexistent() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(&app, "/api/session/delete", json!({"note_id": "nope"})).await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_session_switch() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "s1"})).await;
        let (_, body) = post_json(&app, "/api/session/switch", json!({"note_id": "s1"})).await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_session_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(&tmp);
        let app = Router::new()
            .route("/api/session/create", post(note_create_handler))
            .route("/api/session/rename", post(note_rename_handler))
            .with_state(state.clone());

        post_json(&app, "/api/session/create", json!({"name": "old-name"})).await;

        // Create a dummy markdown file in old-name note directory
        let old_md_dir = state.note_markdown_dir("old-name");
        tokio::fs::create_dir_all(&old_md_dir).await.unwrap();
        let dummy_file = old_md_dir.join("hello.md");
        tokio::fs::write(&dummy_file, b"content").await.unwrap();

        // 1. Rename to same name but update icon (simulate emoji picker icon change)
        let (_, body) = post_json(
            &app,
            "/api/session/rename",
            json!({
                "note_id": "old-name",
                "name": "old-name",
                "icon": "🚀"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Assert that note session icon is indeed "🚀" in db:
        let record = state.chat_db.get_session("old-name").unwrap().unwrap();
        assert_eq!(record.icon.as_deref(), Some("🚀"));

        // Assert markdown file still exists (not deleted!)
        assert!(dummy_file.exists());

        // 2. Rename to a different name
        let (_, body) = post_json(
            &app,
            "/api/session/rename",
            json!({
                "note_id": "old-name",
                "name": "new-name-test",
                "icon": "🚀"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Assert that new note session icon is indeed "🚀" in db:
        let record = state.chat_db.get_session("new-name-test").unwrap().unwrap();
        assert_eq!(record.icon.as_deref(), Some("🚀"));

        // Assert markdown file moved to new directory
        let new_md_dir = state.note_markdown_dir("new-name-test");
        assert!(new_md_dir.join("hello.md").exists());

        // Assert old directory is cleaned up
        assert!(!old_md_dir.exists());
    }

    // ─── File CRUD tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_file_create() {
        let (app, tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "file-test"})).await;
        let (_, body) = post_json(
            &app,
            "/api/file/create",
            json!({
                "note_id": "file-test",
                "name": "notes.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_file_create_invalid_name() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f1"})).await;
        let (_, body) = post_json(
            &app,
            "/api/file/create",
            json!({
                "note_id": "f1",
                "name": "bad file.txt"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("Invalid"));
    }

    #[tokio::test]
    async fn test_file_create_duplicate() {
        let (app, tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f2"})).await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f2", "name": "a.md"}),
        )
        .await;
        let (_, body) = post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f2", "name": "a.md"}),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("exists"));
    }

    #[tokio::test]
    async fn test_file_create_no_session() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/file/create",
            json!({
                "note_id": "",
                "name": "x.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_file_update_and_switch() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f3"})).await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f3", "name": "doc.md"}),
        )
        .await;

        // Update
        let (_, body) = post_json(
            &app,
            "/api/file/update",
            json!({
                "note_id": "f3",
                "filename": "doc.md",
                "content": "# Hello\nWorld"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Switch (read back)
        let (_, body) = post_json(
            &app,
            "/api/file/switch",
            json!({
                "note_id": "f3",
                "name": "doc.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_file_switch_not_found() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f4"})).await;
        let (_, body) = post_json(
            &app,
            "/api/file/switch",
            json!({
                "note_id": "f4",
                "name": "nope.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_file_rename() {
        let (app, tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f5"})).await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f5", "name": "old.md"}),
        )
        .await;
        let (_, body) = post_json(
            &app,
            "/api/file/rename",
            json!({
                "note_id": "f5",
                "old_name": "old.md",
                "new_name": "new.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_file_rename_conflict() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f6"})).await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f6", "name": "a.md"}),
        )
        .await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f6", "name": "b.md"}),
        )
        .await;
        let (_, body) = post_json(
            &app,
            "/api/file/rename",
            json!({
                "note_id": "f6",
                "old_name": "a.md",
                "new_name": "b.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("exists"));
    }

    #[tokio::test]
    async fn test_file_delete() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f7"})).await;
        post_json(
            &app,
            "/api/file/create",
            json!({"note_id": "f7", "name": "rm.md"}),
        )
        .await;
        let (_, body) = post_json(
            &app,
            "/api/file/delete",
            json!({
                "note_id": "f7",
                "name": "rm.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_file_update_invalid_filename() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "f8"})).await;
        let (_, body) = post_json(
            &app,
            "/api/file/update",
            json!({
                "note_id": "f8",
                "filename": "",
                "content": "x"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
    }

    // ─── Model switch tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_model_switch_valid() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/model/switch",
            json!({
                "model": "claude-sonnet-4.6"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_model_switch_unknown() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/model/switch",
            json!({
                "model": "unknown-model"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("Unknown"));
    }

    // ─── Chat tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_chat_no_session() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/chat",
            json!({
                "note_id": "",
                "content": "hello"
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("note"));
    }

    #[tokio::test]
    async fn test_chat_empty_content() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/chat",
            json!({
                "note_id": "s1",
                "content": "   "
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("Empty"));
    }

    // ─── Archive + Search tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_archive_no_session() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/chat/archive",
            json!({
                "note_id": ""
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn test_search_empty_query() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/chat/search",
            json!({
                "note_id": "s1",
                "query": ""
            }),
        )
        .await;
        assert_eq!(body["ok"], false);
        assert!(body["error"].as_str().unwrap().contains("Empty"));
    }

    #[tokio::test]
    async fn test_search_valid() {
        let (app, _tmp) = test_app();
        post_json(&app, "/api/session/create", json!({"name": "search-test"})).await;
        let (_, body) = post_json(
            &app,
            "/api/chat/search",
            json!({
                "note_id": "search-test",
                "query": "hello"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    // ─── Dir browse tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dir_browse_root() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/dir/browse",
            json!({
                "path": "/tmp"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
        assert!(body["data"].is_object());
    }

    #[tokio::test]
    async fn test_dir_browse_nonexistent() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/dir/browse",
            json!({
                "path": "/nonexistent_path_12345"
            }),
        )
        .await;
        // Should still return ok with empty entries
        assert_eq!(body["ok"], true);
    }

    // ─── Approval test ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_approval_granted() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/approval",
            json!({
                "id": "test-123",
                "approved": true
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_approval_denied() {
        let (app, _tmp) = test_app();
        let (_, body) = post_json(
            &app,
            "/api/approval",
            json!({
                "id": "test-456",
                "approved": false
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    // ─── SSE endpoint test ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_sse_events_connect() {
        let (app, _tmp) = test_app();
        let req = Request::builder()
            .method("GET")
            .uri("/api/events?nickname=tester")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Content-Type should be text/event-stream
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/event-stream"),
            "Expected SSE content type, got: {}",
            ct
        );
    }

    // ─── Full flow integration test ────────────────────────────────────────────

    #[tokio::test]
    async fn test_full_session_file_flow() {
        let (app, _tmp) = test_app();

        // Create session
        let (_, body) = post_json(
            &app,
            "/api/session/create",
            json!({
                "name": "integration",
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Create file
        let (_, body) = post_json(
            &app,
            "/api/file/create",
            json!({
                "note_id": "integration",
                "name": "readme.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Update file
        let (_, body) = post_json(
            &app,
            "/api/file/update",
            json!({
                "note_id": "integration",
                "filename": "readme.md",
                "content": "# Integration Test\n\nThis works!"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Switch to file (read back)
        let (_, body) = post_json(
            &app,
            "/api/file/switch",
            json!({
                "note_id": "integration",
                "name": "readme.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Rename file
        let (_, body) = post_json(
            &app,
            "/api/file/rename",
            json!({
                "note_id": "integration",
                "old_name": "readme.md",
                "new_name": "docs.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Delete file
        let (_, body) = post_json(
            &app,
            "/api/file/delete",
            json!({
                "note_id": "integration",
                "name": "docs.md"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);

        // Delete session
        let (_, body) = post_json(
            &app,
            "/api/session/delete",
            json!({
                "note_id": "integration"
            }),
        )
        .await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn test_file_visibility_change_broadcasts_note_list_to_all_rooms() {
        // Regression test: changing file visibility on note B while subscribed to note A
        // must update the note_list in note A's room so the sidebar reflects the change.
        use tokio::sync::broadcast;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("note-a", "note-a", None);
        let _ = db.set_note_public("note-a", true);
        let _ = db.create_note("note-b", "note-b", None);
        let _ = db.set_note_public("note-b", true);
        let _ = db.set_file_public("note-b", "doc.md", false);

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        // Subscribe to note-a's room BEFORE triggering visibility change on note-b
        let room_a = state.get_or_create_room("note-a").await;
        let mut rx_a = room_a.broadcast_tx.subscribe();

        // Trigger file visibility change on note-b
        broadcast_file_list(&state, "note-b").await;

        // note-a's room should receive a note_list event (from broadcast_note_list)
        let mut got_note_list = false;
        // Drain up to 10 messages looking for note_list
        for _ in 0..10 {
            match rx_a.try_recv() {
                Ok(msg) => {
                    if msg.contains("note_list") {
                        got_note_list = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(
            got_note_list,
            "note-a room should receive note_list broadcast when note-b file visibility changes"
        );
    }
}

// ─── Per-Note Isolation Tests ──────────────────────────────────────────────

#[cfg(test)]
mod isolation_tests {
    use crate::config::RuneConfig;
    use crate::serve::api::NoteRoom;
    use crate::serve::db::ChatDb;
    use crate::serve::ModelInfo;
    use crate::serve::ServerState;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{broadcast, RwLock};
    use tokio_util::sync::CancellationToken;

    fn make_state() -> (ServerState, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db_path = tmp.path().join("test.db");
        let db = ChatDb::open(&db_path).unwrap();
        let state = ServerState {
            config: RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![
                ModelInfo {
                    provider: None,
                    id: "gpt-5-mini".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
                ModelInfo {
                    provider: None,
                    id: "claude-sonnet-4.6".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
            ])),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new("gpt-5-mini".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };
        (state, tmp)
    }

    #[tokio::test]
    async fn test_room_lazy_create() {
        let (state, _tmp) = make_state();
        let room1 = state.get_or_create_room("note-a").await;
        let room2 = state.get_or_create_room("note-a").await;
        assert!(Arc::ptr_eq(&room1, &room2));
    }

    #[tokio::test]
    async fn test_room_different_notes() {
        let (state, _tmp) = make_state();
        let room_a = state.get_or_create_room("note-a").await;
        let room_b = state.get_or_create_room("note-b").await;
        assert!(!Arc::ptr_eq(&room_a, &room_b));
    }

    #[tokio::test]
    async fn test_effective_model_fallback() {
        let (state, _tmp) = make_state();
        let model = state.effective_model("note-x").await;
        assert_eq!(model, "gpt-5-mini");
    }

    #[tokio::test]
    async fn test_effective_model_override() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-x").await;
        *room.model_override.write().await = Some("claude-sonnet-4.6".into());
        let model = state.effective_model("note-x").await;
        assert_eq!(model, "claude-sonnet-4.6");
    }

    #[tokio::test]
    async fn test_model_info_context_window_applied_to_cfg() {
        // When ModelInfo has a context_window, it should override cfg.context_window
        // This test verifies the lookup logic mirrors what handle_chat_message does.
        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = tokio::sync::broadcast::channel(256);
        let db_path = tmp.path().join("test.db");
        let db = ChatDb::open(&db_path).unwrap();
        let model_id = "gpt-5-mini";
        let expected_cw: u64 = 131072;
        let state = ServerState {
            config: RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: model_id.into(),
                context_window: Some(expected_cw),
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new(model_id.into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };
        let active_model = state.effective_model("note-x").await;
        assert_eq!(active_model, model_id);
        // Simulate what handle_chat_message does: look up ModelInfo and override cfg
        let mut cfg = state.config.clone();
        let models = state.models.read().await;
        if let Some(model_info) = models.iter().find(|m| m.id == active_model) {
            if let Some(cw) = model_info.context_window {
                cfg.context_window = cw as usize;
            }
        }
        drop(models);
        assert_eq!(
            cfg.context_window, expected_cw as usize,
            "cfg.context_window must be overridden by ModelInfo.context_window"
        );
    }

    #[tokio::test]
    async fn test_model_info_context_window_fallback_when_none() {
        // When ModelInfo.context_window is None, cfg.context_window keeps its default
        let (state, _tmp) = make_state();
        let active_model = state.effective_model("note-x").await;
        let default_cw = state.config.context_window;
        let mut cfg = state.config.clone();
        let models = state.models.read().await;
        if let Some(model_info) = models.iter().find(|m| m.id == active_model) {
            if let Some(cw) = model_info.context_window {
                cfg.context_window = cw as usize;
            }
        }
        drop(models);
        assert_eq!(
            cfg.context_window, default_cw,
            "cfg.context_window must remain as default when ModelInfo.context_window is None"
        );
    }

    #[tokio::test]
    async fn test_cancel_replace() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-cancel").await;

        let token1 = CancellationToken::new();
        {
            let mut guard = room.cancel_token.lock().unwrap();
            *guard = Some(token1.clone());
        }
        assert!(!token1.is_cancelled());

        let token2 = CancellationToken::new();
        {
            let mut guard = room.cancel_token.lock().unwrap();
            if let Some(old) = guard.replace(token2.clone()) {
                old.cancel();
            }
        }
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancel_on_note_delete() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-del").await;

        let token = CancellationToken::new();
        {
            let mut guard = room.cancel_token.lock().unwrap();
            *guard = Some(token.clone());
        }

        // Simulate note deletion
        {
            let rooms = state.rooms.read().await;
            if let Some(r) = rooms.get("note-del") {
                let g = r.cancel_token.lock().unwrap();
                if let Some(ref t) = *g {
                    t.cancel();
                }
            }
        }
        state.rooms.write().await.remove("note-del");

        assert!(token.is_cancelled());
        assert!(state.rooms.read().await.get("note-del").is_none());
    }

    #[tokio::test]
    async fn test_streaming_tokens_accumulate_and_clear() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-stream").await;

        // Initially empty
        assert!(room.streaming_tokens.read().await.is_empty());

        // Simulate token accumulation
        {
            let mut buf = room.streaming_tokens.write().await;
            buf.push_str("Hello ");
            buf.push_str("world");
        }
        assert_eq!(*room.streaming_tokens.read().await, "Hello world");

        // Clear on done
        {
            let mut buf = room.streaming_tokens.write().await;
            buf.clear();
        }
        assert!(room.streaming_tokens.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_active_status_lifecycle() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-status").await;

        // Initially idle
        assert_eq!(*room.active_status.read().await, "idle");

        // Set to thinking (simulates chat start)
        {
            let mut s = room.active_status.write().await;
            *s = "thinking".to_string();
        }
        assert_eq!(*room.active_status.read().await, "thinking");

        // Set to tool status (simulates tool execution)
        {
            let mut s = room.active_status.write().await;
            *s = "tool:fetch_url".to_string();
        }
        // Verify strip_prefix works for reconnect logic
        {
            let status = room.active_status.read().await;
            assert_eq!(*status, "tool:fetch_url");
            assert_eq!(status.strip_prefix("tool:"), Some("fetch_url"));
        }

        // Set to typing (simulates token streaming)
        {
            let mut s = room.active_status.write().await;
            *s = "typing".to_string();
        }
        assert_eq!(*room.active_status.read().await, "typing");

        // Back to idle (simulates completion)
        {
            let mut s = room.active_status.write().await;
            *s = "idle".to_string();
        }
        assert_eq!(*room.active_status.read().await, "idle");
    }

    #[tokio::test]
    async fn test_sse_isolation_broadcast() {
        let (state, _tmp) = make_state();

        let room_a = state.get_or_create_room("note-a").await;
        let room_b = state.get_or_create_room("note-b").await;

        let mut rx_a = room_a.broadcast_tx.subscribe();
        let mut rx_b = room_b.broadcast_tx.subscribe();

        let _ = room_a.broadcast_tx.send("hello-a".into());

        let msg = rx_a.try_recv().unwrap();
        assert_eq!(msg, "hello-a");

        // Room B does NOT receive it
        assert!(rx_b.try_recv().is_err());
    }

    #[test]
    fn test_guest_allowed_events() {
        use crate::serve::api::is_guest_allowed_event;
        // Allowed
        assert!(is_guest_allowed_event("chat_token"));
        assert!(is_guest_allowed_event("chat_done"));
        assert!(is_guest_allowed_event("chat_message"));
        assert!(is_guest_allowed_event("chat_meta"));
        assert!(is_guest_allowed_event("file_content"));
        assert!(is_guest_allowed_event("file_list"));
        assert!(is_guest_allowed_event("note_list"));
        assert!(is_guest_allowed_event("auth_result"));
        assert!(is_guest_allowed_event("model_list"));
        assert!(is_guest_allowed_event("model_changed"));
        // Blocked (per spec)
        assert!(!is_guest_allowed_event("approval_request"));
        assert!(!is_guest_allowed_event("users_update"));
        assert!(!is_guest_allowed_event("status"));
        assert!(!is_guest_allowed_event("system"));
        assert!(!is_guest_allowed_event("search_results"));
        assert!(!is_guest_allowed_event("archive_done"));
        assert!(!is_guest_allowed_event("dir_browse_result"));
        assert!(!is_guest_allowed_event("history"));
    }

    #[tokio::test]
    async fn test_per_note_system_prompt() {
        let (state, _tmp) = make_state();
        let room = state.get_or_create_room("note-prompt").await;

        // Initially None
        assert!(room.system_prompt.read().await.is_none());

        // Set override
        *room.system_prompt.write().await = Some("You are a pirate.".into());
        let prompt = room.system_prompt.read().await.clone();
        assert_eq!(prompt, Some("You are a pirate.".into()));

        // Clear override
        *room.system_prompt.write().await = None;
        assert!(room.system_prompt.read().await.is_none());
    }

    #[tokio::test]
    async fn test_room_concurrent_create() {
        let (state, _tmp) = make_state();
        let state = Arc::new(state);

        // Spawn 10 tasks all trying to create the same room
        let mut handles = vec![];
        for _ in 0..10 {
            let s = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                s.get_or_create_room("concurrent-note").await
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // All should return the same Arc
        for r in &results[1..] {
            assert!(Arc::ptr_eq(&results[0], r));
        }

        // Only one room in map
        let rooms = state.rooms.read().await;
        assert_eq!(rooms.len(), 1);
    }

    #[tokio::test]
    async fn test_model_override_admin_only_via_api() {
        // Use tower to test that model/switch with note_id requires admin
        use axum::body::Body;
        use axum::{routing::post, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![
                ModelInfo {
                    provider: None,
                    id: "gpt-5-mini".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
                ModelInfo {
                    provider: None,
                    id: "claude-sonnet-4.6".into(),
                    context_window: None,
                    reasoning_efforts: vec![],
                    supported_endpoints: vec![],
                },
            ])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("gpt-5-mini".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        // Create note in DB so room can be created
        let _ = state.chat_db.create_note("test-note", "test-note", None);

        let app = Router::new()
            .route(
                "/api/model/switch",
                post(crate::serve::api::model_switch_handler),
            )
            .with_state(state.clone());

        // Admin can set per-note override
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/model/switch")
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"model":"claude-sonnet-4.6","note_id":"test-note"}"#,
            ))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Verify override took effect
        let effective = state.effective_model("test-note").await;
        assert_eq!(effective, "claude-sonnet-4.6");

        // Global default unchanged
        let global = state.global_default_model.read().await.clone();
        assert_eq!(global, "gpt-5-mini");
    }

    #[tokio::test]
    async fn test_sse_note_not_found() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let session = crate::serve::oauth::Session {
            id: "session-test".to_string(),
            login: "test-user".to_string(),
            role: crate::serve::oauth::Role::User,
            avatar_url: "".to_string(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        };
        state.sessions.insert(session).await;

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Request SSE with non-existent note_id
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?note_id=nonexistent&nickname=test")
            .header(axum::http::header::COOKIE, "rune_sid=session-test")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Body should contain error event
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert!(
            body_str.contains("Note not found"),
            "Expected 'Note not found' in: {}",
            body_str
        );
    }

    #[tokio::test]
    async fn test_guest_private_note_rejected() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        // Create a private note
        let _ = db.create_note("private-note", "private-note", None);

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let session = crate::serve::oauth::Session {
            id: "session-guest".to_string(),
            login: "guest-user".to_string(),
            role: crate::serve::oauth::Role::Guest,
            avatar_url: "".to_string(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        };
        state.sessions.insert(session).await;

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Guest trying to subscribe to private note
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?note_id=private-note&nickname=guest")
            .header(axum::http::header::COOKIE, "rune_sid=session-guest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert!(
            body_str.contains("Guests cannot access private notes"),
            "Expected auth_error in: {}",
            body_str
        );
    }

    #[tokio::test]
    async fn test_guest_public_note_allowed() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        // Create a public note
        let _ = db.create_note("public-note", "public-note", None);
        let _ = db.set_note_public("public-note", true);

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let session = crate::serve::oauth::Session {
            id: "session-guest".to_string(),
            login: "guest-user".to_string(),
            role: crate::serve::oauth::Role::Guest,
            avatar_url: "".to_string(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        };
        state.sessions.insert(session).await;

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Guest subscribing to public note — should succeed (200 OK)
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?note_id=public-note&nickname=guest")
            .header(axum::http::header::COOKIE, "rune_sid=session-guest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Read first frame with timeout (SSE streams forever, so we just check first data)
        let mut body = resp.into_body();
        let first_frame =
            tokio::time::timeout(std::time::Duration::from_millis(100), body.frame()).await;
        assert!(
            first_frame.is_ok(),
            "Should receive first SSE frame quickly"
        );
        let frame = first_frame.unwrap().unwrap().unwrap();
        let data = frame.into_data().unwrap();
        let text = String::from_utf8_lossy(&data);
        // First event should be auth_result
        assert!(
            text.contains("auth_result"),
            "Expected auth_result in first frame: {}",
            text
        );
    }

    #[tokio::test]
    async fn test_public_note_index_returns_public_files() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("main", "main", None);
        let _ = db.set_note_public("main", true);
        let _ = db.set_file_public("main", "OpenAI.md", true);
        let _ = db.set_file_public("main", "private.md", false);

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route(
                "/public/{note}/",
                get(crate::serve::api::public_note_index_handler),
            )
            .route(
                "/public/{note}",
                get(crate::serve::api::public_note_index_handler),
            )
            .with_state(state);

        // With trailing slash
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/public/main/")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes())
            .to_string();
        assert!(
            body.contains("OpenAI.md"),
            "Should show public file; got: {}",
            crate::config::safe_truncate(&body, 300)
        );
        assert!(!body.contains("private.md"), "Should NOT show private file");

        // Without trailing slash
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/public/main")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_public_note_index_private_note_returns_404() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("secret", "secret", None);
        // Note stays private (public=false by default)

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route(
                "/public/{note}/",
                get(crate::serve::api::public_note_index_handler),
            )
            .with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/public/secret/")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_public_notes_list_handler_shows_public_notes() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("pub-note", "pub-note", None);
        let _ = db.set_note_public("pub-note", true);
        let _ = db.set_file_public("pub-note", "hello.md", true);
        let _ = db.create_note("priv-note", "priv-note", None);
        // priv-note stays private

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route("/public", get(crate::serve::api::public_notes_list_handler))
            .route(
                "/public/",
                get(crate::serve::api::public_notes_list_handler),
            )
            .with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/public/")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes())
            .to_string();
        assert!(body.contains("pub-note"), "Should list public note");
        assert!(!body.contains("priv-note"), "Should NOT list private note");
        // Links must use /public/ prefix
        assert!(body.contains("/public/"), "Links must use /public/ prefix");
        assert!(
            !body.contains("/notes/"),
            "Links must NOT use /notes/ prefix"
        );
    }

    #[test]
    fn test_public_preview_html_highlight_dark_has_media() {
        use super::PUBLIC_PREVIEW_HTML;
        assert!(
            PUBLIC_PREVIEW_HTML
                .contains(r#"highlight-dark.min.css" media="(prefers-color-scheme: dark)""#),
            "PUBLIC_PREVIEW_HTML: highlight-dark must have media=(prefers-color-scheme: dark)"
        );
    }

    #[test]
    fn test_public_preview_html_highlight_light_has_media() {
        use super::PUBLIC_PREVIEW_HTML;
        assert!(
            PUBLIC_PREVIEW_HTML
                .contains(r#"highlight-light.min.css" media="(prefers-color-scheme: light)""#),
            "PUBLIC_PREVIEW_HTML: highlight-light.min.css must be loaded for light mode"
        );
    }

    #[tokio::test]
    async fn test_public_preview_handler_uses_public_route() {
        use axum::body::Body;
        use axum::{routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("mynote", "mynote", None);
        let _ = db.set_note_public("mynote", true);
        let _ = db.set_file_public("mynote", "doc.md", true);

        // Create the actual markdown file so the handler can serve it
        let md_dir = tmp
            .path()
            .join(".rune")
            .join("notes")
            .join("mynote")
            .join("markdown");
        std::fs::create_dir_all(&md_dir).unwrap();
        std::fs::write(md_dir.join("doc.md"), "# Hello").unwrap();

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "m1".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route(
                "/public/{note}/{file}",
                get(crate::serve::api::public_preview_handler),
            )
            .with_state(state);

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/public/mynote/doc")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes())
            .to_string();
        // Preview HTML must link back to /public/, not /notes/
        assert!(
            body.contains("/public/"),
            "Preview must use /public/ back-link"
        );
        assert!(
            !body.contains("/notes/"),
            "Preview must NOT use /notes/ back-link"
        );
    }

    #[tokio::test]
    async fn test_goal_set_clear_cancel_endpoints() {
        use axum::body::Body;
        use axum::{routing::post, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let _ = db.create_note("note-goal", "note-goal", None);

        let mut config = crate::config::RuneConfig::default();
        config.model = "mock-loop".to_string();
        config.provider = Some("mock-loop".to_string());
        config.api_key = Some("dummy-key".to_string());

        let state = crate::serve::ServerState {
            config,
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![ModelInfo {
                provider: None,
                id: "mock-loop".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("mock-loop".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route("/api/goal/set", post(crate::serve::api::goal_set_handler))
            .route(
                "/api/goal/clear",
                post(crate::serve::api::goal_clear_handler),
            )
            .route(
                "/api/chat/cancel",
                post(crate::serve::api::chat_cancel_handler),
            )
            .with_state(state.clone());

        // 1. Set goal
        let set_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/goal/set")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"note_id":"note-goal","condition":"Verify correctness","model":null}"#,
            ))
            .unwrap();

        let resp = app.clone().oneshot(set_req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let room = state.get_or_create_room("note-goal").await;
        {
            let cond = room.goal_condition.read().await;
            assert_eq!(cond.as_deref(), Some("Verify correctness"));
            let status = room.goal_status.read().await;
            assert_eq!(status.as_deref(), Some("Running"));
        }

        // 2. Cancel goal
        let cancel_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/chat/cancel")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note_id":"note-goal"}"#))
            .unwrap();

        let resp = app.clone().oneshot(cancel_req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        {
            let status = room.goal_status.read().await;
            // Should be Paused after cancellation
            assert_eq!(status.as_deref(), Some("Paused"));
        }

        // 3. Clear goal
        let clear_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/goal/clear")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"note_id":"note-goal"}"#))
            .unwrap();

        let resp = app.clone().oneshot(clear_req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        {
            let cond = room.goal_condition.read().await;
            assert!(cond.is_none());
            let status = room.goal_status.read().await;
            assert!(status.is_none());
        }
    }
}
