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

    /// User edits the spec document.
    #[serde(rename = "spec_update")]
    SpecUpdate { content: String },

    /// User responds to an approval request.
    #[serde(rename = "approval_response")]
    ApprovalResponse { id: String, approved: bool },
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

    // Send initial spec content
    let spec = state.spec_content.read().await.clone();
    let init_msg = ServerMsg::SpecFull { content: spec };
    if let Ok(json) = serde_json::to_string(&init_msg) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Send chat history (last 200 messages) to the newly connected client
    let history = state.chat_db.load_recent_async("default".to_string(), 200).await;
    if !history.is_empty() {
        let hist_msg = ServerMsg::History { messages: history };
        if let Ok(json) = serde_json::to_string(&hist_msg) {
            let _ = ws_tx.send(Message::Text(json.into())).await;
        }
    }

    // Tell client their role
    let auth_msg = ServerMsg::AuthResult { is_admin };
    if let Ok(json) = serde_json::to_string(&auth_msg) {
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
    let spec_content = state.spec_content.clone();
    let config = state.config.clone();
    let broadcast_tx = state.broadcast_tx.clone();
    let nickname_clone = nickname.clone();

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
                            "default".to_string(),
                            "user".to_string(),
                            nickname_clone.clone(),
                            content.clone(),
                        ).await;

                        let tx_clone = tx.clone();
                        let bcast_clone = broadcast_tx.clone();
                        let config_clone = config.clone();
                        let spec_clone = spec_content.clone();

                        // Send thinking status (broadcast)
                        let thinking = ServerMsg::Status { state: "thinking".to_string() };
                        if let Ok(json) = serde_json::to_string(&thinking) {
                            let _ = bcast_clone.send(json);
                        }

                        let pending_clone = pending_approvals.clone();
                        let db_clone = state.chat_db.clone();
                        let admin_bcast_clone = state.admin_broadcast_tx.clone();
                        tokio::spawn(async move {
                            let result = tokio::task::spawn(async move {
                                handle_chat_message(content, config_clone, spec_clone, tx_clone, bcast_clone, admin_bcast_clone, pending_clone, db_clone).await;
                            }).await;
                            if let Err(e) = result {
                                eprintln!("Agent task panicked: {:?}", e);
                            }
                        });
                    }
                    Ok(ClientMsg::SpecUpdate { content }) => {
                        debug!("Spec update from client '{}'", nickname_clone);
                        let mut spec = spec_content.write().await;
                        *spec = content.clone();
                        // Persist to disk
                        let spec_path = super::data_dir().join("spec.md");
                        if let Err(e) = tokio::fs::write(&spec_path, &content).await {
                            warn!("Failed to persist spec.md: {}", e);
                        }
                        // Broadcast updated spec to all clients
                        let spec_msg = ServerMsg::SpecFull { content };
                        if let Ok(json) = serde_json::to_string(&spec_msg) {
                            let _ = broadcast_tx.send(json);
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
async fn handle_chat_message(
    user_msg: String,
    config: RuneConfig,
    spec_content: Arc<RwLock<String>>,
    tx: mpsc::UnboundedSender<ServerMsg>,
    broadcast_tx: tokio::sync::broadcast::Sender<String>,
    admin_broadcast_tx: tokio::sync::broadcast::Sender<String>,
    pending_approvals: PendingApprovals,
    chat_db: crate::serve::db::ChatDb,
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

    let mut agent = Agent::new(config.clone(), provider, true, embedding);
    agent.token_callback = Some(token_callback);
    agent.approval_callback = Some(approval_callback);
    agent.spec_content = Some(spec_content.clone());

    // Set system prompt
    let system_prompt = build_system_prompt(&config).await;
    agent.set_system_prompt(&system_prompt);

    // Send typing status (broadcast)
    let typing = ServerMsg::Status { state: "typing".to_string() };
    if let Ok(json) = serde_json::to_string(&typing) { let _ = broadcast_tx.send(json); }

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

    // Persist assistant message to SQLite
    let final_text = assistant_text.lock().map(|t| t.clone()).unwrap_or_default();
    if !final_text.is_empty() {
        chat_db.insert_async(
            "default".to_string(),
            "assistant".to_string(),
            "ᚱᚢᚾᛖ".to_string(),
            final_text,
        ).await;
    }

    // Broadcast chat done
    let done = ServerMsg::ChatDone {};
    if let Ok(json) = serde_json::to_string(&done) { let _ = broadcast_tx.send(json); }

    // Push updated spec to all
    let new_spec = spec_content.read().await.clone();
    let spec_msg = ServerMsg::SpecFull { content: new_spec.clone() };
    if let Ok(json) = serde_json::to_string(&spec_msg) { let _ = broadcast_tx.send(json); }

    // Persist spec
    let spec_path = super::data_dir().join("spec.md");
    if let Err(e) = tokio::fs::write(&spec_path, &new_spec).await {
        warn!("Failed to persist spec.md after agent edit: {}", e);
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
        "You are Rune, a high-performance zero-trust AI agent. \
         You are currently in WebUI serve mode, collaborating with the user on a shared spec document (spec.md). \
         You can read and edit the spec using the read_spec and edit_spec tools. \
         The spec.md is displayed in real-time in the center panel. \
         When editing the spec, prefer targeted search+replace edits over full replacement. \
         Be concise in chat; put detailed content into the spec document.",
    ).to_string()
}

#[cfg(test)]
mod tests {
    /// Test the admin token detection logic (extracted as pure function for testability).
    fn is_admin_for(admin_token: Option<&str>, provided: Option<&str>) -> bool {
        admin_token
            .map(|at| !at.is_empty() && provided == Some(at))
            .unwrap_or(false)
    }

    /// Test regular token acceptance (admin token also satisfies regular token).
    fn token_ok(expected: &str, provided: Option<&str>, is_admin: bool) -> bool {
        provided == Some(expected) || is_admin
    }

    #[test]
    fn test_admin_token_match() {
        assert!(is_admin_for(Some("secret"), Some("secret")));
    }

    #[test]
    fn test_admin_token_mismatch() {
        assert!(!is_admin_for(Some("secret"), Some("wrong")));
    }

    #[test]
    fn test_admin_token_none_provided() {
        assert!(!is_admin_for(Some("secret"), None));
    }

    #[test]
    fn test_no_admin_token_configured() {
        assert!(!is_admin_for(None, Some("anything")));
    }

    #[test]
    fn test_empty_admin_token_never_matches() {
        assert!(!is_admin_for(Some(""), Some("")));
        assert!(!is_admin_for(Some(""), Some("anything")));
    }

    #[test]
    fn test_admin_token_satisfies_regular_token() {
        // If user is admin, regular token check should pass even without matching regular token
        assert!(token_ok("regular", Some("admin-secret"), true));
    }

    #[test]
    fn test_regular_token_accepted() {
        assert!(token_ok("regular", Some("regular"), false));
    }

    #[test]
    fn test_wrong_token_rejected() {
        assert!(!token_ok("regular", Some("wrong"), false));
    }

    #[test]
    fn test_no_token_provided_rejected() {
        assert!(!token_ok("regular", None, false));
    }
}
