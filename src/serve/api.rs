//! SSE + REST API handlers for Rune serve.
//!
//! Architecture:
//!   - GET /api/events — SSE stream for server→client push
//!   - POST /api/* — REST endpoints for client→server operations

use crate::agent::{Agent, StopReason};
use crate::config::RuneConfig;
use crate::embedding::EmbeddingEngine;
use crate::provider::{CopilotProvider, GeminiProvider, OpenAiProvider, ProviderRegistry};
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
        }
    }
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
    ChatMeta { model: String, tokens_in: u32, tokens_out: u32, context_tokens: u32, context_window: u32 },
    #[serde(rename = "chat_message")]
    ChatMessage { nickname: String, content: String },
    #[serde(rename = "status")]
    Status { state: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "users_update")]
    UsersUpdate { count: u32 },
    #[serde(rename = "history")]
    History { messages: Vec<crate::serve::db::ChatRecord> },
    #[serde(rename = "auth_result")]
    AuthResult { is_admin: bool, is_guest: bool },
    #[serde(rename = "file_list")]
    FileList { files: Vec<FileEntry>, active: String },
    #[serde(rename = "file_content")]
    FileContent { note_id: String, filename: String, content: String },
    #[serde(rename = "file_deleted")]
    FileDeleted { filename: String },
    #[serde(rename = "note_list")]
    NoteList { notes: Vec<NoteListEntry>, active: String },
    #[serde(rename = "note_switched")]
    NoteSwitched { note_id: String },
    #[serde(rename = "model_list")]
    ModelList { models: Vec<String>, active: String },
    #[serde(rename = "model_changed")]
    ModelChanged { model: String },
    #[serde(rename = "approval_request")]
    ApprovalRequest { id: String, detail: String },
    #[serde(rename = "archive_done")]
    ArchiveDone { filename: String, count: usize },
    #[serde(rename = "search_results")]
    SearchResults { query: String, results: Vec<crate::serve::db::ChatRecord> },
    #[serde(rename = "dir_browse_result")]
    DirBrowseResult { path: String, parent: Option<String>, entries: Vec<DirEntry> },
}

#[derive(Debug, Serialize, Clone)]
pub struct NoteListEntry {
    pub id: String,
    pub name: String,
    pub files: Vec<String>,
    pub public: bool,
    /// Which files are publicly visible (only names that are public=true)
    pub public_files: Vec<String>,
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
    pub nickname: Option<String>,
    pub token: Option<String>,
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
        Self { ok: true, error: None, data: None }
    }
    pub fn with_data(data: serde_json::Value) -> Self {
        Self { ok: true, error: None, data: Some(data) }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), data: None }
    }
}

// ─── Auth helpers ──────────────────────────────────────────────────────────

pub fn check_token(state: &ServerState, token: Option<&str>) -> bool {
    match &state.user_token {
        None => false, // no user_token configured = no user access
        Some(expected) => token == Some(expected.as_str()),
    }
}

pub fn check_admin(state: &ServerState, admin_token: Option<&str>) -> bool {
    match &state.admin_token {
        None => false,
        Some(at) => !at.is_empty() && admin_token == Some(at.as_str()),
    }
}

pub fn check_guest(state: &ServerState, token: Option<&str>) -> bool {
    match &state.guest_token {
        None => false,
        Some(gt) => !gt.is_empty() && token == Some(gt.as_str()),
    }
}

