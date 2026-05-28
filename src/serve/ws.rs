//! WebSocket handler for chat + spec.md sync.

use crate::agent::{Agent, StopReason};
use crate::config::RuneConfig;
use crate::embedding::EmbeddingEngine;
use crate::provider::{CopilotProvider, GeminiProvider, OpenAiProvider, ProviderRegistry};
use crate::serve::ServerState;
use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, warn};

/// Incoming WebSocket message types from the client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    /// Client sets their nickname on connect.
    #[serde(rename = "set_nickname")]
    SetNickname { name: String, token: Option<String> },

    /// User sends a chat message.
    #[serde(rename = "chat_send")]
    ChatSend { content: String },

    /// User edits the active (or named) markdown file.
    #[serde(rename = "spec_update")]
    SpecUpdate { content: String, filename: Option<String> },

    /// User responds to an approval request.
    #[serde(rename = "approval_response")]
    ApprovalResponse { id: String, approved: bool },

    /// Archive current chat history and clear the active window.
    #[serde(rename = "archive_chat")]
    ArchiveChat,

    /// Search across current chat + all archives.
    #[serde(rename = "search_chat")]
    SearchChat { query: String },

    /// Create a new markdown file.
    #[serde(rename = "file_create")]
    FileCreate { name: String },

    /// Delete a markdown file.
    #[serde(rename = "file_delete")]
    FileDelete { name: String },

    /// Switch active file.
    #[serde(rename = "file_switch")]
    FileSwitch { name: String },

    /// Rename a markdown file.
    #[serde(rename = "file_rename")]
    FileRename { old_name: String, new_name: String },

    /// Switch the active AI model (admin only).
    #[serde(rename = "switch_model")]
    SwitchModel { model: String },

    /// Switch the active session (any user).
    #[serde(rename = "session_switch")]
    SessionSwitch { session_id: String },

    /// Create a new session (admin only).
    #[serde(rename = "session_create")]
    SessionCreate { name: String, workspace: Option<String> },

    /// Rename a session (admin only).
    #[serde(rename = "session_rename")]
    SessionRename { session_id: String, name: String },

    /// Delete a session (admin only).
    #[serde(rename = "session_delete")]
    SessionDelete { session_id: String },

    /// Update session workspace (admin only).
    #[serde(rename = "session_set_workspace")]
    SessionSetWorkspace { session_id: String, workspace: String },

    /// Browse directories on the server (admin only).
    #[serde(rename = "dir_browse")]
    DirBrowse { path: String },
}

/// Outgoing WebSocket message types to the client.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
enum ServerMsg {
    /// Streaming chat token from the AI.
    #[serde(rename = "chat_token")]
    ChatToken { content: String },

    /// Chat message complete.
    #[serde(rename = "chat_done")]
    ChatDone {},

    /// Full spec content (sent on connect or after AI edit).
    #[serde(rename = "spec_full")]
    SpecFull { content: String },

    /// Agent status change.
    #[serde(rename = "status")]
    Status { state: String },

    /// Approval request from tool execution.
    #[serde(rename = "approval_request")]
    ApprovalRequest { id: String, detail: String },

    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },

    /// User chat message (broadcast to all).
    #[serde(rename = "chat_message")]
    ChatMessage { nickname: String, content: String },

    /// System notification (join/leave).
    #[serde(rename = "system")]
    System { content: String },

    /// Online users count update.
    #[serde(rename = "users_update")]
    UsersUpdate { count: u32 },

    /// Chat history replay on connect.
    #[serde(rename = "history")]
    History { messages: Vec<crate::serve::db::ChatRecord> },

    /// Auth result: tells the client their role after set_nickname.
    #[serde(rename = "auth_result")]
    AuthResult { is_admin: bool },

    /// List of all markdown files + active filename.
    #[serde(rename = "file_list")]
    FileList { files: Vec<String>, active: String },

    /// Full content of a specific file.
    #[serde(rename = "file_content")]
    FileContent { filename: String, content: String },

    /// A file was deleted.
    #[serde(rename = "file_deleted")]
    FileDeleted { filename: String },

    /// Archive completed.
    #[serde(rename = "archive_done")]
    ArchiveDone { filename: String, count: usize },

    /// Model + token usage for the just-completed assistant turn.
    #[serde(rename = "chat_meta")]
    ChatMeta { model: String, tokens_in: u32, tokens_out: u32, context_tokens: u32, context_window: u32 },

    /// Search results.
    #[serde(rename = "search_results")]
    SearchResults { query: String, results: Vec<super::db::ChatRecord> },

    /// Available model list + currently active model (sent on connect).
    #[serde(rename = "model_list")]
    ModelList { models: Vec<String>, active: String },

    /// Broadcast when active model changes.
    #[serde(rename = "model_changed")]
    ModelChanged { model: String },

    /// Session list (sent on connect and after changes).
    #[serde(rename = "session_list")]
    SessionList { sessions: Vec<SessionListEntry>, active: String },

    /// Directory browse result (admin only).
    #[serde(rename = "dir_browse_result")]
    DirBrowseResult { path: String, parent: Option<String>, entries: Vec<DirEntry> },

    /// Session switched confirmation (includes history + files).
    #[serde(rename = "session_switched")]
    SessionSwitched { session_id: String },
}

/// Session list entry for the UI.
#[derive(Debug, Serialize, Clone)]
pub struct SessionListEntry {
    pub id: String,
    pub name: String,
    pub workspace: String,
    pub files: Vec<String>,
}

