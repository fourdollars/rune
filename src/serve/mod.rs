//! `rune serve` — built-in HTTP + WebSocket server for the Rune WebUI.
//!
//! Architecture:
//!   - axum HTTP server on configurable port (default 9527)
//!   - Static files embedded via rust-embed (HTML/JS/CSS)
//!   - WebSocket endpoint for chat streaming + spec.md sync
//!   - Token auth required for non-localhost connections

mod static_files;
mod ws;

use crate::config::RuneConfig;
use axum::{
    extract::{ConnectInfo, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Get the Rune data directory (~/.rune).
fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".rune")
}

/// Shared server state.
#[derive(Clone)]
pub struct ServerState {
    pub config: RuneConfig,
    pub token: Option<String>,
    pub spec_content: Arc<RwLock<String>>,
}

/// Options for `rune serve`.
pub struct ServeOptions {
    pub port: u16,
    pub bind: IpAddr,
    pub token: Option<String>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            port: 9527,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            token: None,
        }
    }
}

/// Start the serve mode.
pub async fn run(config: RuneConfig, opts: ServeOptions) {
    let spec_path = data_dir().join("spec.md");
    let initial_spec = tokio::fs::read_to_string(&spec_path)
        .await
        .unwrap_or_else(|_| "# Spec\n\nStart writing your spec here.\n".to_string());

    let state = ServerState {
        config: config.clone(),
        token: opts.token.clone(),
        spec_content: Arc::new(RwLock::new(initial_spec)),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/assets/{*path}", get(static_handler))
        .with_state(state);

    let addr = SocketAddr::new(opts.bind, opts.port);
    info!("Rune serve starting on http://{}", addr);

    println!("  ᚱ Rune WebUI → http://{}", addr);
    if opts.token.is_some() {
        println!("  🔒 Token auth enabled for non-localhost");
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind address");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("Server error");
}

/// Serve the main index.html.
async fn index_handler() -> impl IntoResponse {
    match static_files::get("index.html") {
        Some(content) => Html(content).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Static asset handler.
async fn static_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    let mime = if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    };

    match static_files::get(&path) {
        Some(content) => ([(header::CONTENT_TYPE, mime)], content).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// WebSocket upgrade handler with auth check.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Auth check: localhost is free, remote needs token
    if !is_localhost(addr.ip()) {
        if let Some(ref expected_token) = state.token {
            let provided = params.get("token").map(|s| s.as_str()).unwrap_or("");
            if provided != expected_token {
                return StatusCode::UNAUTHORIZED.into_response();
            }
        } else {
            // No token configured but remote connection — reject
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    ws.on_upgrade(move |socket| ws::handle_connection(socket, state))
        .into_response()
}

fn is_localhost(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST || v4 == Ipv4Addr::new(127, 0, 0, 1),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}