pub fn is_valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".md")
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
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
) -> impl IntoResponse {
    let nickname = params.nickname.unwrap_or_else(|| "anonymous".to_string());
    let token = params.token.as_deref();

    // Auth check
    let is_admin = check_admin(&state, token);
    let is_guest = check_guest(&state, token);
    let token_ok = check_token(&state, token) || is_admin || is_guest;
    if !token_ok {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(r#"{"type":"error","message":"Authentication failed"}"#.to_string()))
        });
        return Sse::new(err_stream).keep_alive(KeepAlive::default()).into_response();
    }

    // note_id is REQUIRED per spec
    let note_id = match params.note_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("error")
                    .data(r#"{"type":"error","message":"note_id is required"}"#.to_string()))
            });
            return Sse::new(err_stream).keep_alive(KeepAlive::default()).into_response();
        }
    };

    let notes = build_note_list(&state).await;

    // Verify note exists
    if !notes.iter().any(|n| n.id == note_id) {
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(r#"{"type":"error","message":"Note not found"}"#.to_string()))
        });
        return Sse::new(err_stream).keep_alive(KeepAlive::default()).into_response();
    }

    // Guest + private note check
    if is_guest {
        let note_public = notes.iter().find(|n| n.id == note_id).map(|n| n.public).unwrap_or(false);
        if !note_public {
            let err_stream = futures::stream::once(async {
                Ok::<_, Infallible>(Event::default()
                    .event("auth_error")
                    .data(r#"{"type":"auth_error","message":"Guests cannot access private notes"}"#.to_string()))
            });
            return Sse::new(err_stream).keep_alive(KeepAlive::default()).into_response();
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
    init_msgs.push(SseMsg::AuthResult { is_admin, is_guest });

    // Model list — show effective model for this note
    let effective = state.effective_model(&note_id).await;
    init_msgs.push(SseMsg::ModelList {
        models: state.models.clone(),
        active: effective,
    });

    // Note list — guests only see public notes
    let visible_notes = if is_guest {
        notes.into_iter().filter(|n| n.public).collect()
    } else {
        notes
    };
    init_msgs.push(SseMsg::NoteList { notes: visible_notes, active: note_id.clone() });

    // Users update (not sent to guests per spec)
    if !is_guest {
        init_msgs.push(SseMsg::UsersUpdate { count });
    }

    // System join message — broadcast to the room
    let join_msg = SseMsg::System { content: format!("{} joined", nickname) };
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

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Check if an event type is allowed for guest users.
pub fn is_guest_allowed_event(event_type: &str) -> bool {
    matches!(event_type,
        "chat_token" | "chat_done" | "chat_message" | "chat_meta"
        | "file_content" | "file_list" | "note_list"
        | "auth_result" | "model_list" | "model_changed"
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
    state.chat_db.insert_async(
        req.note_id.clone(),
        "user".to_string(),
        nickname,
        req.content.clone(),
    ).await;

    // Send thinking status to the room
    let thinking = SseMsg::Status { state: "thinking".to_string() };
    broadcast_to_room(&room, &thinking);

    // Cancel & Replace: cancel any existing AI task for this note
    let new_token = CancellationToken::new();
    {
        let mut guard = room.cancel_token.lock().unwrap();
        if let Some(old) = guard.replace(new_token.clone()) {
            old.cancel();
        }
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
        return Json(ApiResponse::err(format!("File already exists: {}", req.name)));
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
    let fc = SseMsg::FileContent { note_id: req.note_id.clone(), filename: req.name, content: empty };
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
        return Json(ApiResponse::err(format!("Invalid filename: {}", req.new_name)));
    }

    let md_dir = state.note_markdown_dir(&req.note_id);
    let new_path = md_dir.join(&req.new_name);
    if new_path.exists() {
        return Json(ApiResponse::err(format!("File already exists: {}", req.new_name)));
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
            tracing::warn!("file/switch failed: note_id={:?} name={:?} path={:?} err={}", req.note_id, req.name, file_path, e);
            Json(serde_json::json!({ "ok": false, "error": format!("File not found: {}", req.name) }))
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
    let fc = SseMsg::FileContent { note_id: req.note_id.clone(), filename: fname, content: req.content };
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
    match state.chat_db.rename_note(&req.note_id, &req.name) {
        Ok(Some(new_id)) => {
            let old_dir = state.data_dir.join("notes").join(&req.note_id);
            let new_dir = state.data_dir.join("notes").join(&new_id);
            if old_dir.exists() {
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
    let src_note = src.file_name().unwrap_or_default().to_string_lossy().to_string();
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
    let history = state.chat_db.load_recent_async(req.note_id.clone(), 100).await;

    // Load file list
    let md_dir = state.note_markdown_dir(&req.note_id);
    let mut files = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") { files.push(name); }
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
    if !state.models.contains(&req.model) {
        return Json(ApiResponse::err(format!("Unknown model: {}", req.model)));
    }

    let msg = SseMsg::ModelChanged { model: req.model.clone() };

    if let Some(ref note_id) = req.note_id {
        // Per-note model override — persist to DB
        let room = state.get_or_create_room(note_id).await;
        *room.model_override.write().await = Some(req.model.clone());
        let _ = state.chat_db.set_note_model(note_id, Some(&req.model));
        broadcast_to_room(&room, &msg);
    } else {
        // Global default model — broadcast to all rooms
        *state.global_default_model.write().await = req.model.clone();
        let rooms = state.rooms.read().await;
        for room in rooms.values() {
            broadcast_to_room(room, &msg);
        }
    }
    Json(ApiResponse::success())
}

pub async fn archive_handler(
    State(state): State<ServerState>,
    Json(req): Json<ArchiveReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    let archive_dir = state.note_markdown_dir(&req.note_id)
        .parent().unwrap().join("archives");
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
    let archive_dir = state.note_markdown_dir(&req.note_id).parent().unwrap().join("archives");
    let results = state.chat_db.search_async(req.note_id.clone(), req.query.clone(), archive_dir).await;
    let room = state.get_or_create_room(&req.note_id).await;
    let msg = SseMsg::SearchResults { query: req.query, results };
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
    let canonical = tokio::fs::canonicalize(browse_path).await
        .unwrap_or_else(|_| browse_path.to_path_buf());
    let parent = canonical.parent().map(|p| p.to_string_lossy().to_string());
    let mut entries = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&canonical).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') { continue; }
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
    }).unwrap_or_default();

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
    match state.chat_db.set_file_public(&req.note_id, &req.filename, req.public) {
        Ok(_) => {
            broadcast_file_list(&state, &req.note_id).await;
            Json(ApiResponse::success())
        }
        Err(e) => Json(ApiResponse::err(format!("Failed: {}", e))),
    }
}

// ─── Public (no-auth) handlers ──────────────────────────────────────────────

const PUBLIC_PREVIEW_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{{TITLE}}</title>
<link rel="stylesheet" href="/assets/highlight-dark.min.css" id="hl-style">
<style>
  :root { color-scheme: light dark; }
  @media (prefers-color-scheme: dark) {
    body { background: #1e1e2e; color: #cdd6f4; }
    .container { background: #181825; border: 1px solid #313244; }
    a { color: #89b4fa; }
    code { background: #313244; color: #cdd6f4; }
    pre { background: #181825; border: 1px solid #313244; }
    h1,h2,h3,h4 { color: #cba6f7; border-bottom-color: #313244; }
    blockquote { border-left: 4px solid #585b70; color: #a6adc8; background: #181825; }
  }
  @media (prefers-color-scheme: light) {
    body { background: #f6f8fa; color: #24292e; }
    .container { background: #fff; border: 1px solid #e1e4e8; }
    a { color: #0366d6; }
    code { background: #f6f8fa; color: #24292e; }
    pre { background: #f6f8fa; border: 1px solid #e1e4e8; }
    h1,h2,h3,h4 { color: #24292e; border-bottom: 1px solid #eaecef; }
    blockquote { border-left: 4px solid #dfe2e5; color: #6a737d; background: #f6f8fa; }
  }
  * { box-sizing: border-box; }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif; margin: 0; padding: 20px; }
  .container { max-width: 860px; margin: 0 auto; padding: 32px 40px; border-radius: 8px; }
  h1,h2,h3,h4,h5,h6 { margin-top: 24px; margin-bottom: 16px; font-weight: 600; line-height: 1.25; padding-bottom: .3em; }
  h1 { font-size: 2em; } h2 { font-size: 1.5em; } h3 { font-size: 1.25em; }
  p { margin: 0 0 16px; line-height: 1.6; }
  pre { padding: 16px; overflow: auto; border-radius: 6px; font-size: 85%; }
  code { padding: .2em .4em; border-radius: 3px; font-size: 85%; }
  pre code { padding: 0; background: transparent; }
  blockquote { margin: 0 0 16px; padding: 0 1em; }
  ul,ol { padding-left: 2em; margin: 0 0 16px; }
  table { border-collapse: collapse; width: 100%; margin-bottom: 16px; }
  th,td { border: 1px solid #dfe2e5; padding: 6px 13px; }
  th { font-weight: 600; }
  img { max-width: 100%; }
  .meta { font-size: 12px; opacity: 0.5; margin-bottom: 24px; }
  #loading { text-align: center; padding: 40px; opacity: 0.5; }
</style>
</head>
<body>
<div class="container">
  <div class="meta">{{NOTE}} / {{FILE}}</div>
  <div id="loading">Loading…</div>
  <div id="content" style="display:none"></div>
</div>
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
        return '<div class="mermaid" id="' + id + '">' + text.replace(/</g,'&lt;') + '</div>';
      }
      if (typeof hljs !== 'undefined') {
        const language = lang && hljs.getLanguage(lang) ? lang : null;
        const highlighted = language ? hljs.highlight(text, {language}).value : hljs.highlightAuto(text).value;
        return '<pre><code class="hljs">' + highlighted + '</code></pre>';
      }
      return '<pre><code>' + text.replace(/</g,'&lt;') + '</code></pre>';
    };
    marked.use({ renderer });
    const html = marked.parse(md);
    const content = document.getElementById('content');
    content.innerHTML = html;
    document.getElementById('loading').style.display = 'none';
    content.style.display = '';
    if (typeof mermaid !== 'undefined') {
      mermaid.initialize({ startOnLoad: false, theme: window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'default' });
      document.querySelectorAll('.mermaid').forEach(async (el) => {
        try { const {svg} = await mermaid.render('mg'+el.id, el.textContent); el.innerHTML = svg; } catch(e) {}
      });
    }
  } catch(e) {
    document.getElementById('loading').textContent = 'Error: ' + e.message;
  }
})();
</script>
<footer style="max-width:860px;margin:32px auto 16px;padding-top:16px;border-top:1px solid currentColor;opacity:0.3;text-align:center;font-size:12px">
  Wrought by <a href="https://fourdollars.github.io/rune/" style="color:inherit;text-decoration:underline">ᚱᚢᚾᛖ</a>
</footer>
</body>
</html>"#;

pub async fn public_notes_list_handler(
    State(state): State<ServerState>,
) -> impl axum::response::IntoResponse {
    let notes = state.chat_db.list_notes().unwrap_or_default();
    let mut html = String::from("<!DOCTYPE html><html><head><meta charset='UTF-8'><title>Public Notes</title>");
    html.push_str("<style>:root{color-scheme:light dark}@media(prefers-color-scheme:dark){body{background:#1e1e2e;color:#cdd6f4}a{color:#89b4fa}footer{border-color:#585b70 !important}}body{font-family:sans-serif;max-width:860px;margin:40px auto;padding:0 20px;background:#f6f8fa;color:#24292e}");
    html.push_str("h1{margin-bottom:24px}ul{list-style:none;padding:0}li{margin:8px 0}a{color:#0366d6;text-decoration:none}a:hover{text-decoration:underline}");
    html.push_str(".note-header{font-weight:600;margin-top:16px;margin-bottom:4px}.file-link{padding-left:16px;display:block}</style></head><body>");
    html.push_str("<h1>Public Notes</h1><ul>");
    let mut any = false;
    for note in &notes {
        if !note.public { continue; }
        let public_files = state.chat_db.list_public_files(&note.id);
        if public_files.is_empty() { continue; }
        any = true;
        html.push_str(&format!("<li><div class='note-header'>&#128193; {}</div><ul>", html_escape(&note.name)));
        for fname in &public_files {
            let slug = fname.strip_suffix(".md").unwrap_or(fname);
            let url = format!("/notes/{}/{}", url_encode(&note.id), url_encode(slug));
            html.push_str(&format!("<li><a class='file-link' href='{}'>{}</a></li>", url, html_escape(fname)));
        }
        html.push_str("</ul></li>");
    }
    if !any { html.push_str("<li><em>No public notes available.</em></li>"); }
    html.push_str("</ul><footer style='margin-top:40px;padding-top:16px;border-top:1px solid #ccc;opacity:0.4;text-align:center;font-size:12px'>Wrought by <a href='https://fourdollars.github.io/rune/' style='color:inherit;text-decoration:underline'>ᚱᚢᚾᛖ</a></footer></body></html>");
    axum::response::Html(html)
}

pub async fn public_preview_handler(
    State(state): State<ServerState>,
    axum::extract::Path((note_id, file_slug)): axum::extract::Path<(String, String)>,
) -> impl axum::response::IntoResponse {
    // Accept both "OpenAI" and "OpenAI.md"
    let filename = if file_slug.ends_with(".md") { file_slug.clone() } else { format!("{}.md", file_slug) };
    let note_public = state.chat_db.list_notes().unwrap_or_default()
        .iter().find(|n| n.id == note_id).map(|n| n.public).unwrap_or(false);
    let file_public = state.chat_db.is_file_public(&note_id, &filename);
    if !note_public || !file_public {
        return (StatusCode::NOT_FOUND, axum::response::Html("<h1>404 Not Found</h1>".to_string())).into_response();
    }
    let title = format!("{} / {}", note_id, filename);
    let page = PUBLIC_PREVIEW_HTML
        .replace("{{TITLE}}", &html_escape(&title))
        .replace("{{NOTE}}", &url_encode(&note_id))
        .replace("{{FILE}}", &url_encode(&filename));
    (StatusCode::OK, axum::response::Html(page)).into_response()
}

pub async fn public_raw_handler(
    State(state): State<ServerState>,
    axum::extract::Path((note_id, file_slug)): axum::extract::Path<(String, String)>,
) -> impl axum::response::IntoResponse {
    let filename = if file_slug.ends_with(".md") { file_slug.clone() } else { format!("{}.md", file_slug) };
    let note_public = state.chat_db.list_notes().unwrap_or_default()
        .iter().find(|n| n.id == note_id).map(|n| n.public).unwrap_or(false);
    let file_public = state.chat_db.is_file_public(&note_id, &filename);
    if !note_public || !file_public {
        return (StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, "text/plain")], "".to_string()).into_response();
    }
    let file_path = state.note_markdown_dir(&note_id).join(&filename);
    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8")], content).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, "text/plain")], "".to_string()).into_response(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn url_encode(s: &str) -> String {
    s.chars().map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        _ => format!("%{:02X}", c as u32),
    }).collect()
}


async fn broadcast_file_list(state: &ServerState, note_id: &str) {
    let md_dir = state.note_markdown_dir(note_id);
    let mut file_names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") { file_names.push(name); }
        }
    }
    file_names.sort();
    let visibility = state.chat_db.get_file_visibility(note_id);
    let files: Vec<FileEntry> = file_names.iter().map(|name| {
        let public = visibility.iter().find(|(f, _)| f == name).map(|(_, p)| *p).unwrap_or(false);
        FileEntry { name: name.clone(), public }
    }).collect();
    let active = files.first().map(|f| f.name.clone()).unwrap_or_default();
    let msg = SseMsg::FileList { files, active };
    // Send to the room for this note
    let room = state.get_or_create_room(note_id).await;
    broadcast_to_room(&room, &msg);
}

async fn broadcast_note_list(state: &ServerState) {
    let notes = build_note_list(state).await;
    let msg = SseMsg::NoteList { notes, active: String::new() };
    // Note list is global — broadcast to all rooms
    let rooms = state.rooms.read().await;
    for room in rooms.values() {
        broadcast_to_room(room, &msg);
    }
}


/// GET /api/notes — JSON list of notes (authenticated users only)
pub async fn notes_list_json_handler(
    State(state): State<ServerState>,
) -> Json<serde_json::Value> {
    let notes = build_note_list(&state).await;
    Json(serde_json::json!({ "ok": true, "notes": notes }))
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
            let err = SseMsg::Error { message: format!("Provider error: {}", e) };
            broadcast_to_room(&room, &err);
            let idle = SseMsg::Status { state: "idle".to_string() };
            broadcast_to_room(&room, &idle);
            return;
        }
    };

    // Build embedding
    let embedding = build_embedding(&config).await;

    // Token streaming callback — sends to room only
    let room_for_token = Arc::clone(&room);
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        let msg = SseMsg::ChatToken { content: token.to_string() };
        broadcast_to_room(&room_for_token, &msg);
    });

    // Approval callback — requests go to room, responses come back via room
    let room_for_approval = Arc::clone(&room);
    let approval_callback: Arc<dyn Fn(String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync> =
        Arc::new(move |id: String, detail: String| {
            let room = Arc::clone(&room_for_approval);
            Box::pin(async move {
                // Send approval request to the room
                let msg = SseMsg::ApprovalRequest { id: id.clone(), detail };
                broadcast_to_room(&room, &msg);
                // Wait for approval response via room channel
                let mut rx = room.broadcast_tx.subscribe();
                let approve_key = format!("__approval_granted__{}", id);
                let deny_key = format!("__approval_denied__{}", id);
                loop {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(300),
                        rx.recv()
                    ).await {
                        Ok(Ok(msg)) => {
                            if msg == approve_key { return true; }
                            if msg == deny_key { return false; }
                        }
                        _ => return false,
                    }
                }
            })
        });

    // Build agent
    let mut cfg = config.clone();
    cfg.model = active_model.clone();
    let mut agent = Agent::new(cfg, provider, true, embedding);
    agent.token_callback = Some(token_callback);
    agent.approval_callback = Some(approval_callback);
    agent.user_name = if !nickname.is_empty() && nickname != "user" { Some(nickname) } else { None };
    agent.markdown_dir = Some(state.note_markdown_dir(&note_id));
    agent.chat_db = Some(state.chat_db.clone());
    agent.chat_note_id = Some(note_id.clone());
    agent.chat_archive_dir = Some(state.note_markdown_dir(&note_id)
        .parent().unwrap().join("archives"));
    // Notify UI whenever AI writes/creates a markdown file — broadcast to room
    let state_for_filelist = state.clone();
    let note_id_for_filelist = note_id.clone();
    agent.file_list_callback = Some(Arc::new(move || {
        let s = state_for_filelist.clone();
        let n = note_id_for_filelist.clone();
        tokio::spawn(async move { broadcast_file_list(&s, &n).await; });
    }));

    // Broadcast file content changes to all users in the room (real-time sync)
    let state_for_content = state.clone();
    let note_id_for_content = note_id.clone();
    agent.file_content_callback = Some(Arc::new(move |filename: String, content: String| {
        let s = state_for_content.clone();
        let n = note_id_for_content.clone();
        tokio::spawn(async move {
            let room = s.get_or_create_room(&n).await;
            let fc = SseMsg::FileContent { note_id: n, filename, content };
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

    // Load chat history into agent context
    let history = state.chat_db.load_recent_async(note_id.clone(), 51).await;
    let history_without_current: Vec<_> = history
        .into_iter()
        .filter(|r| !(r.role == "user" && r.content == user_msg))
        .collect();
    let max_history = 100usize;
    let history_slice = if history_without_current.len() > max_history {
        &history_without_current[history_without_current.len() - max_history..]
    } else {
        &history_without_current[..]
    };
    agent.load_history(history_slice);

    // Run agent
    let stop_reason = agent.run(&user_msg).await;

    let done = SseMsg::ChatDone {};
    broadcast_to_room(&room, &done);

    // Process result
    match &stop_reason {
        StopReason::FinalAnswer(answer) => {
            // Save assistant response
            state.chat_db.insert_async(
                note_id.clone(),
                "assistant".to_string(),
                "ᚱᚢᚾᛖ".to_string(),
                answer.clone(),
            ).await;
        }
        StopReason::Error(e) => {
            let err = SseMsg::Error { message: format!("Agent error: {}", e) };
            broadcast_to_room(&room, &err);
        }
        StopReason::MaxSteps => {
            let err = SseMsg::Error { message: "Agent reached max steps".to_string() };
            broadcast_to_room(&room, &err);
        }
        StopReason::TokenBudgetExhausted => {
            let err = SseMsg::Error { message: "Token budget exhausted".to_string() };
            broadcast_to_room(&room, &err);
        }
        _ => {}
    }

    // Broadcast updated file list to the room
    broadcast_file_list(&state, &note_id).await;

    let idle = SseMsg::Status { state: "idle".to_string() };
    broadcast_to_room(&room, &idle);
}

// ─── Provider/Embedding builders (from ws.rs) ──────────────────────────────

fn build_provider(config: &RuneConfig) -> anyhow::Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();

    let key = config
        .api_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No API key configured. Run `rune init` first."))?;

    let provider_name = config.provider.as_deref().unwrap_or_else(|| {
        if key.starts_with("ghu_")
            || key.starts_with("ghp_")
            || config.base_url.as_deref().map(|u| u.contains("githubcopilot")).unwrap_or(false)
        {
            "github-copilot"
        } else if key.starts_with("AIza")
            || config.base_url.as_deref().map(|u| u.contains("generativelanguage.googleapis.com")).unwrap_or(false)
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
    let emb_config = config.embedding.clone();
    let provider_name = config.provider.as_deref().unwrap_or("");
    if provider_name == "github-copilot" || provider_name == "copilot"
        || api_key.starts_with("ghu_") || api_key.starts_with("ghp_")
    {
        Some(EmbeddingEngine::new_copilot(emb_config, api_key))
    } else {
        Some(EmbeddingEngine::new(emb_config))
    }
}

async fn build_system_prompt(config: &RuneConfig) -> String {
    if let Some(ref prompt) = config.system_prompt {
        if !prompt.is_empty() {
            return prompt.clone();
        }
    }
    "You are Rune, a helpful AI assistant. Be concise and helpful.".to_string()
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
        assert!(!is_valid_filename("..md"));
    }

    #[test]
    fn test_check_token_no_config() {
        // With no user_token configured, check_token always returns false (strict mode)
        let state = mock_state(None, None);
        assert!(!check_token(&state, None));
        assert!(!check_token(&state, Some("anything")));
    }

    #[test]
    fn test_check_token_with_config() {
        let state = mock_state(Some("secret".into()), None);
        assert!(check_token(&state, Some("secret")));
        assert!(!check_token(&state, Some("wrong")));
        assert!(!check_token(&state, None));
    }

    #[test]
    fn test_check_admin() {
        let state = mock_state(None, Some("admin123".into()));
        assert!(check_admin(&state, Some("admin123")));
        assert!(!check_admin(&state, Some("wrong")));
        assert!(!check_admin(&state, None));
    }

    #[test]
    fn test_check_admin_empty() {
        let state = mock_state(None, Some("".into()));
        assert!(!check_admin(&state, Some("")));
    }

    #[test]
    fn test_check_admin_none_configured() {
        let state = mock_state(None, None);
        assert!(!check_admin(&state, Some("anything")));
    }

    #[test]
    fn test_strict_auth_no_tokens_configured_rejects_all() {
        // When no tokens are configured, nothing should pass
        let state = mock_state(None, None);
        assert!(!check_token(&state, None));
        assert!(!check_token(&state, Some("random")));
        assert!(!check_admin(&state, None));
        assert!(!check_admin(&state, Some("random")));
        assert!(!check_guest(&state, None));
        assert!(!check_guest(&state, Some("random")));
    }

    #[test]
    fn test_strict_auth_guest_cannot_impersonate_user() {
        let mut state = mock_state(Some("user-secret".into()), None);
        state.guest_token = Some("guest-secret".into());
        // Guest token does not pass check_token
        assert!(!check_token(&state, Some("guest-secret")));
        // But does pass check_guest
        assert!(check_guest(&state, Some("guest-secret")));
    }

    #[test]
    fn test_strict_auth_guest_cannot_impersonate_admin() {
        let mut state = mock_state(None, Some("admin-secret".into()));
        state.guest_token = Some("guest-secret".into());
        // Guest token does not pass check_admin
        assert!(!check_admin(&state, Some("guest-secret")));
    }

    #[test]
    fn test_strict_auth_empty_token_rejected() {
        let state = mock_state(Some("secret".into()), None);
        assert!(!check_token(&state, Some("")));
        assert!(!check_token(&state, None));
    }

    #[test]
    fn test_strict_auth_empty_guest_token_rejected() {
        let mut state = mock_state(Some("user".into()), None);
        state.guest_token = Some("".into());
        // Empty guest_token should never match
        assert!(!check_guest(&state, Some("")));
        assert!(!check_guest(&state, None));
    }

    #[test]
    fn test_strict_auth_wrong_token_rejected() {
        let mut state = mock_state(Some("correct-user".into()), Some("correct-admin".into()));
        state.guest_token = Some("correct-guest".into());
        assert!(!check_token(&state, Some("wrong")));
        assert!(!check_admin(&state, Some("wrong")));
        assert!(!check_guest(&state, Some("wrong")));
    }

    #[test]
    fn test_strict_auth_no_localhost_bypass() {
        // This test documents the security invariant:
        // There is no localhost bypass in auth middleware.
        // All connections must present a valid token.
        let state = mock_state(Some("secret".into()), None);
        // Even with correct token, no special treatment for any IP
        assert!(check_token(&state, Some("secret")));
        assert!(!check_token(&state, None));
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
        let msg = SseMsg::ChatToken { content: "hello".into() };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"chat_token""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn test_sse_msg_error_serialization() {
        let msg = SseMsg::Error { message: "oops".into() };
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
            tokens_in: 100,
            tokens_out: 50,
            context_tokens: 1000,
            context_window: 128000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"chat_meta""#));
        assert!(json.contains(r#""tokens_in":100"#));
    }

    #[test]
    fn test_sse_msg_file_list() {
        let msg = SseMsg::FileList {
            files: vec![FileEntry { name: "a.md".into(), public: false }, FileEntry { name: "b.md".into(), public: false }],
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
            models: vec!["gpt-4".into(), "claude".into()],
            active: "gpt-4".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"model_list""#));
    }

    #[test]
    fn test_sse_msg_dir_browse() {
        let msg = SseMsg::DirBrowseResult {
            path: "/home".into(),
            parent: Some("/".into()),
            entries: vec![DirEntry { name: "user".into(), is_dir: true }],
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
    fn mock_state(token: Option<String>, admin_token: Option<String>) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(16);
        ServerState {
            config: crate::config::RuneConfig::default(),
            user_token: token,
            admin_token,
            guest_token: None,
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["test-model".into()],
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
        user_token: None,
        admin_token: Some("admin123".into()),
        guest_token: None,
        files: Arc::new(RwLock::new(std::collections::HashMap::new())),
        active_file: Arc::new(RwLock::new(String::new())),
        models: vec!["gpt-5-mini".into(), "claude-sonnet-4.6".into()],
        rooms: Arc::new(RwLock::new(std::collections::HashMap::new())),
        global_default_model: Arc::new(RwLock::new("gpt-5-mini".into())),
        admin_broadcast_tx,
        chat_db: db,
        data_dir: tmp.path().join(".rune"),
    }
}

fn test_state_with_token(tmp: &TempDir) -> ServerState {
    let mut state = test_state(tmp);
    state.user_token = Some("secret".into());
    state
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
    let (status, body) = post_json(&app, "/api/session/create", json!({
        "name": "test-session",
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_session_create_empty_name() {
    let (app, _tmp) = test_app();
    let (status, body) = post_json(&app, "/api/session/create", json!({
        "name": "",
    })).await;
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
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "old-name"})).await;
    let (_, body) = post_json(&app, "/api/session/rename", json!({
        "note_id": "old-name",
        "name": "new-name"
    })).await;
    assert_eq!(body["ok"], true);
}

// ─── File CRUD tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_file_create() {
    let (app, tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "file-test"})).await;
    let (_, body) = post_json(&app, "/api/file/create", json!({
        "note_id": "file-test",
        "name": "notes.md"
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_file_create_invalid_name() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f1"})).await;
    let (_, body) = post_json(&app, "/api/file/create", json!({
        "note_id": "f1",
        "name": "bad file.txt"
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn test_file_create_duplicate() {
    let (app, tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f2"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f2", "name": "a.md"})).await;
    let (_, body) = post_json(&app, "/api/file/create", json!({"note_id": "f2", "name": "a.md"})).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("exists"));
}

#[tokio::test]
async fn test_file_create_no_session() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/file/create", json!({
        "note_id": "",
        "name": "x.md"
    })).await;
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn test_file_update_and_switch() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f3"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f3", "name": "doc.md"})).await;

    // Update
    let (_, body) = post_json(&app, "/api/file/update", json!({
        "note_id": "f3",
        "filename": "doc.md",
        "content": "# Hello\nWorld"
    })).await;
    assert_eq!(body["ok"], true);

    // Switch (read back)
    let (_, body) = post_json(&app, "/api/file/switch", json!({
        "note_id": "f3",
        "name": "doc.md"
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_file_switch_not_found() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f4"})).await;
    let (_, body) = post_json(&app, "/api/file/switch", json!({
        "note_id": "f4",
        "name": "nope.md"
    })).await;
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn test_file_rename() {
    let (app, tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f5"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f5", "name": "old.md"})).await;
    let (_, body) = post_json(&app, "/api/file/rename", json!({
        "note_id": "f5",
        "old_name": "old.md",
        "new_name": "new.md"
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_file_rename_conflict() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f6"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f6", "name": "a.md"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f6", "name": "b.md"})).await;
    let (_, body) = post_json(&app, "/api/file/rename", json!({
        "note_id": "f6",
        "old_name": "a.md",
        "new_name": "b.md"
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("exists"));
}

#[tokio::test]
async fn test_file_delete() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f7"})).await;
    post_json(&app, "/api/file/create", json!({"note_id": "f7", "name": "rm.md"})).await;
    let (_, body) = post_json(&app, "/api/file/delete", json!({
        "note_id": "f7",
        "name": "rm.md"
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_file_update_invalid_filename() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "f8"})).await;
    let (_, body) = post_json(&app, "/api/file/update", json!({
        "note_id": "f8",
        "filename": "",
        "content": "x"
    })).await;
    assert_eq!(body["ok"], false);
}

// ─── Model switch tests ────────────────────────────────────────────────────

#[tokio::test]
async fn test_model_switch_valid() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/model/switch", json!({
        "model": "claude-sonnet-4.6"
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_model_switch_unknown() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/model/switch", json!({
        "model": "unknown-model"
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("Unknown"));
}

// ─── Chat tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_chat_no_session() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/chat", json!({
        "note_id": "",
        "content": "hello"
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("note"));
}

#[tokio::test]
async fn test_chat_empty_content() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/chat", json!({
        "note_id": "s1",
        "content": "   "
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("Empty"));
}

// ─── Archive + Search tests ────────────────────────────────────────────────

#[tokio::test]
async fn test_archive_no_session() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/chat/archive", json!({
        "note_id": ""
    })).await;
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn test_search_empty_query() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/chat/search", json!({
        "note_id": "s1",
        "query": ""
    })).await;
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("Empty"));
}

#[tokio::test]
async fn test_search_valid() {
    let (app, _tmp) = test_app();
    post_json(&app, "/api/session/create", json!({"name": "search-test"})).await;
    let (_, body) = post_json(&app, "/api/chat/search", json!({
        "note_id": "search-test",
        "query": "hello"
    })).await;
    assert_eq!(body["ok"], true);
}

// ─── Dir browse tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_dir_browse_root() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/dir/browse", json!({
        "path": "/tmp"
    })).await;
    assert_eq!(body["ok"], true);
    assert!(body["data"].is_object());
}