/// Directory entry for folder browser.
#[derive(Debug, Serialize, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Generate a simple timestamp-based ID (seconds since epoch).
fn chrono_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Build the session list with their markdown files and broadcast to all clients.
async fn broadcast_session_list(state: &ServerState) {
    let sessions = state.chat_db.list_sessions().unwrap_or_default();
    let mut entries = Vec::new();
    for s in sessions {
        let md_dir = super::session_markdown_dir(&s.id);
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
        entries.push(SessionListEntry {
            id: s.id,
            name: s.name,
            workspace: s.workspace,
            files,
        });
    }
    let msg = ServerMsg::SessionList { sessions: entries, active: String::new() };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = state.broadcast_tx.send(json);
    }
}

/// Build session list for a single client (returns serialized JSON).
async fn build_session_list_msg(state: &ServerState) -> Option<String> {
    let sessions = state.chat_db.list_sessions().unwrap_or_default();
    let mut entries = Vec::new();
    for s in sessions {
        let md_dir = super::session_markdown_dir(&s.id);
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
        entries.push(SessionListEntry {
            id: s.id,
            name: s.name,
            workspace: s.workspace,
            files,
        });
    }
    let msg = ServerMsg::SessionList { sessions: entries, active: String::new() };
    serde_json::to_string(&msg).ok()
}

