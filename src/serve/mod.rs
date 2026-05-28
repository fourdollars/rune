//! `rune serve` — built-in HTTP + WebSocket server for the Rune WebUI.
//!
//! Architecture:
//!   - axum HTTP server on configurable port (default 9527)
//!   - Static files embedded via rust-embed (HTML/JS/CSS)
//!   - WebSocket endpoint for chat streaming + markdown file sync
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
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".rune")
}

/// Get the markdown directory for a session: ~/.rune/sessions/<session>/markdown/
pub fn session_markdown_dir(session: &str) -> PathBuf {
    data_dir().join("sessions").join(session).join("markdown")
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
    /// All available models (parsed from config.model by comma).
    pub models: Vec<String>,
    /// Currently selected model (may be overridden at runtime).
    pub active_model: Arc<RwLock<String>>,
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
    // Files are loaded per-session on client connect/switch; start empty
    let initial_files = std::collections::HashMap::new();

    let (broadcast_tx, _) = broadcast::channel(256);
    let (admin_broadcast_tx, _) = broadcast::channel(64);

    let db_path = data_dir().join("chat.db");
    let chat_db = ChatDb::open_lazy(&db_path).unwrap_or_else(|e| {
        eprintln!("warning: failed to open chat db: {}", e);
        // Fallback: pure in-memory without deferred path
        ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db failed")
    });

    // Parse comma-separated model list from config
    let models: Vec<String> = config.model
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let first_model = models.first().cloned().unwrap_or_else(|| config.model.clone());

    let state = ServerState {
        config: config.clone(),
        token: opts.token.clone(),
        admin_token: opts.admin_token.clone(),
        files: Arc::new(RwLock::new(initial_files)),
        active_file: Arc::new(RwLock::new(String::new())),
        models,
        active_model: Arc::new(RwLock::new(first_model)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    // ──────────────────────────────────────────────
    // data_dir / session_markdown_dir
    // ──────────────────────────────────────────────

    #[test]
    fn test_data_dir_uses_home_env() {
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = data_dir();
        assert_eq!(d, std::path::PathBuf::from("/tmp/fake_home/.rune"));
    }

    #[test]
    fn test_data_dir_fallback_when_no_home() {
        // Temporarily remove HOME
        let orig = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let d = data_dir();
        assert_eq!(d, std::path::PathBuf::from("./.rune"));
        if let Some(v) = orig { std::env::set_var("HOME", v); }
    }

    #[test]
    fn test_session_markdown_dir_structure() {
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = session_markdown_dir("my-session");
        assert_eq!(d, std::path::PathBuf::from("/tmp/fake_home/.rune/sessions/my-session/markdown"));
    }

    #[test]
    fn test_session_markdown_dir_special_chars() {
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = session_markdown_dir("session-123_abc");
        assert!(d.to_string_lossy().contains("session-123_abc"));
    }

    // ──────────────────────────────────────────────
    // is_localhost
    // ──────────────────────────────────────────────

    #[test]
    fn test_is_localhost_ipv4_loopback() {
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn test_is_localhost_ipv4_127_0_0_1() {
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn test_is_localhost_ipv4_non_local() {
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn test_is_localhost_ipv6_loopback() {
        assert!(is_localhost(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn test_is_localhost_ipv6_non_local() {
        assert!(!is_localhost(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
    }

    // ──────────────────────────────────────────────
    // mime_for
    // ──────────────────────────────────────────────

    #[test]
    fn test_mime_for_js() {
        assert_eq!(mime_for("bundle.js"), "application/javascript");
        assert_eq!(mime_for("path/to/app.js"), "application/javascript");
    }

    #[test]
    fn test_mime_for_css() {
        assert_eq!(mime_for("style.css"), "text/css");
    }

    #[test]
    fn test_mime_for_html() {
        assert_eq!(mime_for("index.html"), "text/html");
    }

    #[test]
    fn test_mime_for_svg() {
        assert_eq!(mime_for("icon.svg"), "image/svg+xml");
    }

    #[test]
    fn test_mime_for_unknown() {
        assert_eq!(mime_for("file.wasm"), "application/octet-stream");
        assert_eq!(mime_for("data.bin"), "application/octet-stream");
        assert_eq!(mime_for("README.md"), "application/octet-stream");
    }

    // ──────────────────────────────────────────────
    // ServeOptions defaults
    // ──────────────────────────────────────────────

    #[test]
    fn test_serve_options_default() {
        let opts = ServeOptions::default();
        assert_eq!(opts.port, 9527);
        assert_eq!(opts.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(opts.token.is_none());
        assert!(opts.admin_token.is_none());
    }

    // ──────────────────────────────────────────────
    // ServerState model parsing
    // ──────────────────────────────────────────────

    #[test]
    fn test_model_list_parsing_single() {
        let model_str = "gpt-4o";
        let models: Vec<String> = model_str.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models, vec!["gpt-4o"]);
    }

    #[test]
    fn test_model_list_parsing_multiple() {
        let model_str = "gpt-4o, claude-3-5-sonnet, gemini-1.5-pro";
        let models: Vec<String> = model_str.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models, vec!["gpt-4o", "claude-3-5-sonnet", "gemini-1.5-pro"]);
    }

    #[test]
    fn test_model_list_parsing_empty_parts_filtered() {
        let model_str = "gpt-4o,,claude-3-5-sonnet, ";
        let models: Vec<String> = model_str.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models, vec!["gpt-4o", "claude-3-5-sonnet"]);
    }

    #[test]
    fn test_first_model_fallback_when_empty() {
        let models: Vec<String> = "".split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let config_model = "fallback-model".to_string();
        let first = models.first().cloned().unwrap_or_else(|| config_model.clone());
        assert_eq!(first, "fallback-model");
    }
}