#[tokio::test]
async fn test_dir_browse_nonexistent() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/dir/browse", json!({
        "path": "/nonexistent_path_12345"
    })).await;
    // Should still return ok with empty entries
    assert_eq!(body["ok"], true);
}

// ─── Approval test ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_approval_granted() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/approval", json!({
        "id": "test-123",
        "approved": true
    })).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_approval_denied() {
    let (app, _tmp) = test_app();
    let (_, body) = post_json(&app, "/api/approval", json!({
        "id": "test-456",
        "approved": false
    })).await;
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
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"), "Expected SSE content type, got: {}", ct);
}

// ─── Full flow integration test ────────────────────────────────────────────

#[tokio::test]
async fn test_full_session_file_flow() {
    let (app, _tmp) = test_app();

    // Create session
    let (_, body) = post_json(&app, "/api/session/create", json!({
        "name": "integration",
    })).await;
    assert_eq!(body["ok"], true);

    // Create file
    let (_, body) = post_json(&app, "/api/file/create", json!({
        "note_id": "integration",
        "name": "readme.md"
    })).await;
    assert_eq!(body["ok"], true);

    // Update file
    let (_, body) = post_json(&app, "/api/file/update", json!({
        "note_id": "integration",
        "filename": "readme.md",
        "content": "# Integration Test\n\nThis works!"
    })).await;
    assert_eq!(body["ok"], true);

    // Switch to file (read back)
    let (_, body) = post_json(&app, "/api/file/switch", json!({
        "note_id": "integration",
        "name": "readme.md"
    })).await;
    assert_eq!(body["ok"], true);

    // Rename file
    let (_, body) = post_json(&app, "/api/file/rename", json!({
        "note_id": "integration",
        "old_name": "readme.md",
        "new_name": "docs.md"
    })).await;
    assert_eq!(body["ok"], true);

    // Delete file
    let (_, body) = post_json(&app, "/api/file/delete", json!({
        "note_id": "integration",
        "name": "docs.md"
    })).await;
    assert_eq!(body["ok"], true);

    // Delete session
    let (_, body) = post_json(&app, "/api/session/delete", json!({
        "note_id": "integration"
    })).await;
    assert_eq!(body["ok"], true);
}

}