/// Handle a single WebSocket connection.
pub async fn handle_connection(mut socket: WebSocket, state: ServerState) {
    // Step 1: Wait for set_nickname (first message, timeout 5s).
    // Returns (nickname, is_admin). Token verification happens here.
    let (nickname, is_admin) = {
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.recv(),
        )
        .await;

        match first {
            Ok(Some(Ok(Message::Text(text)))) => {
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::SetNickname { name, token: provided_token }) => {
                        // Check if provided token matches admin_token
                        let is_admin = state.admin_token.as_deref()
                            .map(|at| !at.is_empty() && provided_token.as_deref() == Some(at))
                            .unwrap_or(false);

                        // Admin token also satisfies regular token requirement
                        if let Some(ref expected) = state.token {
                            let ok = provided_token.as_deref() == Some(expected.as_str()) || is_admin;
                            if !ok {
                                warn!("WebSocket auth failed: bad token");
                                let err = ServerMsg::Error { message: "Invalid token".to_string() };
                                if let Ok(json) = serde_json::to_string(&err) {
                                    let _ = socket.send(Message::Text(json.into())).await;
                                }
                                return;
                            }
                        }
                        let nick = if name.trim().is_empty() {
                            format!("guest-{}", uuid_short())
                        } else {
                            name.trim().chars().take(20).collect::<String>()
                        };
                        (nick, is_admin)
                    }
                    _ => {
                        if state.token.is_some() {
                            warn!("WebSocket auth failed: no valid set_nickname");
                            return;
                        }
                        (format!("guest-{}", uuid_short()), false)
                    }
                }
            }
            _ => {
                if state.token.is_some() {
                    warn!("WebSocket auth failed: timeout");
                    return;
                }
                (format!("guest-{}", uuid_short()), false)
            }
        }
    };

    info!("WebSocket client connected as '{}'", nickname);

    // Now split the socket after auth passed
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send file list + active file content on connect
    {
        let files = state.files.read().await;
        let active = state.active_file.read().await.clone();
        let mut file_names: Vec<String> = files.keys().cloned().collect();
        file_names.sort();
        let list_msg = ServerMsg::FileList { files: file_names, active: active.clone() };
        if let Ok(json) = serde_json::to_string(&list_msg) {
            let _ = ws_tx.send(Message::Text(json.into())).await;
        }
        let content = files.get(&active).cloned().unwrap_or_default();
        let content_msg = ServerMsg::FileContent { filename: active, content };
        if let Ok(json) = serde_json::to_string(&content_msg) {
            let _ = ws_tx.send(Message::Text(json.into())).await;
        }
    }

    // No history loaded on connect — client will switch to a session after receiving session_list

    // Tell client their role
    let auth_msg = ServerMsg::AuthResult { is_admin };
    if let Ok(json) = serde_json::to_string(&auth_msg) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Send model list
    {
        let active_model = state.active_model.read().await.clone();
        let ml_msg = ServerMsg::ModelList {
            models: state.models.clone(),
            active: active_model,
        };
        if let Ok(json) = serde_json::to_string(&ml_msg) {
            let _ = ws_tx.send(Message::Text(json.into())).await;
        }
    }

    // Send session list
    if let Some(json) = build_session_list_msg(&state).await {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Send ready status
    let status_msg = ServerMsg::Status { state: "idle".to_string() };
    if let Ok(json) = serde_json::to_string(&status_msg) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Pending approval responses (id -> oneshot sender)
    let pending_approvals: PendingApprovals =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

    // Subscribe to broadcast channels
    let mut bcast_rx = state.broadcast_tx.subscribe();
    // Admin clients also receive admin-only approval_request messages
    let mut admin_bcast_rx = state.admin_broadcast_tx.subscribe();

    // Channel for sending messages back to websocket from agent task
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    // Broadcast join message
    let join_msg = ServerMsg::System { content: format!("🟢 {} joined", nickname) };
    if let Ok(json) = serde_json::to_string(&join_msg) {
        let _ = state.broadcast_tx.send(json);
    }

    // Spawn task to forward broadcast messages to this client's ws_tx
    let (ws_forward_tx, mut ws_forward_rx) = mpsc::unbounded_channel::<String>();

    // Merge: mpsc from agent AND broadcast into ws_tx
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                // From agent (mpsc)
                msg = rx.recv() => {
                    match msg {
                        Some(m) => {
                            if let Ok(json) = serde_json::to_string(&m) {
                                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
                // From broadcast
                json = ws_forward_rx.recv() => {
                    match json {
                        Some(j) => {
                            if ws_tx.send(Message::Text(j.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    // Spawn task to forward general broadcast to per-client mpsc
    let ws_forward_tx_clone = ws_forward_tx.clone();
    tokio::spawn(async move {
        loop {
            match bcast_rx.recv().await {
                Ok(json) => {
                    if ws_forward_tx_clone.send(json).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Broadcast lagged by {} messages", n);
                }
                Err(_) => break,
            }
        }
    });

    // Admin clients also receive admin-only broadcast (approval requests)
    if is_admin {
        let ws_forward_tx_admin = ws_forward_tx.clone();
        tokio::spawn(async move {
            loop {
                match admin_bcast_rx.recv().await {
                    Ok(json) => {
                        if ws_forward_tx_admin.send(json).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Admin broadcast lagged by {} messages", n);
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Process incoming messages
    let files_ref   = state.files.clone();
    let active_ref  = state.active_file.clone();
    let config = state.config.clone();
    let broadcast_tx = state.broadcast_tx.clone();
    let nickname_clone = nickname.clone();
    let mut current_session = String::new();

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let text_str: &str = &text;
                match serde_json::from_str::<ClientMsg>(text_str) {
                    Ok(ClientMsg::ChatSend { content }) => {
                        let preview: String = content.chars().take(50).collect();
                        info!("Chat message from '{}': {}", nickname_clone, preview);

                        // Broadcast the user message to all clients
                        let chat_msg = ServerMsg::ChatMessage {
                            nickname: nickname_clone.clone(),
                            content: content.clone(),
                        };
                        if let Ok(json) = serde_json::to_string(&chat_msg) {
                            let _ = broadcast_tx.send(json);
                        }
                        // Persist user message to SQLite
                        state.chat_db.insert_async(
                            current_session.clone(),
                            "user".to_string(),
                            nickname_clone.clone(),
                            content.clone(),
                        ).await;

                        let tx_clone = tx.clone();
                        let bcast_clone = broadcast_tx.clone();
                        let config_clone = config.clone();
                        let files_clone = state.files.clone();
                        let active_clone = state.active_file.clone();

                        // Send thinking status (broadcast)
                        let thinking = ServerMsg::Status { state: "thinking".to_string() };
                        if let Ok(json) = serde_json::to_string(&thinking) {
                            let _ = bcast_clone.send(json);
                        }

                        let pending_clone = pending_approvals.clone();
                        let db_clone = state.chat_db.clone();
                        let admin_bcast_clone = state.admin_broadcast_tx.clone();
                        let active_model_clone = state.active_model.read().await.clone();
                        let session_clone = current_session.clone();
                        tokio::spawn(async move {
                            let result = tokio::task::spawn(async move {
                                handle_chat_message(content, config_clone, active_model_clone, files_clone, active_clone, tx_clone, bcast_clone, admin_bcast_clone, pending_clone, db_clone, session_clone).await;
                            }).await;
                            if let Err(e) = result {
                                eprintln!("Agent task panicked: {:?}", e);
                            }
                        });
                    }
                    Ok(ClientMsg::SpecUpdate { content, filename }) => {
                        let fname = {
                            if let Some(f) = filename {
                                f
                            } else {
                                active_ref.read().await.clone()
                            }
                        };
                        if !is_valid_filename(&fname) {
                            warn!("Invalid filename in spec_update: {}", fname);
                        } else {
                            debug!("Spec update for '{}' from '{}'", fname, nickname_clone);
                            {
                                let mut files = files_ref.write().await;
                                files.insert(fname.clone(), content.clone());
                            }
                            // Persist to disk
                            let file_path = super::session_markdown_dir("main").join(&fname);
                            if let Err(e) = tokio::fs::write(&file_path, &content).await {
                                warn!("Failed to persist {}: {}", fname, e);
                            }
                            // Broadcast updated file content to all clients
                            let msg = ServerMsg::FileContent { filename: fname, content };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let _ = broadcast_tx.send(json);
                            }
                        }
                    }
                    Ok(ClientMsg::FileCreate { name }) => {
                        if !is_valid_filename(&name) {
                            let _ = tx.send(ServerMsg::Error {
                                message: format!("Invalid filename: {}", name),
                            });
                        } else {
                            let mut files = files_ref.write().await;
                            if files.contains_key(&name) {
                                let _ = tx.send(ServerMsg::Error {
                                    message: format!("File already exists: {}", name),
                                });
                            } else {
                                let empty = format!("# {}

", name.trim_end_matches(".md"));
                                files.insert(name.clone(), empty.clone());
                                drop(files);
                                let file_path = super::session_markdown_dir("main").join(&name);
                                tokio::fs::write(&file_path, &empty).await.ok();
                                // Switch active to new file
                                *active_ref.write().await = name.clone();
                                let files = files_ref.read().await;
                                let mut file_names: Vec<String> = files.keys().cloned().collect();
                                file_names.sort();
                                let list = ServerMsg::FileList { files: file_names, active: name.clone() };
                                if let Ok(json) = serde_json::to_string(&list) {
                                    let _ = broadcast_tx.send(json);
                                }
                                let fc = ServerMsg::FileContent { filename: name, content: empty };
                                if let Ok(json) = serde_json::to_string(&fc) {
                                    let _ = broadcast_tx.send(json);
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::FileDelete { name }) => {
                        if name == "spec.md" && files_ref.read().await.len() == 1 {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Cannot delete the last file".to_string(),
                            });
                        } else {
                            let new_active = {
                                let mut files = files_ref.write().await;
                                files.remove(&name);
                                let cur_active = active_ref.read().await.clone();
                                let new_active = if cur_active == name {
                                    files.keys().next().cloned().unwrap_or_else(|| "spec.md".to_string())
                                } else {
                                    cur_active
                                };
                                new_active
                            };
                            *active_ref.write().await = new_active.clone();
                            let file_path = super::session_markdown_dir("main").join(&name);
                            tokio::fs::remove_file(&file_path).await.ok();
                            let del = ServerMsg::FileDeleted { filename: name };
                            if let Ok(json) = serde_json::to_string(&del) {
                                let _ = broadcast_tx.send(json);
                            }
                            let files = files_ref.read().await;
                            let mut file_names: Vec<String> = files.keys().cloned().collect();
                            file_names.sort();
                            let list = ServerMsg::FileList { files: file_names, active: new_active.clone() };
                            if let Ok(json) = serde_json::to_string(&list) {
                                let _ = broadcast_tx.send(json);
                            }
                            let content = files.get(&new_active).cloned().unwrap_or_default();
                            let fc = ServerMsg::FileContent { filename: new_active, content };
                            if let Ok(json) = serde_json::to_string(&fc) {
                                let _ = broadcast_tx.send(json);
                            }
                        }
                    }
                    Ok(ClientMsg::FileSwitch { name }) => {
                        let content = {
                            let files = files_ref.read().await;
                            files.get(&name).cloned()
                        };
                        if let Some(content) = content {
                            *active_ref.write().await = name.clone();
                            let files = files_ref.read().await;
                            let mut file_names: Vec<String> = files.keys().cloned().collect();
                            file_names.sort();
                            let list = ServerMsg::FileList { files: file_names, active: name.clone() };
                            if let Ok(json) = serde_json::to_string(&list) {
                                let _ = broadcast_tx.send(json);
                            }
                            let fc = ServerMsg::FileContent { filename: name, content };
                            if let Ok(json) = serde_json::to_string(&fc) {
                                let _ = broadcast_tx.send(json);
                            }
                        } else {
                            let _ = tx.send(ServerMsg::Error {
                                message: format!("File not found: {}", name),
                            });
                        }
                    }
                    Ok(ClientMsg::FileRename { old_name, new_name }) => {
                        if !is_valid_filename(&new_name) {
                            let _ = tx.send(ServerMsg::Error {
                                message: format!("Invalid filename: {}", new_name),
                            });
                        } else if files_ref.read().await.contains_key(&new_name) {
                            let _ = tx.send(ServerMsg::Error {
                                message: format!("File already exists: {}", new_name),
                            });
                        } else {
                            let content = {
                                let mut files = files_ref.write().await;
                                let c = files.remove(&old_name);
                                if let Some(ref text) = c {
                                    files.insert(new_name.clone(), text.clone());
                                }
                                c
                            };
                            if content.is_some() {
                                let old_path = super::session_markdown_dir("main").join(&old_name);
                                let new_path = super::session_markdown_dir("main").join(&new_name);
                                tokio::fs::rename(&old_path, &new_path).await.ok();
                                let cur = active_ref.read().await.clone();
                                if cur == old_name {
                                    *active_ref.write().await = new_name.clone();
                                }
                                let files = files_ref.read().await;
                                let active = active_ref.read().await.clone();
                                let mut file_names: Vec<String> = files.keys().cloned().collect();
                                file_names.sort();
                                let list = ServerMsg::FileList { files: file_names, active };
                                if let Ok(json) = serde_json::to_string(&list) {
                                    let _ = broadcast_tx.send(json);
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::ArchiveChat) => {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let filename = format!("{}.jsonl", ts);
                        let archive_dir = super::session_markdown_dir("main")
                            .parent().unwrap().join("archives");
                        let archive_path = archive_dir.join(&filename);
                        let db = state.chat_db.clone();
                        match db.archive_async(current_session.clone(), archive_path).await {
                            Ok(count) => {
                                let msg = ServerMsg::ArchiveDone { filename: filename.clone(), count };
                                if let Ok(json) = serde_json::to_string(&msg) {
                                    let _ = broadcast_tx.send(json);
                                }
                                info!("Chat archived to {} ({} messages)", filename, count);
                            }
                            Err(e) => {
                                let _ = tx.send(ServerMsg::Error {
                                    message: format!("Archive failed: {}", e),
                                });
                            }
                        }
                    }
                    Ok(ClientMsg::SearchChat { query }) => {
                        let archive_dir = super::session_markdown_dir(&current_session)
                            .parent().unwrap().join("archives");
                        let db = state.chat_db.clone();
                        let results = db.search_async(
                            current_session.clone(),
                            query.clone(),
                            archive_dir,
                        ).await;
                        let msg = ServerMsg::SearchResults { query, results };
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = tx.send(msg);
                            drop(json);
                        }
                    }
                    Ok(ClientMsg::SwitchModel { model }) => {
                        // Only admin can switch model; model must be in allowed list
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can switch model".to_string(),
                            });
                        } else if !state.models.contains(&model) {
                            let _ = tx.send(ServerMsg::Error {
                                message: format!("Unknown model: {}", model),
                            });
                        } else {
                            *state.active_model.write().await = model.clone();
                            info!("Model switched to '{}' by admin '{}'", model, nickname_clone);
                            let changed = ServerMsg::ModelChanged { model };
                            if let Ok(json) = serde_json::to_string(&changed) {
                                let _ = state.broadcast_tx.send(json);
                            }
                        }
                    }
                    Ok(ClientMsg::ApprovalResponse { id, approved }) => {
                        if is_admin {
                            info!("Approval response from admin '{}': {} = {}", nickname_clone, id, approved);
                            if let Some(tx) = pending_approvals.lock().await.remove(&id) {
                                let _ = tx.send(approved);
                            }
                        } else {
                            warn!("Non-admin '{}' tried to approve request {}", nickname_clone, id);
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can approve requests".to_string(),
                            });
                        }
                    }
                    Ok(ClientMsg::SessionSwitch { session_id }) => {
                        // Any user can switch their view to another session
                        current_session = session_id.clone();
                        // Reload chat history and file list for that session
                        let history = state.chat_db.load_recent_async(session_id.clone(), 100).await;
                        // Send session_switched first, then history
                        let switched_msg = ServerMsg::SessionSwitched { session_id: session_id.clone() };
                        if let Ok(json) = serde_json::to_string(&switched_msg) {
                            let _ = ws_forward_tx.send(json);
                        }
                        let hist_msg = ServerMsg::History { messages: history };
                        if let Ok(json) = serde_json::to_string(&hist_msg) {
                            let _ = ws_forward_tx.send(json);
                        }
                        // Send file list for this session
                        let md_dir = super::session_markdown_dir(&session_id);
                        let _ = tokio::fs::create_dir_all(&md_dir).await;
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
                        let active = files.first().cloned().unwrap_or_else(|| "spec.md".to_string());
                        let fl_msg = ServerMsg::FileList { files, active: active.clone() };
                        if let Ok(json) = serde_json::to_string(&fl_msg) {
                            let _ = ws_forward_tx.send(json);
                        }
                        // Send content of active file
                        let file_path = md_dir.join(&active);
                        if let Ok(content_str) = tokio::fs::read_to_string(&file_path).await {
                            let fc_msg = ServerMsg::FileContent { filename: active, content: content_str };
                            if let Ok(json) = serde_json::to_string(&fc_msg) {
                                let _ = ws_forward_tx.send(json);
                            }
                        }
                    }
                    Ok(ClientMsg::SessionCreate { name, workspace }) => {
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can create sessions".to_string(),
                            });
                        } else {
                            let ws = workspace.unwrap_or_else(|| {
                                std::env::current_dir()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|_| "/tmp".to_string())
                            });
                            // id = name (directory name = session name)
                            let id = name.clone();
                            // Persist DB to file on first session creation
                            if let Err(e) = state.chat_db.ensure_persistent() {
                                warn!("Failed to persist DB: {}", e);
                            }
                            match state.chat_db.create_session(&id, &name, &ws, Some(&nickname_clone)) {
                                Ok(_) => {
                                    info!("Session '{}' created by '{}'", id, nickname_clone);
                                    // Create session directory structure
                                    let md_dir = super::session_markdown_dir(&id);
                                    let _ = tokio::fs::create_dir_all(&md_dir).await;
                                    // Broadcast updated session list
                                    broadcast_session_list(&state).await;
                                }
                                Err(e) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: format!("Failed to create session: {}", e),
                                    });
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::SessionRename { session_id, name }) => {
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can rename sessions".to_string(),
                            });
                        } else {
                            match state.chat_db.rename_session(&session_id, &name) {
                                Ok(Some(new_id)) => {
                                    info!("Session '{}' renamed to '{}' by '{}'", session_id, new_id, nickname_clone);
                                    // Rename the session directory
                                    let old_dir = super::data_dir().join("sessions").join(&session_id);
                                    let new_dir = super::data_dir().join("sessions").join(&new_id);
                                    if old_dir.exists() && !new_dir.exists() {
                                        let _ = tokio::fs::rename(&old_dir, &new_dir).await;
                                    }
                                    // Update current_session if client was on this session
                                    if current_session == session_id {
                                        current_session = new_id.clone();
                                    }
                                    broadcast_session_list(&state).await;
                                }
                                Ok(None) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: "Session not found or name conflict".to_string(),
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: format!("Failed to rename session: {}", e),
                                    });
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::SessionDelete { session_id }) => {
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can delete sessions".to_string(),
                            });
                        } else if false {
                            // placeholder for future guards
                        } else {
                            match state.chat_db.delete_session(&session_id) {
                                Ok(true) => {
                                    info!("Session '{}' deleted by '{}'", session_id, nickname_clone);
                                    broadcast_session_list(&state).await;
                                }
                                Ok(false) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: "Session not found".to_string(),
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: format!("Failed to delete session: {}", e),
                                    });
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::SessionSetWorkspace { session_id, workspace }) => {
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can change workspace".to_string(),
                            });
                        } else {
                            // Validate path exists and is a directory
                            match tokio::fs::metadata(&workspace).await {
                                Ok(m) if m.is_dir() => {
                                    match state.chat_db.update_session_workspace(&session_id, &workspace) {
                                        Ok(true) => {
                                            info!("Session '{}' workspace set to '{}' by '{}'", session_id, workspace, nickname_clone);
                                            broadcast_session_list(&state).await;
                                        }
                                        Ok(false) => {
                                            let _ = tx.send(ServerMsg::Error {
                                                message: "Session not found".to_string(),
                                            });
                                        }
                                        Err(e) => {
                                            let _ = tx.send(ServerMsg::Error {
                                                message: format!("Failed to update workspace: {}", e),
                                            });
                                        }
                                    }
                                }
                                Ok(_) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: "Path is not a directory".to_string(),
                                    });
                                }
                                Err(_) => {
                                    let _ = tx.send(ServerMsg::Error {
                                        message: "Path does not exist".to_string(),
                                    });
                                }
                            }
                        }
                    }
                    Ok(ClientMsg::DirBrowse { path }) => {
                        if !is_admin {
                            let _ = tx.send(ServerMsg::Error {
                                message: "Permission denied: only admins can browse directories".to_string(),
                            });
                        } else {
                            let browse_path = std::path::Path::new(&path);
                            let canonical = tokio::fs::canonicalize(browse_path).await
                                .unwrap_or_else(|_| browse_path.to_path_buf());
                            let parent = canonical.parent().map(|p| p.to_string_lossy().to_string());
                            let mut entries = Vec::new();
                            if let Ok(mut rd) = tokio::fs::read_dir(&canonical).await {
                                while let Ok(Some(entry)) = rd.next_entry().await {
                                    let name = entry.file_name().to_string_lossy().to_string();
                                    // Skip hidden dirs that are not useful
                                    if name.starts_with('.') { continue; }
                                    if let Ok(meta) = entry.metadata().await {
                                        if meta.is_dir() {
                                            entries.push(DirEntry { name, is_dir: true });
                                        }
                                    }
                                }
                            }
                            entries.sort_by(|a, b| a.name.cmp(&b.name));
                            let result = ServerMsg::DirBrowseResult {
                                path: canonical.to_string_lossy().to_string(),
                                parent,
                                entries,
                            };
                            if let Ok(json) = serde_json::to_string(&result) {
                                let _ = ws_forward_tx.send(json);
                            }
                        }
                    }
                    Ok(ClientMsg::SetNickname { .. }) => {
                        // Ignore duplicate nickname messages
                    }
                    Err(e) => {
                        warn!("Invalid WebSocket message from '{}': {}", nickname_clone, e);
                        let _ = tx.send(ServerMsg::Error {
                            message: format!("Invalid message format: {}", e),
                        });
                    }
                }
            }
            Message::Close(_) => {
                info!("WebSocket client '{}' disconnected", nickname_clone);
                break;
            }
            _ => {}
        }
    }

    // Broadcast leave message
    let leave_msg = ServerMsg::System { content: format!("🔴 {} left", nickname) };
    if let Ok(json) = serde_json::to_string(&leave_msg) {
        let _ = state.broadcast_tx.send(json);
    }

    send_task.abort();
}

