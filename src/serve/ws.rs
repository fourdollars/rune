//! WebSocket handler for chat + spec.md sync.

use crate::agent::{Agent, StopReason};
use crate::config::RuneConfig;
use crate::provider::{CopilotProvider, GeminiProvider, OpenAiProvider, ProviderRegistry};
use crate::serve::ServerState;
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;
use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// Incoming WebSocket message types from the client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
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

    /// Spec patch from AI edit.
    #[serde(rename = "spec_patch")]
    SpecPatch { content: String },

    /// Agent status change.
    #[serde(rename = "status")]
    Status { state: String },

    /// Approval request from tool execution.
    #[serde(rename = "approval_request")]
    ApprovalRequest { id: String, detail: String },

    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },
}

/// Handle a single WebSocket connection.
pub async fn handle_connection(socket: WebSocket, state: ServerState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send initial spec content
    let spec = state.spec_content.read().await.clone();
    let init_msg = ServerMsg::SpecFull { content: spec };
    if let Ok(json) = serde_json::to_string(&init_msg) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Send ready status
    let status_msg = ServerMsg::Status {
        state: "idle".to_string(),
    };
    if let Ok(json) = serde_json::to_string(&status_msg) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    info!("WebSocket client connected");

    // Channel for sending messages back to websocket from agent task
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    // Spawn a task to forward channel messages to websocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Process incoming messages
    let spec_content = state.spec_content.clone();
    let config = state.config.clone();

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let text_str: &str = &text;
                match serde_json::from_str::<ClientMsg>(text_str) {
                    Ok(ClientMsg::ChatSend { content }) => {
                        info!("Chat message received: {}", &content[..content.len().min(50)]);
                        let tx_clone = tx.clone();
                        let config_clone = config.clone();
                        let spec_clone = spec_content.clone();

                        // Send thinking status
                        let _ = tx_clone.send(ServerMsg::Status {
                            state: "thinking".to_string(),
                        });

                        // Spawn agent task
                        tokio::spawn(async move {
                            handle_chat_message(content, config_clone, spec_clone, tx_clone).await;
                        });
                    }
                    Ok(ClientMsg::SpecUpdate { content }) => {
                        debug!("Spec update from client");
                        let mut spec = spec_content.write().await;
                        *spec = content.clone();
                        // Persist to disk
                        let spec_path = super::data_dir().join("spec.md");
                        if let Err(e) = tokio::fs::write(&spec_path, &content).await {
                            warn!("Failed to persist spec.md: {}", e);
                        }
                    }
                    Ok(ClientMsg::ApprovalResponse { id, approved }) => {
                        info!("Approval response: {} = {}", id, approved);
                        // TODO: Route to pending approval handler
                    }
                    Err(e) => {
                        warn!("Invalid WebSocket message: {}", e);
                        let _ = tx.send(ServerMsg::Error {
                            message: format!("Invalid message format: {}", e),
                        });
                    }
                }
            }
            Message::Close(_) => {
                info!("WebSocket client disconnected");
                break;
            }
            _ => {}
        }
    }

    send_task.abort();
}

/// Handle a chat message — run the agent and stream responses.
async fn handle_chat_message(
    user_msg: String,
    config: RuneConfig,
    spec_content: Arc<RwLock<String>>,
    tx: mpsc::UnboundedSender<ServerMsg>,
) {
    // For MVP: simple echo-back with spec context
    // TODO: Full agent integration with streaming callback
    let spec = spec_content.read().await.clone();

    // Simulate thinking then respond
    // In full implementation, this creates an Agent instance with a streaming callback
    let _ = tx.send(ServerMsg::ChatToken {
        content: format!("Received: {}\n\n", user_msg),
    });

    let _ = tx.send(ServerMsg::ChatToken {
        content: format!("(Current spec: {} bytes)\n", spec.len()),
    });

    let _ = tx.send(ServerMsg::ChatToken {
        content: "[Agent integration pending — MVP echo mode]".to_string(),
    });

    let _ = tx.send(ServerMsg::ChatDone {});
    let _ = tx.send(ServerMsg::Status {
        state: "idle".to_string(),
    });
}
