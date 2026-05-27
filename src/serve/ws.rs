//! WebSocket handler for chat + spec.md sync.

use crate::agent::{Agent, StopReason};
use crate::config::{self, RuneConfig};
use crate::embedding::EmbeddingEngine;
use crate::provider::{CopilotProvider, GeminiProvider, OpenAiProvider, ProviderRegistry};
use crate::serve::ServerState;
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
                        info!(
                            "Chat message received: {}",
                            &content[..content.len().min(50)]
                        );
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
                        // TODO: Route to pending approval handler in future phases
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

/// Handle a chat message — create an Agent, run it with streaming, and report.
async fn handle_chat_message(
    user_msg: String,
    config: RuneConfig,
    spec_content: Arc<RwLock<String>>,
    tx: mpsc::UnboundedSender<ServerMsg>,
) {
    // Build provider
    let provider = match build_provider(&config) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(ServerMsg::Error {
                message: format!("Provider error: {}", e),
            });
            let _ = tx.send(ServerMsg::Status {
                state: "idle".to_string(),
            });
            return;
        }
    };

    // Build embedding engine (optional)
    let embedding = build_embedding(&config).await;

    // Create agent with streaming callback
    let tx_token = tx.clone();
    let token_callback: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |token: &str| {
        let _ = tx_token.send(ServerMsg::ChatToken {
            content: token.to_string(),
        });
    });

    let mut agent = Agent::new(config.clone(), provider, true, embedding);
    agent.token_callback = Some(token_callback);
    agent.spec_content = Some(spec_content.clone());

    // Set system prompt with spec awareness
    let system_prompt = build_system_prompt(&config).await;
    agent.set_system_prompt(&system_prompt);

    // Send typing status
    let _ = tx.send(ServerMsg::Status {
        state: "typing".to_string(),
    });

    // Run the agent
    let result = agent.run(&user_msg).await;

    // Handle the result
    match &result {
        StopReason::FinalAnswer(answer) => {
            // If there was no streaming (non-streaming provider), send the full answer
            if !answer.is_empty() {
                // The streaming callback already sent tokens, but for non-streaming
                // providers the answer comes here directly
                // Check if we already streamed (step_count > 0 means we went through the loop)
            }
        }
        StopReason::Error(e) => {
            let _ = tx.send(ServerMsg::ChatToken {
                content: format!("\n\n⚠ Error: {}", e),
            });
        }
        StopReason::MaxSteps => {
            let _ = tx.send(ServerMsg::ChatToken {
                content: "\n\n⚠ Stopped: maximum steps reached".to_string(),
            });
        }
        StopReason::TokenBudgetExhausted => {
            let _ = tx.send(ServerMsg::ChatToken {
                content: "\n\n⚠ Stopped: token budget exhausted".to_string(),
            });
        }
        StopReason::UserInterrupt => {}
    }

    // Send chat done + spec update (in case AI edited spec)
    let _ = tx.send(ServerMsg::ChatDone {});

    // If spec was edited by the agent, push new content to client
    let new_spec = spec_content.read().await.clone();
    let _ = tx.send(ServerMsg::SpecFull {
        content: new_spec.clone(),
    });

    // Persist spec to disk
    let spec_path = super::data_dir().join("spec.md");
    if let Err(e) = tokio::fs::write(&spec_path, &new_spec).await {
        warn!("Failed to persist spec.md after agent edit: {}", e);
    }

    let _ = tx.send(ServerMsg::Status {
        state: "idle".to_string(),
    });
}

/// Build a ProviderRegistry from config (mirrors cli::init_provider logic).
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

    Ok(registry)
}

/// Build embedding engine if configured.
async fn build_embedding(config: &RuneConfig) -> Option<EmbeddingEngine> {
    use crate::embedding::EmbeddingConfig;

    if config.embedding.model.is_some() || config.embedding.enabled {
        let mut emb_cfg = config.embedding.clone();
        // Copy API key if not set in embedding config
        if emb_cfg.api_key.is_none() {
            emb_cfg.api_key = config.api_key.clone();
        }

        let is_copilot = config
            .provider
            .as_deref()
            .map(|p| p.contains("copilot"))
            .unwrap_or_else(|| {
                config
                    .api_key
                    .as_deref()
                    .map(|k| k.starts_with("ghu_") || k.starts_with("ghp_"))
                    .unwrap_or(false)
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
    let base = config.system_prompt.as_deref().unwrap_or(
        "You are Rune, a high-performance zero-trust AI agent. \
         You are currently in WebUI serve mode, collaborating with the user on a shared spec document (spec.md). \
         You can read and edit the spec using the read_spec and edit_spec tools. \
         The spec.md is displayed in real-time in the center panel. \
         When editing the spec, prefer targeted search+replace edits over full replacement. \
         Be concise in chat; put detailed content into the spec document.",
    );
    base.to_string()
}