/// Simple short random id for guest nicknames.
fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    format!("{:04x}", t & 0xffff)
}

type PendingApprovals = Arc<tokio::sync::Mutex<std::collections::HashMap<String, oneshot::Sender<bool>>>>;

/// Handle a chat message — create an Agent, run it with streaming, and broadcast tokens.
fn is_valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".md")
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

async fn handle_chat_message(
    user_msg: String,
    config: RuneConfig,
    active_model: String,
    files: Arc<RwLock<std::collections::HashMap<String, String>>>,
    active_file: Arc<RwLock<String>>,
    tx: mpsc::UnboundedSender<ServerMsg>,
    broadcast_tx: tokio::sync::broadcast::Sender<String>,
    admin_broadcast_tx: tokio::sync::broadcast::Sender<String>,
    pending_approvals: PendingApprovals,
    chat_db: crate::serve::db::ChatDb,
    session_id: String,
) {
    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(ServerMsg::Error {
                message: format!("Provider error: {}", e),
            });
            let idle = ServerMsg::Status { state: "idle".to_string() };
            if let Ok(json) = serde_json::to_string(&idle) { let _ = broadcast_tx.send(json); }
            return;
        }
    };

    // Build embedding engine (optional)
    let embedding = build_embedding(&config).await;

    // Create agent with streaming callback — broadcast tokens to all
    let bcast = broadcast_tx.clone();
    let assistant_text = Arc::new(std::sync::Mutex::new(String::new()));
    let assistant_text_cb = assistant_text.clone();
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        if let Ok(mut t) = assistant_text_cb.lock() { t.push_str(token); }
        let msg = ServerMsg::ChatToken { content: token.to_string() };
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = bcast.send(json);
        }
    });

    // Approval callback: send ONLY to admin clients via admin_broadcast_tx
    let bcast_approval = admin_broadcast_tx.clone();
    let pending_approvals_cb = pending_approvals.clone();
    let approval_callback: Arc<dyn Fn(String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync> =
        Arc::new(move |id: String, detail: String| {
            let bcast = bcast_approval.clone();
            let pending = pending_approvals_cb.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel::<bool>();
                pending.lock().await.insert(id.clone(), tx);
                let msg = ServerMsg::ApprovalRequest { id, detail };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = bcast.send(json);
                }
                // Wait up to 60 seconds for user response
                match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                    Ok(Ok(approved)) => approved,
                    _ => false, // timeout or channel closed = deny
                }
            })
        });

    // Override model from active_model (runtime switch support)
    let mut cfg = config.clone();
    cfg.model = active_model.clone();
    let mut agent = Agent::new(cfg, provider, true, embedding);
    agent.token_callback = Some(token_callback);
    agent.approval_callback = Some(approval_callback);
    agent.files = Some(files.clone());
    agent.active_file = Some(active_file.clone());
    agent.chat_db = Some(chat_db.clone());
    agent.chat_session_id = Some(session_id.clone());
    agent.chat_archive_dir = Some(super::session_markdown_dir("main")
        .parent().unwrap().join("archives"));

    // Set system prompt
    let system_prompt = build_system_prompt(&config).await;
    agent.set_system_prompt(&system_prompt);

    // Load chat history (last 50 turns) into agent context, excluding the
    // current user message (already being passed to agent.run())
    let history = chat_db.load_recent_async(session_id.clone(), 51).await;
    // Drop the last record if it's the user message we're about to send
    let history_without_current: Vec<_> = history
        .into_iter()
        .filter(|r| !(r.role == "user" && r.content == user_msg))
        .collect();
    // Keep at most last 50 turns (user+assistant pairs = 100 messages)
    let max_history = 100usize;
    let history_slice = if history_without_current.len() > max_history {
        &history_without_current[history_without_current.len() - max_history..]
    } else {
        &history_without_current[..]
    };
    agent.load_history(history_slice);

    // Status: thinking → typing (first token) → idle
    // "thinking" was already broadcast by the caller before spawning this task.
    // Switch to "typing" on first token so the user sees the distinction.
    let first_token_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let first_token_bcast = broadcast_tx.clone();
    let first_token_flag = first_token_sent.clone();
    let prev_callback = agent.token_callback.take();
    let bcast_for_token = first_token_bcast.clone();
    agent.token_callback = Some(Arc::new(move |token: &str| {
        // Switch status to typing on first token
        if !first_token_flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let typing = ServerMsg::Status { state: "typing".to_string() };
            if let Ok(json) = serde_json::to_string(&typing) {
                let _ = bcast_for_token.send(json);
            }
        }
        if let Some(ref cb) = prev_callback { cb(token); }
    }));

    // Run the agent
    let result = agent.run(&user_msg).await;

    match &result {
        StopReason::Error(e) => {
            let msg = ServerMsg::ChatToken { content: format!("\n\n⚠ Error: {}", e) };
            if let Ok(json) = serde_json::to_string(&msg) { let _ = broadcast_tx.send(json); }
        }
        StopReason::MaxSteps => {
            let msg = ServerMsg::ChatToken { content: "\n\n⚠ Stopped: maximum steps reached".to_string() };
            if let Ok(json) = serde_json::to_string(&msg) { let _ = broadcast_tx.send(json); }
        }
        StopReason::TokenBudgetExhausted => {
            let msg = ServerMsg::ChatToken { content: "\n\n⚠ Stopped: token budget exhausted".to_string() };
            if let Ok(json) = serde_json::to_string(&msg) { let _ = broadcast_tx.send(json); }
        }
        _ => {}
    }

    // Persist assistant message to SQLite (with model + token metadata)
    let final_text = assistant_text.lock().map(|t| t.clone()).unwrap_or_default();
    if !final_text.is_empty() {
        let tok_in  = if agent.tokens_in  > 0 { Some(agent.tokens_in  as i32) } else { None };
        let tok_out = if agent.tokens_out > 0 { Some(agent.tokens_out as i32) } else { None };
        chat_db.insert_with_meta_async(
            session_id.clone(),
            "assistant".to_string(),
            "ᚱᚢᚾᛖ".to_string(),
            final_text,
            Some(active_model.clone()),
            tok_in,
            tok_out,
        ).await;
    }

    // Broadcast model + token metadata (before chat_done)
    let ctx_tokens = agent.total_context_tokens() as u32;
    let ctx_window  = agent.config.context_window as u32;
    let meta = ServerMsg::ChatMeta {
        model: active_model.clone(),
        tokens_in: agent.tokens_in,
        tokens_out: agent.tokens_out,
        context_tokens: ctx_tokens,
        context_window: ctx_window,
    };
    if let Ok(json) = serde_json::to_string(&meta) { let _ = broadcast_tx.send(json); }

    // Broadcast chat done
    let done = ServerMsg::ChatDone {};
    if let Ok(json) = serde_json::to_string(&done) { let _ = broadcast_tx.send(json); }

    // Push updated active file to all clients after agent edits
    let active = active_file.read().await.clone();
    let active_content = files.read().await.get(&active).cloned().unwrap_or_default();
    let fc_msg = ServerMsg::FileContent { filename: active.clone(), content: active_content.clone() };
    if let Ok(json) = serde_json::to_string(&fc_msg) { let _ = broadcast_tx.send(json); }
    // Persist to disk
    let file_path = super::session_markdown_dir("main").join(&active);
    if let Err(e) = tokio::fs::write(&file_path, &active_content).await {
        warn!("Failed to persist {} after agent edit: {}", active, e);
    }

    let idle = ServerMsg::Status { state: "idle".to_string() };
    if let Ok(json) = serde_json::to_string(&idle) { let _ = broadcast_tx.send(json); }
}

