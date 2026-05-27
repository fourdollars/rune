//! `rune serve` — built-in HTTP + WebSocket server for the Rune WebUI.
//!
//! Architecture:
//!   - axum HTTP server on configurable port (default 9527)
//!   - Static files embedded via rust-embed (HTML/JS/CSS)
//!   - WebSocket endpoint for chat streaming + spec.md sync
//!   - Token auth required for non-localhost connections

pub mod db;
mod static_files;
mod ws;
pub use db::ChatDb;

use crate::config::RuneConfig;
use axum::{
    extract::{ConnectInfo, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
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
    /// Admin token — clients presenting this token get admin role.
    pub admin_token: Option<String>,
    /// All markdown files: filename → content.
    pub files: Arc<RwLock<std::collections::HashMap<String, String>>>,
    /// Currently active filename shown in the editor.
    pub active_file: Arc<RwLock<String>>,
    /// Broadcast to ALL connected clients.
    pub broadcast_tx: broadcast::Sender<String>,
    /// Broadcast to ADMIN clients only (approval requests).
    pub admin_broadcast_tx: broadcast::Sender<String>,
    pub chat_db: ChatDb,
}

/// Options for `rune serve`.
pub struct ServeOptions {
    pub port: u16,
    pub bind: IpAddr,
    pub token: Option<String>,
    pub admin_token: Option<String>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            port: 9527,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            token: None,
            admin_token: None,
        }
    }
}

/// Start the serve mode.
pub async fn run(config: RuneConfig, opts: ServeOptions) {
    // Load all .md files from data_dir into the HashMap
    let data = data_dir();
    tokio::fs::create_dir_all(&data).await.ok();
    let spec_path = data.join("spec.md");
    let initial_spec = tokio::fs::read_to_string(&spec_path)
        .await
        .unwrap_or_else(|_| "# Spec\n\nStart writing your spec here.\n".to_string());
    let mut initial_files = std::collections::HashMap::new();
    initial_files.insert("spec.md".to_string(), initial_spec);

    let (broadcast_tx, _) = broadcast::channel(256);
    let (admin_broadcast_tx, _) = broadcast::channel(64);

    let db_path = data_dir().join("chat.db");
    let chat_db = ChatDb::open(&db_path).unwrap_or_else(|e| {
        eprintln!("warning: failed to open chat db: {}", e);
        ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db failed")
    });

    let state = ServerState {
        config: config.clone(),
        token: opts.token.clone(),
        admin_token: opts.admin_token.clone(),
        files: Arc::new(RwLock::new(initial_files)),
        active_file: Arc::new(RwLock::new("spec.md".to_string())),
        broadcast_tx,
        admin_broadcast_tx,
        chat_db,
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/favicon.ico", get(favicon_handler))
        .route("/favicon.svg", get(favicon_handler))
        .route("/assets/{*path}", get(static_handler))
        .route("/assets-bin/{*path}", get(binary_asset_handler))
        .with_state(state);

    let addr = SocketAddr::new(opts.bind, opts.port);
    info!("Rune serve starting on http://{}", addr);

    println!("  ᚱ Rune WebUI → http://{}", addr);
    if opts.token.is_some() {
        println!("  🔒 Token auth enabled for non-localhost");
    }
    if opts.admin_token.is_some() {
        println!("  👑 Admin token configured");
    }

    // Ignore SIGHUP so server stays up when SSH session ends
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut hup = signal(SignalKind::hangup()).expect("failed to register SIGHUP handler");
        tokio::spawn(async move {
            loop {
                hup.recv().await;
                info!("Received SIGHUP, ignoring (server continues)");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind address");

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        eprintln!("Server error: {}", e);
    }
}

/// Serve the main index.html.
async fn index_handler() -> impl IntoResponse {
    match static_files::get("index.html") {
        Some(content) => Html(content).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Static asset handler.
fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".js")   { "application/javascript" }
    else if path.ends_with(".css")  { "text/css" }
    else if path.ends_with(".html") { "text/html" }
    else if path.ends_with(".svg")  { "image/svg+xml" }
    else { "application/octet-stream" }
}

async fn favicon_handler() -> impl IntoResponse {
    match static_files::get("favicon.svg") {
        Some(content) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
            content,
        ).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn binary_asset_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    match static_files::get_bytes(&path) {
        Some(bytes) => {
            let mime = mime_for(&path);
            (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

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

/// WebSocket upgrade handler — token auth deferred to handshake message.
/// Clients send token inside set_nickname; URL query token no longer required.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    // Localhost always allowed.
    // Remote only allowed when token is configured (verified in ws::handle_connection).
    if !is_localhost(addr.ip()) && state.token.is_none() {
        return StatusCode::FORBIDDEN.into_response();
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
