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
use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

// ─── Online user counter ───────────────────────────────────────────────────

static ONLINE_COUNT: AtomicU32 = AtomicU32::new(0);

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
    AuthResult { is_admin: bool },
    #[serde(rename = "file_list")]
    FileList { files: Vec<String>, active: String },
    #[serde(rename = "file_content")]
    FileContent { filename: String, content: String },
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
    match &state.token {
        None => true, // no token configured = open
        Some(expected) => token == Some(expected.as_str()),
    }
}

pub fn check_admin(state: &ServerState, admin_token: Option<&str>) -> bool {
    match &state.admin_token {
        None => false,
        Some(at) => !at.is_empty() && admin_token == Some(at.as_str()),
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
        let md_dir = super::note_markdown_dir(&s.id);
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
        entries.push(NoteListEntry {
            id: s.id,
            name: s.name,
            files,
        });
    }
    entries
}

/// Broadcast an SSE message to all connected clients.
pub fn broadcast(state: &ServerState, msg: &SseMsg) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = state.broadcast_tx.send(json);
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
    let token_ok = check_token(&state, token) || is_admin;
    if !token_ok {
        // Return 401 as a one-shot SSE error then close
        let err_stream = futures::stream::once(async {
            Ok::<_, Infallible>(Event::default()
                .event("error")
                .data(r#"{"type":"error","message":"Authentication failed"}"#.to_string()))
        });
        return Sse::new(err_stream).keep_alive(KeepAlive::default()).into_response();
    }

    // Increment online count
    let count = ONLINE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

    // Create a receiver for broadcast messages
    let mut rx = state.broadcast_tx.subscribe();

    // Build initial messages to send
    let mut init_msgs = Vec::new();

    // Auth result
    init_msgs.push(SseMsg::AuthResult { is_admin });

    // Model list
    let active_model = state.active_model.read().await.clone();
    init_msgs.push(SseMsg::ModelList {
        models: state.models.clone(),
        active: active_model,
    });

    // Session list
    let notes = build_note_list(&state).await;
    init_msgs.push(SseMsg::NoteList { notes, active: String::new() });

    // Users update
    init_msgs.push(SseMsg::UsersUpdate { count });

    // System join message
    let join_msg = SseMsg::System { content: format!("{} joined", nickname) };
    broadcast(&state, &join_msg);

    // Users update broadcast
    let users_msg = SseMsg::UsersUpdate { count };
    broadcast(&state, &users_msg);

    let nickname_clone = nickname.clone();
    let state_clone = state.clone();

    let stream = async_stream::stream! {
        // Send initial messages
        for msg in init_msgs {
            if let Ok(json) = serde_json::to_string(&msg) {
                let event_type = extract_event_type(&json);
                yield Ok::<_, Infallible>(Event::default().event(event_type).data(json));
            }
        }

        // Stream broadcast messages
        loop {
            match rx.recv().await {
                Ok(json) => {
                    let event_type = extract_event_type(&json);
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
        broadcast(&state_clone, &leave_msg);
        let users_msg = SseMsg::UsersUpdate { count };
        broadcast(&state_clone, &users_msg);
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
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

    // Broadcast user message
    let nickname = req.nickname.clone().unwrap_or_else(|| "user".to_string());
    let user_msg = SseMsg::ChatMessage {
        nickname: nickname.clone(),
        content: req.content.clone(),
    };
    broadcast(&state, &user_msg);

    // Persist user message
    state.chat_db.insert_async(
        req.note_id.clone(),
        "user".to_string(),
        nickname,
        req.content.clone(),
    ).await;

    // Send thinking status
    let thinking = SseMsg::Status { state: "thinking".to_string() };
    broadcast(&state, &thinking);

    // Spawn agent task
    let state_clone = state.clone();
    let note_id = req.note_id.clone();
    let content = req.content.clone();
    let nick = req.nickname.clone().unwrap_or_else(|| "user".to_string());
    tokio::spawn(async move {
        handle_chat_message(content, state_clone, note_id, nick).await;
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

    let md_dir = super::note_markdown_dir(&req.note_id);
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

    // Broadcast file content
    let fc = SseMsg::FileContent { filename: req.name, content: empty };
    broadcast(&state, &fc);

    Json(ApiResponse::success())
}

pub async fn file_delete_handler(
    State(state): State<ServerState>,
    Json(req): Json<FileDeleteReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }

    let md_dir = super::note_markdown_dir(&req.note_id);
    let file_path = md_dir.join(&req.name);
    tokio::fs::remove_file(&file_path).await.ok();

    let del = SseMsg::FileDeleted { filename: req.name };
    broadcast(&state, &del);
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

    let md_dir = super::note_markdown_dir(&req.note_id);
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
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }

    let file_path = super::note_markdown_dir(&req.note_id).join(&req.name);
    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => {
            let fc = SseMsg::FileContent { filename: req.name, content };
            broadcast(&state, &fc);
            Json(ApiResponse::success())
        }
        Err(_) => Json(ApiResponse::err(format!("File not found: {}", req.name))),
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

    let file_path = super::note_markdown_dir(&req.note_id).join(&fname);
    if let Err(e) = tokio::fs::write(&file_path, &req.content).await {
        return Json(ApiResponse::err(format!("Failed to write: {}", e)));
    }

    let fc = SseMsg::FileContent { filename: fname, content: req.content };
    broadcast(&state, &fc);
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
            let md_dir = super::note_markdown_dir(&id);
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
            let old_dir = super::data_dir().join("notes").join(&req.note_id);
            let new_dir = super::data_dir().join("notes").join(&new_id);
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
    let md_dir = super::note_markdown_dir(&req.note_id);
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
    Json(serde_json::json!({
        "ok": true,
        "note_id": req.note_id,
        "history": history,
        "files": files,
        "current_file": first_file,
        "file_content": first_content,
    }))
}


pub async fn model_switch_handler(
    State(state): State<ServerState>,
    Json(req): Json<ModelSwitchReq>,
) -> Json<ApiResponse> {
    if !state.models.contains(&req.model) {
        return Json(ApiResponse::err(format!("Unknown model: {}", req.model)));
    }
    *state.active_model.write().await = req.model.clone();
    let msg = SseMsg::ModelChanged { model: req.model };
    broadcast(&state, &msg);
    Json(ApiResponse::success())
}

pub async fn archive_handler(
    State(state): State<ServerState>,
    Json(req): Json<ArchiveReq>,
) -> Json<ApiResponse> {
    if req.note_id.is_empty() {
        return Json(ApiResponse::err("No note selected"));
    }
    let archive_dir = super::note_markdown_dir(&req.note_id)
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
            let msg = SseMsg::ArchiveDone { filename, count };
            broadcast(&state, &msg);
            // Send empty history
            let hist = SseMsg::History { messages: vec![] };
            broadcast(&state, &hist);
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
    let archive_dir = super::note_markdown_dir(&req.note_id).parent().unwrap().join("archives");
    let results = state.chat_db.search_async(req.note_id, req.query.clone(), archive_dir).await;
    let msg = SseMsg::SearchResults { query: req.query, results };
    broadcast(&state, &msg);
    Json(ApiResponse::success())
}

pub async fn approval_handler(
    State(state): State<ServerState>,
    Json(req): Json<ApprovalReq>,
) -> Json<ApiResponse> {
    // Send approval response through the broadcast channel
    // The agent task listens for this via its approval callback
    let msg = if req.approved {
        format!("__approval_granted__{}", req.id)
    } else {
        format!("__approval_denied__{}", req.id)
    };
    let _ = state.broadcast_tx.send(msg);
    Json(ApiResponse::success())
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

async fn broadcast_file_list(state: &ServerState, note_id: &str) {
    let md_dir = super::note_markdown_dir(note_id);
    let mut files = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") { files.push(name); }
        }
    }
    files.sort();
    let active = files.first().cloned().unwrap_or_default();
    let msg = SseMsg::FileList { files, active };
    broadcast(state, &msg);
}

async fn broadcast_note_list(state: &ServerState) {
    let notes = build_note_list(state).await;
    let msg = SseMsg::NoteList { notes, active: String::new() };
    broadcast(state, &msg);
}

// ─── Agent chat handler ────────────────────────────────────────────────────

async fn handle_chat_message(
    user_msg: String,
    state: ServerState,
    note_id: String,
    nickname: String,
) {
    let config = state.config.clone();
    let active_model = state.active_model.read().await.clone();

    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let err = SseMsg::Error { message: format!("Provider error: {}", e) };
            broadcast(&state, &err);
            let idle = SseMsg::Status { state: "idle".to_string() };
            broadcast(&state, &idle);
            return;
        }
    };

    // Build embedding
    let embedding = build_embedding(&config).await;

    // Token streaming callback
    let state_for_token = state.clone();
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        let msg = SseMsg::ChatToken { content: token.to_string() };
        broadcast(&state_for_token, &msg);
    });

    // Approval callback
    let state_for_approval = state.clone();
    let approval_callback: Arc<dyn Fn(String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync> =
        Arc::new(move |id: String, detail: String| {
            let state = state_for_approval.clone();
            Box::pin(async move {
                let msg = SseMsg::ApprovalRequest { id: id.clone(), detail };
                broadcast(&state, &msg);
                // Wait for approval response via broadcast
                let mut rx = state.broadcast_tx.subscribe();
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
    agent.markdown_dir = Some(super::note_markdown_dir(&note_id));
    agent.chat_db = Some(state.chat_db.clone());
    agent.chat_note_id = Some(note_id.clone());
    agent.chat_archive_dir = Some(super::note_markdown_dir(&note_id)
        .parent().unwrap().join("archives"));

    // Set system prompt
    let system_prompt = build_system_prompt(&config).await;
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
    broadcast(&state, &done);

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
            broadcast(&state, &err);
        }
        StopReason::MaxSteps => {
            let err = SseMsg::Error { message: "Agent reached max steps".to_string() };
            broadcast(&state, &err);
        }
        StopReason::TokenBudgetExhausted => {
            let err = SseMsg::Error { message: "Token budget exhausted".to_string() };
            broadcast(&state, &err);
        }
        _ => {}
    }

    // Broadcast updated file list (new files may have been created)
    broadcast_file_list(&state, &note_id).await;

    let idle = SseMsg::Status { state: "idle".to_string() };
    broadcast(&state, &idle);
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
        let state = mock_state(None, None);
        assert!(check_token(&state, None));
        assert!(check_token(&state, Some("anything")));
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
            files: vec!["a.md".into(), "b.md".into()],
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
        let (broadcast_tx, _) = broadcast::channel(16);
        let (admin_broadcast_tx, _) = broadcast::channel(16);
        ServerState {
            config: crate::config::RuneConfig::default(),
            token,
            admin_token,
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: vec!["test-model".into()],
            active_model: Arc::new(tokio::sync::RwLock::new("test-model".into())),
            broadcast_tx,
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
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
    let (broadcast_tx, _) = broadcast::channel(256);
    let (admin_broadcast_tx, _) = broadcast::channel(256);
    let db_path = tmp.path().join("test.db");
    let db = ChatDb::open(&db_path).unwrap();
    ServerState {
        config: RuneConfig::default(),
        token: None,
        admin_token: Some("admin123".into()),
        files: Arc::new(RwLock::new(std::collections::HashMap::new())),
        active_file: Arc::new(RwLock::new(String::new())),
        models: vec!["gpt-5-mini".into(), "claude-sonnet-4.6".into()],
        active_model: Arc::new(RwLock::new("gpt-5-mini".into())),
        broadcast_tx,
        admin_broadcast_tx,
        chat_db: db,
    }
}

fn test_state_with_token(tmp: &TempDir) -> ServerState {
    let mut state = test_state(tmp);
    state.token = Some("secret".into());
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
    std::env::set_var("HOME", tmp.path());
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
    std::env::set_var("HOME", tmp.path());
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
    std::env::set_var("HOME", tmp.path());
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