/// Build a ProviderRegistry from config.
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

    Ok(registry)
}

/// Build embedding engine if configured.
async fn build_embedding(config: &RuneConfig) -> Option<EmbeddingEngine> {
    use crate::embedding::EmbeddingConfig;

    if config.embedding.model.is_some() || config.embedding.enabled {
        let mut emb_cfg = config.embedding.clone();
        if emb_cfg.api_key.is_none() {
            emb_cfg.api_key = config.api_key.clone();
        }

        let is_copilot = config.provider.as_deref().map(|p| p.contains("copilot")).unwrap_or_else(|| {
            config.api_key.as_deref().map(|k| k.starts_with("ghu_") || k.starts_with("ghp_")).unwrap_or(false)
        });

        if is_copilot {
            if emb_cfg.base_url.is_none() {
                emb_cfg.base_url = Some("https://api.githubcopilot.com".to_string());
            }
            let pat = config.api_key.clone().unwrap_or_default();
            Some(EmbeddingEngine::new_copilot(emb_cfg, pat))
        } else {
            Some(EmbeddingEngine::new(emb_cfg))
        }
    } else {
        None
    }
}

/// Build the system prompt for serve mode.
async fn build_system_prompt(config: &RuneConfig) -> String {
    config.system_prompt.as_deref().unwrap_or(
        "You are Rune, a high-performance zero-trust AI agent.\n\
         You are in WebUI serve mode. You have access to shared markdown files and the conversation history.\n\
         MARKDOWN FILE TOOLS (never use read_file/write_file for .md files):\n\
         - list_markdown: list all markdown files and the active one\n\
         - read_markdown(filename?): read a file (default: active file)\n\
         - edit_markdown(filename?, content) or edit_markdown(filename?, search, replace): edit a file\n\
         CHAT HISTORY TOOL:\n\
         - search_chat(query): search the full conversation history (including archives) by keyword\n\
         Prefer search+replace over full replacement for targeted edits.\n\
         Be concise in chat; put detailed content into the markdown files.",
    ).to_string()
}