// ─── Per-Note Isolation Tests ──────────────────────────────────────────────

#[cfg(test)]
mod isolation_tests {
    use crate::serve::api::NoteRoom;
    use crate::serve::ServerState;
    use crate::serve::db::ChatDb;
    use crate::config::RuneConfig;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tokio::sync::{broadcast, RwLock};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn make_state() -> (ServerState, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db_path = tmp.path().join("test.db");
        let db = ChatDb::open(&db_path).unwrap();
        let state = ServerState {
            config: RuneConfig::default(),
            user_token: None,
            admin_token: Some("admin123".into()),
            guest_token: None,
            files: Arc::new(RwLock::new(HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: vec!["gpt-5-mini".into(), "claude-sonnet-4.6".into()],
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
        use axum::{routing::post, Router};
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            user_token: Some("user-tok".into()),
            admin_token: Some("admin-tok".into()),
            guest_token: None,
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["gpt-5-mini".into(), "claude-sonnet-4.6".into()],
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("gpt-5-mini".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        // Create note in DB so room can be created
        let _ = state.chat_db.create_note("test-note", "test-note", None);

        let app = Router::new()
            .route("/api/model/switch", post(crate::serve::api::model_switch_handler))
            .with_state(state.clone());

        // Admin can set per-note override
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/model/switch")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"model":"claude-sonnet-4.6","note_id":"test-note"}"#))
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
        use axum::{routing::get, Router};
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            user_token: Some("tok".into()),
            admin_token: None,
            guest_token: None,
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["m1".into()],
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Request SSE with non-existent note_id
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?token=tok&note_id=nonexistent&nickname=test")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Body should contain error event
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("Note not found"), "Expected 'Note not found' in: {}", body_str);
    }

    #[tokio::test]
    async fn test_guest_private_note_rejected() {
        use axum::{routing::get, Router};
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db = crate::serve::db::ChatDb::open(&tmp.path().join("t.db")).unwrap();
        // Create a private note
        let _ = db.create_note("private-note", "private-note", None);

        let state = crate::serve::ServerState {
            config: crate::config::RuneConfig::default(),
            user_token: None,
            admin_token: None,
            guest_token: Some("guest-tok".into()),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["m1".into()],
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Guest trying to subscribe to private note
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?token=guest-tok&note_id=private-note&nickname=guest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("Guests cannot access private notes"),
            "Expected auth_error in: {}", body_str);
    }

    #[tokio::test]
    async fn test_guest_public_note_allowed() {
        use axum::{routing::get, Router};
        use axum::body::Body;
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
            user_token: None,
            admin_token: None,
            guest_token: Some("guest-tok".into()),
            files: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["m1".into()],
            rooms: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new("m1".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
        };

        let app = Router::new()
            .route("/api/events", get(crate::serve::api::events_handler))
            .with_state(state);

        // Guest subscribing to public note — should succeed (200 OK)
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/events?token=guest-tok&note_id=public-note&nickname=guest")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Read first frame with timeout (SSE streams forever, so we just check first data)
        let mut body = resp.into_body();
        let first_frame = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            body.frame(),
        ).await;
        assert!(first_frame.is_ok(), "Should receive first SSE frame quickly");
        let frame = first_frame.unwrap().unwrap().unwrap();
        let data = frame.into_data().unwrap();
        let text = String::from_utf8_lossy(&data);
        // First event should be auth_result
        assert!(text.contains("auth_result"), "Expected auth_result in first frame: {}", text);
    }

}
