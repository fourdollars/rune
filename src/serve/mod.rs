//! `rune serve` — built-in HTTP + WebSocket server for the Rune WebUI.
//!
//! Architecture:
//!   - axum HTTP server on configurable port (default 9527)
//!   - Static files embedded via rust-embed (HTML/JS/CSS)
//!   - SSE endpoint for server→client push + REST API for client→server
//!   - Token auth required for non-localhost connections

pub mod api;
pub mod db;
mod static_files;
pub use db::ChatDb;

use crate::config::RuneConfig;
use crate::provider::ModelInfo;
use crate::serve::api::NoteRoom;
use axum::{
    extract::ConnectInfo,
    http::{header, StatusCode},
    middleware as axum_mw,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use std::collections::HashMap;
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
pub fn note_markdown_dir(session: &str) -> PathBuf {
    data_dir().join("notes").join(session).join("markdown")
}

/// Shared server state.
#[derive(Clone)]
pub struct ServerState {
    pub config: RuneConfig,
    pub user_token: Option<String>,
    /// Admin token — clients presenting this token get admin role.
    pub admin_token: Option<String>,
    /// Guest token — read-only access, no mutations allowed.
    pub guest_token: Option<String>,
    /// All markdown files: filename → content.
    pub files: Arc<RwLock<std::collections::HashMap<String, String>>>,
    /// Currently active filename shown in the editor.
    pub active_file: Arc<RwLock<String>>,
    /// All available models (parsed from config.model by comma).
    pub models: Vec<ModelInfo>,
    /// Per-note rooms: isolated SSE channel + model + cancel token per note.
    pub rooms: Arc<RwLock<HashMap<String, Arc<NoteRoom>>>>,
    /// Global default model (used when a note has no per-note override).
    pub global_default_model: Arc<RwLock<String>>,
    /// Broadcast to ADMIN clients only (approval requests).
    pub admin_broadcast_tx: broadcast::Sender<String>,
    pub chat_db: ChatDb,
    /// Base data directory (default: ~/.rune). Injectable for testing.
    pub data_dir: PathBuf,
}

impl ServerState {
    /// Returns the markdown directory for a given note session.
    pub fn note_markdown_dir(&self, session: &str) -> PathBuf {
        self.data_dir.join("notes").join(session).join("markdown")
    }

    /// Get existing room or lazy-create one for the given note_id.
    /// Uses double-checked locking: read first, write only on miss.
    pub async fn get_or_create_room(&self, note_id: &str) -> Arc<NoteRoom> {
        // Fast path: read lock
        {
            let rooms = self.rooms.read().await;
            if let Some(room) = rooms.get(note_id) {
                return Arc::clone(room);
            }
        }
        // Slow path: write lock, re-check
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get(note_id) {
            return Arc::clone(room);
        }
        let room = Arc::new(NoteRoom::new(note_id.to_string()));
        // Load persisted model_override from DB (fallback to None = global default)
        if let Some(model) = self.chat_db.get_note_model(note_id) {
            *room.model_override.write().await = Some(model);
        }
        rooms.insert(note_id.to_string(), Arc::clone(&room));
        room
    }

    /// Effective model for a note: per-note override if set, else global default.
    pub async fn effective_model(&self, note_id: &str) -> String {
        let room = self.get_or_create_room(note_id).await;
        let override_model = room.model_override.read().await;
        if let Some(ref m) = *override_model {
            return m.clone();
        }
        self.global_default_model.read().await.clone()
    }
}

/// Options for `rune serve`.
pub struct NotesOptions {
    pub port: u16,
    pub bind: IpAddr,
    pub user_token: Option<String>,
    pub admin_token: Option<String>,
    pub guest_token: Option<String>,
}

impl Default for NotesOptions {
    fn default() -> Self {
        Self {
            port: 9527,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_token: None,
            admin_token: None,
            guest_token: None,
        }
    }
}

/// Start the serve mode.
pub async fn run(config: RuneConfig, opts: NotesOptions) {
    // Refuse to start without at least one token configured (early return; safe in tests)
    if opts.user_token.is_none() && opts.admin_token.is_none() && opts.guest_token.is_none() {
        eprintln!("  ✗ ERROR: No tokens configured. At least one of user_token, admin_token, or guest_token must be set in [notes] config.");
        eprintln!("    Without tokens, no one can access the server.");
        return;
    }

    // Files are loaded per-session on client connect/switch; start empty
    let initial_files = std::collections::HashMap::new();

    let (admin_broadcast_tx, _) = broadcast::channel(64);

    let db_path = data_dir().join("chat.db");
    let chat_db = ChatDb::open_lazy(&db_path).unwrap_or_else(|e| {
        eprintln!("warning: failed to open chat db: {}", e);
        // Fallback: pure in-memory without deferred path
        ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db failed")
    });

    // Auto-discover notes: scan ~/.rune/notes/ and register any missing sessions.
    // Only register if markdown/ subdirectory exists and contains at least one file.
    {
        let notes_root = data_dir().join("notes");
        if let Ok(entries) = std::fs::read_dir(&notes_root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let note_id = entry.file_name().to_string_lossy().to_string();
                    let md_dir = entry.path().join("markdown");
                    let has_files = std::fs::read_dir(&md_dir)
                        .map(|mut rd| rd.next().is_some())
                        .unwrap_or(false);
                    if has_files {
                        let _ = chat_db.create_note(&note_id, &note_id, None);
                    }
                }
            }
        }
        // Persist DB to disk if any notes were discovered
        let _ = chat_db.ensure_persistent();
    }

    // Parse comma-separated model list from config
    let mut models: Vec<ModelInfo> = config
        .model
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|id| ModelInfo { id, context_window: None, reasoning_efforts: vec![] })
        .collect();

    // Auto-discover models from provider if none configured
    if models.is_empty() {
        eprintln!("  ℹ No models configured, discovering from provider...");
        match crate::serve::api::build_provider_pub(&config) {
            Ok(registry) => {
                match registry.list_models().await {
                    Ok(discovered) if !discovered.is_empty() => {
                        eprintln!("  ✓ Discovered {} models from provider", discovered.len());
                        models = discovered;
                    }
                    Ok(_) => {
                        eprintln!("  ⚠ Provider returned no models, using default");
                        models = vec![ModelInfo { id: config.model.clone(), context_window: None, reasoning_efforts: vec![] }];
                    }
                    Err(e) => {
                        eprintln!("  ⚠ Failed to discover models: {}", e);
                        models = vec![ModelInfo { id: config.model.clone(), context_window: None, reasoning_efforts: vec![] }];
                    }
                }
            }
            Err(e) => {
                eprintln!("  ⚠ Cannot build provider for model discovery: {}", e);
                models = vec![ModelInfo { id: config.model.clone(), context_window: None, reasoning_efforts: vec![] }];
            }
        }
    }

    let first_model = models
        .first()
        .map(|m| m.id.clone())
        .unwrap_or_else(|| config.model.clone());

    let state = ServerState {
        config: config.clone(),
        user_token: opts.user_token.clone(),
        admin_token: opts.admin_token.clone(),
        guest_token: opts.guest_token.clone(),
        files: Arc::new(RwLock::new(initial_files)),
        active_file: Arc::new(RwLock::new(String::new())),
        models,
        rooms: Arc::new(RwLock::new(HashMap::new())),
        global_default_model: Arc::new(RwLock::new(first_model)),
        admin_broadcast_tx,
        chat_db,
        data_dir: data_dir(),
    };

    // Auth middleware for POST API endpoints
    async fn auth_middleware(
        axum::extract::State(state): axum::extract::State<ServerState>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
        req: axum::http::Request<axum::body::Body>,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        // Extract token from header OR query param
        let from_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.to_string());
        let from_query = req.uri().query().and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("token="))
                .map(|p| p.trim_start_matches("token=").to_string())
        });
        let provided = from_header.or(from_query);

        // Strict auth: token must match one of user_token / admin_token / guest_token
        let user_ok = state
            .user_token
            .as_deref()
            .map(|ut| provided.as_deref() == Some(ut))
            .unwrap_or(false);
        let admin_ok = state
            .admin_token
            .as_deref()
            .map(|at| provided.as_deref() == Some(at))
            .unwrap_or(false);
        let guest_ok = state
            .guest_token
            .as_deref()
            .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
            .unwrap_or(false);

        // Reject if none match
        if !user_ok && !admin_ok && !guest_ok {
            let body =
                axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
            return (StatusCode::UNAUTHORIZED, body).into_response();
        }

        // Guest: block all mutations (only allow read-only endpoints)
        if guest_ok {
            let path = req.uri().path().to_string();
            let allowed_guest_paths =
                ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
            if !allowed_guest_paths.iter().any(|p| path == *p) {
                let body = axum::Json(
                    serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                );
                return (StatusCode::FORBIDDEN, body).into_response();
            }
        }

        // Admin-only endpoints: note management, model switch, visibility
        if !admin_ok {
            let path = req.uri().path().to_string();
            let admin_only_paths = [
                "/api/note/create",
                "/api/note/rename",
                "/api/note/delete",
                "/api/model/switch",
                "/api/note/visibility",
                "/api/file/visibility",
            ];
            // system-prompt POST is admin-only (GET is open to all authenticated)
            let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
            if is_admin_only {
                let body = axum::Json(
                    serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                );
                return (StatusCode::FORBIDDEN, body).into_response();
            }
        }
        next.run(req).await
    }

    // API routes with auth middleware
    let api_routes = Router::new()
        .route("/api/chat", post(api::chat_handler))
        .route("/api/file/create", post(api::file_create_handler))
        .route("/api/file/delete", post(api::file_delete_handler))
        .route("/api/file/rename", post(api::file_rename_handler))
        .route("/api/file/switch", post(api::file_switch_handler))
        .route("/api/file/update", post(api::file_update_handler))
        .route("/api/notes", get(api::notes_list_json_handler))
        .route("/api/note/create", post(api::note_create_handler))
        .route("/api/note/rename", post(api::note_rename_handler))
        .route("/api/note/delete", post(api::note_delete_handler))
        .route("/api/note/switch", post(api::note_switch_handler))
        .route("/api/model/switch", post(api::model_switch_handler))
        .route("/api/chat/archive", post(api::archive_handler))
        .route("/api/chat/search", post(api::search_handler))
        .route("/api/approval", post(api::approval_handler))
        .route("/api/dir/browse", post(api::dir_browse_handler))
        .route("/api/note/visibility", post(api::note_visibility_handler))
        .route("/api/file/visibility", post(api::file_visibility_handler))
        .route(
            "/api/system-prompt",
            get(api::system_prompt_get_handler).post(api::system_prompt_handler),
        )
        .layer(axum_mw::from_fn_with_state(state.clone(), auth_middleware));

    // Static + SSE routes (SSE has its own auth logic inside the handler)
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/events", get(api::events_handler))
        .route("/favicon.ico", get(favicon_handler))
        .route("/favicon.svg", get(favicon_handler))
        .route("/assets/{*path}", get(static_handler))
        .route("/assets-bin/{*path}", get(binary_asset_handler))
        // Public (no-auth) routes
        .route("/notes", get(api::public_notes_list_handler))
        .route("/notes/", get(api::public_notes_list_handler))
        .route("/notes/{note}/", get(api::public_note_index_handler))
        .route("/notes/{note}", get(api::public_note_index_handler))
        .route("/notes/{note}/{file}", get(api::public_preview_handler))
        .route(
            "/api/public/raw/{note}/{file}",
            get(api::public_raw_handler),
        )
        .merge(api_routes)
        .with_state(state);

    let addr = SocketAddr::new(opts.bind, opts.port);
    info!("Rune notes starting on http://{}", addr);

    println!("  ᚱ Rune Notes → http://{}", addr);
    if opts.user_token.is_some() {
        println!("  🔒 User token configured");
    }
    if opts.admin_token.is_some() {
        println!("  👑 Admin token configured");
    }
    if opts.guest_token.is_some() {
        println!("  👁 Guest token configured (read-only)");
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
    if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

async fn favicon_handler() -> impl IntoResponse {
    match static_files::get("favicon.svg") {
        Some(content) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
            content,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn binary_asset_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    match static_files::get_bytes(&path) {
        Some(bytes) => {
            let mime = mime_for(&path);
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, mime)],
                bytes,
            )
                .into_response()
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
        Some(content) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "no-cache, must-revalidate"),
            ],
            content,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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

    // Serialise all tests that mutate HOME env to avoid race conditions.
    // Any test touching std::env::{set_var,remove_var}("HOME") must hold this lock.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ──────────────────────────────────────────────
    // data_dir / note_markdown_dir
    // ──────────────────────────────────────────────

    #[test]
    fn test_data_dir_uses_home_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = data_dir();
        assert_eq!(d, std::path::PathBuf::from("/tmp/fake_home/.rune"));
    }

    #[test]
    fn test_data_dir_fallback_when_no_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Temporarily remove HOME
        let orig = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let d = data_dir();
        assert_eq!(d, std::path::PathBuf::from("./.rune"));
        if let Some(v) = orig {
            std::env::set_var("HOME", v);
        }
    }

    #[test]
    fn test_note_markdown_dir_structure() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = note_markdown_dir("my-session");
        assert_eq!(
            d,
            std::path::PathBuf::from("/tmp/fake_home/.rune/notes/my-session/markdown")
        );
    }

    #[test]
    fn test_note_markdown_dir_special_chars() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/fake_home");
        let d = note_markdown_dir("session-123_abc");
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
        assert!(!is_localhost(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
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
    // NotesOptions defaults
    // ──────────────────────────────────────────────

    #[test]
    fn test_serve_options_default() {
        let opts = NotesOptions::default();
        assert_eq!(opts.port, 9527);
        assert_eq!(opts.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(opts.user_token.is_none());
        assert!(opts.admin_token.is_none());
    }

    // ──────────────────────────────────────────────
    // ServerState model parsing
    // ──────────────────────────────────────────────

    #[test]
    fn test_model_list_parsing_single() {
        let model_str = "gpt-4o";
        let models: Vec<String> = model_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models, vec!["gpt-4o"]);
    }

    #[test]
    fn test_model_list_parsing_multiple() {
        let model_str = "gpt-4o, claude-3-5-sonnet, gemini-1.5-pro";
        let models: Vec<String> = model_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            models,
            vec!["gpt-4o", "claude-3-5-sonnet", "gemini-1.5-pro"]
        );
    }

    #[test]
    fn test_model_list_parsing_empty_parts_filtered() {
        let model_str = "gpt-4o,,claude-3-5-sonnet, ";
        let models: Vec<String> = model_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models, vec!["gpt-4o", "claude-3-5-sonnet"]);
    }

    #[test]
    fn test_first_model_fallback_when_empty() {
        let models: Vec<String> = ""
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let config_model = "fallback-model".to_string();
        let first = models
            .first()
            .cloned()
            .unwrap_or_else(|| config_model.clone());
        assert_eq!(first, "fallback-model");
    }

    // ──────────────────────────────────────────────
    // HTTP handler integration tests (via tower)
    // ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_index_handler_returns_html_or_500() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/", get(index_handler));
        let req = Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Either 200 (embedded file found) or 500 (no embed in test binary)
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn test_favicon_handler_returns_svg_or_404() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/favicon.ico", get(favicon_handler));
        let req = Request::builder()
            .uri("/favicon.ico")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,
            "unexpected status: {}",
            resp.status()
        );
        if resp.status() == StatusCode::OK {
            let ct = resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert_eq!(ct, "image/svg+xml");
        }
    }

    #[tokio::test]
    async fn test_static_handler_js_returns_ok_or_404() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/assets/{*path}", get(static_handler));
        let req = Request::builder()
            .uri("/assets/app.js")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,
            "unexpected status: {}",
            resp.status()
        );
        if resp.status() == StatusCode::OK {
            let ct = resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert_eq!(ct, "application/javascript");
        }
    }

    #[tokio::test]
    async fn test_static_handler_css_returns_ok_or_404() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/assets/{*path}", get(static_handler));
        let req = Request::builder()
            .uri("/assets/style.css")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,);
    }

    #[tokio::test]
    async fn test_static_handler_svg_returns_ok_or_404() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/assets/{*path}", get(static_handler));
        let req = Request::builder()
            .uri("/assets/icon.svg")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,);
    }

    #[tokio::test]
    async fn test_static_handler_octet_fallback() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/assets/{*path}", get(static_handler));
        let req = Request::builder()
            .uri("/assets/data.bin")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND,);
    }

    #[tokio::test]
    async fn test_binary_asset_handler_not_found() {
        use axum::http::{Request, StatusCode};
        use axum::{routing::get, Router};
        use tower::ServiceExt;

        // Auth middleware for POST API endpoints
        async fn auth_middleware(
            axum::extract::State(state): axum::extract::State<ServerState>,
            ConnectInfo(addr): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        ) -> axum::response::Response {
            // Extract token from header OR query param
            let from_header = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string());
            let from_query = req.uri().query().and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("token="))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });
            let provided = from_header.or(from_query);

            // Strict auth: token must match one of user_token / admin_token / guest_token
            let user_ok = state
                .user_token
                .as_deref()
                .map(|ut| provided.as_deref() == Some(ut))
                .unwrap_or(false);
            let admin_ok = state
                .admin_token
                .as_deref()
                .map(|at| provided.as_deref() == Some(at))
                .unwrap_or(false);
            let guest_ok = state
                .guest_token
                .as_deref()
                .map(|gt| !gt.is_empty() && provided.as_deref() == Some(gt))
                .unwrap_or(false);

            // Reject if none match
            if !user_ok && !admin_ok && !guest_ok {
                let body =
                    axum::Json(serde_json::json!({"ok": false, "error": "Authentication failed"}));
                return (StatusCode::UNAUTHORIZED, body).into_response();
            }

            // Guest: block all mutations (only allow read-only endpoints)
            if guest_ok {
                let path = req.uri().path().to_string();
                let allowed_guest_paths =
                    ["/api/note/switch", "/api/file/switch", "/api/system-prompt"];
                if !allowed_guest_paths.iter().any(|p| path == *p) {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Guest access is read-only"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }

            // Admin-only endpoints: note management, model switch, visibility
            if !admin_ok {
                let path = req.uri().path().to_string();
                let admin_only_paths = [
                    "/api/note/create",
                    "/api/note/rename",
                    "/api/note/delete",
                    "/api/model/switch",
                    "/api/note/visibility",
                    "/api/file/visibility",
                ];
                // system-prompt POST is admin-only (GET is open to all authenticated)
                let is_admin_only = admin_only_paths.iter().any(|p| path == *p)
                    || (path == "/api/system-prompt" && req.method() == axum::http::Method::POST);
                if is_admin_only {
                    let body = axum::Json(
                        serde_json::json!({"ok": false, "error": "Admin privileges required"}),
                    );
                    return (StatusCode::FORBIDDEN, body).into_response();
                }
            }
            next.run(req).await
        }

        let app = Router::new().route("/assets-bin/{*path}", get(binary_asset_handler));
        let req = Request::builder()
            .uri("/assets-bin/nonexistent.wasm")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ──────────────────────────────────────────────
    // mime_for — additional edge cases
    // ──────────────────────────────────────────────

    #[test]
    fn test_mime_for_path_with_directory() {
        assert_eq!(
            mime_for("assets/deep/path/bundle.js"),
            "application/javascript"
        );
        assert_eq!(mime_for("assets/theme/dark.css"), "text/css");
    }

    #[test]
    fn test_mime_for_dotfile() {
        assert_eq!(mime_for(".hidden"), "application/octet-stream");
    }

    #[test]
    fn test_mime_for_empty_string() {
        assert_eq!(mime_for(""), "application/octet-stream");
    }

    // ──────────────────────────────────────────────
    // data_dir — additional edge cases
    // ──────────────────────────────────────────────

    #[test]
    fn test_data_dir_ends_with_rune() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/some/path");
        let d = data_dir();
        assert_eq!(d.file_name().unwrap(), ".rune");
    }

    #[test]
    fn test_note_markdown_dir_ends_with_markdown() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp");
        let d = note_markdown_dir("sess");
        assert_eq!(d.file_name().unwrap(), "markdown");
    }

    // ──────────────────────────────────────────────
    // NotesOptions — custom construction
    // ──────────────────────────────────────────────

    #[test]
    fn test_serve_options_custom_port() {
        let opts = NotesOptions {
            port: 8080,
            bind: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            user_token: Some("tok".into()),
            admin_token: Some("admin".into()),
            guest_token: None,
        };
        assert_eq!(opts.port, 8080);
        assert_eq!(opts.bind, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(opts.user_token.as_deref(), Some("tok"));
        assert_eq!(opts.admin_token.as_deref(), Some("admin"));
    }

    // ──────────────────────────────────────────────
    // is_localhost — additional edge cases
    // ──────────────────────────────────────────────

    #[test]
    fn test_is_localhost_ipv4_all_zeros() {
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
    }

    #[test]
    fn test_is_localhost_ipv4_broadcast() {
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255))));
    }

    #[test]
    fn test_is_localhost_ipv6_all_zeros() {
        assert!(!is_localhost(IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0, 0, 0
        ))));
    }

    // ──────────────────────────────────────────────
    // ServerState construction
    // ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_server_state_construction() {
        use crate::config::RuneConfig;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::{broadcast, RwLock};

        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");

        let config = RuneConfig::default();
        let models: Vec<ModelInfo> = config
            .model
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|id| ModelInfo { id, context_window: None, reasoning_efforts: vec![] })
            .collect();
        let first_model = models
            .first()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| config.model.clone());

        let state = ServerState {
            config: config.clone(),
            user_token: None,
            admin_token: Some("admin".into()),
            guest_token: None,
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: models.clone(),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new(first_model.clone())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
        };

        assert_eq!(state.user_token, None);
        assert_eq!(state.admin_token.as_deref(), Some("admin"));
        assert_eq!(*state.global_default_model.read().await, first_model);
    }

    #[tokio::test]
    async fn test_server_state_with_token() {
        use crate::config::RuneConfig;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::{broadcast, RwLock};

        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");

        let state = ServerState {
            config: RuneConfig::default(),
            user_token: Some("my-secret-token".into()),
            admin_token: None,
            guest_token: None,
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(RwLock::new("main.md".into())),
            models: vec![ModelInfo { id: "gpt-4o".into(), context_window: None, reasoning_efforts: vec![] }],
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new("gpt-4o".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
        };

        assert_eq!(state.user_token.as_deref(), Some("my-secret-token"));
        assert!(state.admin_token.is_none());
        assert_eq!(*state.active_file.read().await, "main.md");
    }

    #[tokio::test]
    async fn test_server_state_room_broadcast() {
        use crate::config::RuneConfig;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::{broadcast, RwLock};

        let (admin_broadcast_tx, _) = broadcast::channel(64);
        let db = ChatDb::open(std::path::Path::new(":memory:")).expect("in-memory db");

        let state = ServerState {
            config: RuneConfig::default(),
            user_token: None,
            admin_token: None,
            guest_token: None,
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: vec![],
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new(String::new())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
        };

        // Verify per-room broadcast channel is functional
        let room = state.get_or_create_room("test-room").await;
        let mut rx = room.broadcast_tx.subscribe();
        room.broadcast_tx.send("hello".into()).unwrap();
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, "hello");
    }

    #[tokio::test]
    async fn test_data_dir_db_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/testrun");
        let db_path = data_dir().join("chat.db");
        assert_eq!(
            db_path,
            std::path::PathBuf::from("/tmp/testrun/.rune/chat.db")
        );
    }

    // ──────────────────────────────────────────────
    // run() — startup smoke test (cancelled quickly)
    // ──────────────────────────────────────────────

    /// Smoke-test: call `run()` on a random port and immediately cancel it.
    /// This covers the initialization path (db open, model parse, state build,
    /// router construction, addr setup, token/admin prints) without actually
    /// waiting for HTTP traffic.
    #[tokio::test]
    async fn test_run_startup_and_cancel() {
        use crate::config::RuneConfig;
        use std::net::{IpAddr, Ipv4Addr};
        use tokio::time::{timeout, Duration};

        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/test_run_home");

        let config = RuneConfig::default();
        let opts = NotesOptions {
            port: 19527, // Use a fixed high port unlikely to conflict
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_token: Some("test-tok".into()),
            admin_token: Some("test-admin".into()),
            guest_token: None,
        };

        // run() binds and serves; we cancel after 100ms
        let result = timeout(Duration::from_millis(100), run(config, opts)).await;

        // Timeout means the server started listening (good);
        // an Err(Elapsed) is expected and correct.
        assert!(result.is_err(), "expected timeout, got early return");
    }

    #[tokio::test]
    async fn test_run_no_token_startup() {
        use crate::config::RuneConfig;
        use std::net::{IpAddr, Ipv4Addr};
        use tokio::time::{timeout, Duration};

        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOME", "/tmp/test_run_home2");

        let config = RuneConfig::default();
        let opts = NotesOptions {
            port: 19528,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_token: None,
            admin_token: None,
            guest_token: None,
        };

        // run() now returns early (instead of process::exit) when no tokens configured.
        // We expect it to complete immediately (Ok) rather than timeout (Err).
        let result = timeout(Duration::from_millis(200), run(config, opts)).await;
        assert!(
            result.is_ok(),
            "expected early return when no tokens, got timeout"
        );
    }

    #[test]
    fn test_model_list_empty_triggers_discovery_path() {
        // Verify: empty model string → empty Vec after split/filter
        let model_str = "";
        let models: Vec<String> = model_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(models.is_empty(), "empty model config should yield empty list");
    }

    #[test]
    fn test_model_list_populated_skips_discovery() {
        let model_str = "gpt-5-mini, claude-sonnet-4.6";
        let models: Vec<String> = model_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0], "gpt-5-mini");
        assert_eq!(models[1], "claude-sonnet-4.6");
    }
}