#[cfg(test)]
mod tests {
    use super::is_valid_filename;

    fn is_admin_for(admin_token: Option<&str>, provided: Option<&str>) -> bool {
        admin_token
            .map(|at| !at.is_empty() && provided == Some(at))
            .unwrap_or(false)
    }

    fn token_ok(expected: &str, provided: Option<&str>, is_admin: bool) -> bool {
        provided == Some(expected) || is_admin
    }

    #[test]
    fn test_admin_token_match() { assert!(is_admin_for(Some("secret"), Some("secret"))); }
    #[test]
    fn test_admin_token_mismatch() { assert!(!is_admin_for(Some("secret"), Some("wrong"))); }
    #[test]
    fn test_admin_token_none_provided() { assert!(!is_admin_for(Some("secret"), None)); }
    #[test]
    fn test_no_admin_token_configured() { assert!(!is_admin_for(None, Some("anything"))); }
    #[test]
    fn test_empty_admin_token_never_matches() {
        assert!(!is_admin_for(Some(""), Some("")));
        assert!(!is_admin_for(Some(""), Some("anything")));
    }
    #[test]
    fn test_admin_token_satisfies_regular_token() {
        assert!(token_ok("regular", Some("admin-secret"), true));
    }
    #[test]
    fn test_regular_token_accepted() { assert!(token_ok("regular", Some("regular"), false)); }
    #[test]
    fn test_wrong_token_rejected() { assert!(!token_ok("regular", Some("wrong"), false)); }
    #[test]
    fn test_no_token_provided_rejected() { assert!(!token_ok("regular", None, false)); }

    #[test]
    fn test_filename_validation() {
        assert!(is_valid_filename("spec.md"));
        assert!(is_valid_filename("my-doc.md"));
        assert!(is_valid_filename("arch_v2.md"));
        assert!(!is_valid_filename(""));
        assert!(!is_valid_filename("file.txt"));
        assert!(!is_valid_filename("../etc/passwd.md"));
        assert!(!is_valid_filename("file name.md"));
        assert!(!is_valid_filename("file;rm.md"));
    }

    #[test]
    fn test_chat_meta_serializes_context_fields() {
        let msg = super::ServerMsg::ChatMeta {
            model: "gpt-4o".to_string(),
            tokens_in: 1000,
            tokens_out: 200,
            context_tokens: 15000,
            context_window: 128000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"chat_meta"#));
        assert!(json.contains(r#""context_tokens":15000"#));
        assert!(json.contains(r#""context_window":128000"#));
        assert!(json.contains(r#""tokens_in":1000"#));
        assert!(json.contains(r#""tokens_out":200"#));
        assert!(json.contains(r#""model":"gpt-4o""#));
    }

    #[test]
    fn test_chat_meta_context_pct_range() {
        // context_tokens should always be <= context_window for sane input
        let ctx_tokens: u32 = 80_000;
        let ctx_window: u32 = 128_000;
        let pct = (ctx_tokens as f64 / ctx_window as f64 * 100.0).round() as u32;
        assert_eq!(pct, 63); // 62.5 rounds to 63

        let ctx_tokens2: u32 = 0;
        let pct2 = (ctx_tokens2 as f64 / ctx_window as f64 * 100.0).round() as u32;
        assert_eq!(pct2, 0);

        let ctx_tokens3: u32 = 128_000;
        let pct3 = (ctx_tokens3 as f64 / ctx_window as f64 * 100.0).round() as u32;
        assert_eq!(pct3, 100);
    }
}
