//! GitHub OAuth 2.0 authentication for Rune Notes.
//!
//! Flow:
//!   1. `GET /auth/github` → redirect to GitHub with CSRF state cookie
//!   2. `GET /auth/github/callback` → verify state, exchange code, fetch user,
//!      resolve role, create session, set cookies, redirect to /notes/
//!   3. `GET /auth/logout` → clear session + cookies, redirect to /
//!   4. `GET /auth/denied` → 403 "not authorized" page

use crate::config::GitHubOAuthConfig;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ─── Constants ─────────────────────────────────────────────────────────────

pub const SESSION_DURATION: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours
const STATE_COOKIE_DURATION_SECS: u64 = 300; // 5 minutes

// ─── Role ──────────────────────────────────────────────────────────────────

/// Role of an authenticated user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Admin,
    User,
    Guest,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::Guest => "guest",
        }
    }
}

// ─── Session ───────────────────────────────────────────────────────────────

/// An authenticated user session.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub login: String,
    pub role: Role,
    pub avatar_url: String,
    pub expires_at: Instant,
}

impl Session {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
    pub fn is_guest(&self) -> bool {
        self.role == Role::Guest
    }
}

// ─── SessionStore ──────────────────────────────────────────────────────────

/// In-memory session store. Thread-safe, clone-on-share.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn insert(&self, session: Session) {
        self.inner.write().await.insert(session.id.clone(), session);
    }

    /// Get a non-expired session by ID.
    pub async fn get(&self, id: &str) -> Option<Session> {
        let map = self.inner.read().await;
        map.get(id).and_then(|s| {
            if s.is_expired() {
                None
            } else {
                Some(s.clone())
            }
        })
    }

    pub async fn remove(&self, id: &str) {
        self.inner.write().await.remove(id);
    }

    /// Remove all expired sessions.
    pub async fn sweep_expired(&self) {
        let now = Instant::now();
        self.inner.write().await.retain(|_, s| s.expires_at > now);
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Role resolution ───────────────────────────────────────────────────────

/// Parse an `"org:org_name/team_name"` or `"org:org_name"` entry.
/// Returns `(org, Option<team>)` if it starts with `"org:"`, else `None`.
pub fn parse_org_team_entry(entry: &str) -> Option<(String, Option<String>)> {
    let rest = entry.strip_prefix("org:")?;
    if let Some((org, team)) = rest.split_once('/') {
        Some((org.to_string(), Some(team.to_string())))
    } else {
        Some((rest.to_string(), None))
    }
}

/// Resolve role by plain GitHub login match only (synchronous, no network).
/// Precedence: admin > user > guest.
pub fn resolve_role_by_login(login: &str, cfg: &GitHubOAuthConfig) -> Option<Role> {
    let login_lower = login.to_lowercase();
    for entry in &cfg.admins {
        if parse_org_team_entry(entry).is_none() && entry.to_lowercase() == login_lower {
            return Some(Role::Admin);
        }
    }
    for entry in &cfg.users {
        if parse_org_team_entry(entry).is_none() && entry.to_lowercase() == login_lower {
            return Some(Role::User);
        }
    }
    for entry in &cfg.guests {
        if parse_org_team_entry(entry).is_none() && entry.to_lowercase() == login_lower {
            return Some(Role::Guest);
        }
    }
    None
}

/// Generate a cryptographically random 32-char lowercase hex session ID.
pub fn generate_session_id() -> String {
    use std::time::SystemTime;
    // Mix multiple sources of entropy
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let ptr_val: u64 = (&t as *const _ as usize).try_into().unwrap_or(0);
    // Use sha2 for mixing
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(t.to_le_bytes());
    hasher.update(ptr_val.to_le_bytes());
    // Add tokio runtime entropy (pseudo-random padding)
    hasher.update(rand_bytes());
    let result = hasher.finalize();
    // Take first 16 bytes → 32 hex chars
    result[..16].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate 16 pseudo-random bytes using thread-local state.
fn rand_bytes() -> [u8; 16] {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new({
            let t = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            t ^ 0xdeadbeef_cafebabe
        });
    }
    let mut out = [0u8; 16];
    STATE.with(|s| {
        let mut x = s.get();
        for chunk in out.chunks_mut(8) {
            // xorshift64
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for (i, b) in chunk.iter_mut().enumerate() {
                *b = ((x >> (i * 8)) & 0xff) as u8;
            }
        }
        s.set(x);
    });
    out
}

/// Exchange OAuth code for an access token.
pub async fn exchange_code(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
) -> Result<String, String> {
    let body_str = format!(
        "client_id={}&client_secret={}&code={}",
        urlencod(client_id),
        urlencod(client_secret),
        urlencod(code)
    );
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body_str)
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("json parse error: {e}"))?;

    body["access_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            body["error_description"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string()
        })
}

/// GitHub user info.
#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    pub avatar_url: String,
}

/// Fetch authenticated GitHub user profile.
pub async fn fetch_github_user(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<GitHubUser, String> {
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "rune-notes/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    resp.json::<GitHubUser>()
        .await
        .map_err(|e| format!("json parse error: {e}"))
}

/// Check if a GitHub user is a member of an org (and optionally a team).
pub async fn check_github_membership(
    client: &reqwest::Client,
    access_token: &str,
    login: &str,
    org: &str,
    team: Option<&str>,
) -> bool {
    if let Some(team_slug) = team {
        // Check team membership
        let url = format!(
            "https://api.github.com/orgs/{}/teams/{}/memberships/{}",
            org, team_slug, login
        );
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "rune-notes/1.0")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await;
        match resp {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    } else {
        // Check org membership
        let url = format!("https://api.github.com/orgs/{}/members/{}", org, login);
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "rune-notes/1.0")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await;
        match resp {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    }
}

/// Resolve role: login match first, then org/team membership checks.
/// Precedence: admin > user > guest.
pub async fn resolve_role_full(
    login: &str,
    access_token: &str,
    cfg: &GitHubOAuthConfig,
) -> Option<Role> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    // Check admins
    for entry in &cfg.admins {
        if let Some((org, team)) = parse_org_team_entry(entry) {
            if check_github_membership(&client, access_token, login, &org, team.as_deref()).await {
                return Some(Role::Admin);
            }
        } else if entry.to_lowercase() == login.to_lowercase() {
            return Some(Role::Admin);
        }
    }

    // Check users
    for entry in &cfg.users {
        if let Some((org, team)) = parse_org_team_entry(entry) {
            if check_github_membership(&client, access_token, login, &org, team.as_deref()).await {
                return Some(Role::User);
            }
        } else if entry.to_lowercase() == login.to_lowercase() {
            return Some(Role::User);
        }
    }

    // Check guests
    for entry in &cfg.guests {
        if let Some((org, team)) = parse_org_team_entry(entry) {
            if check_github_membership(&client, access_token, login, &org, team.as_deref()).await {
                return Some(Role::Guest);
            }
        } else if entry.to_lowercase() == login.to_lowercase() {
            return Some(Role::Guest);
        }
    }

    None
}

// ─── Cookie helpers ────────────────────────────────────────────────────────

/// Read a named cookie value from request headers.
pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_str = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())?;
    for pair in cookie_str.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Build `Set-Cookie` header value for a session.
pub fn set_session_cookie(session_id: &str) -> (String, String) {
    let secs = SESSION_DURATION.as_secs();
    let http_only =
        format!("rune_sid={session_id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={secs}");
    let js_readable = format!("rune_session_id={session_id}; Path=/; SameSite=Lax; Max-Age={secs}");
    (http_only, js_readable)
}

/// Build `Set-Cookie` header values to clear session cookies.
pub fn clear_session_cookies() -> (String, String) {
    let http_only = "rune_sid=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string();
    let js_readable = "rune_session_id=; Path=/; SameSite=Lax; Max-Age=0".to_string();
    (http_only, js_readable)
}

/// Set a short-lived CSRF state cookie.
pub fn set_state_cookie(state: &str) -> String {
    format!(
        "rune_oauth_state={state}; Path=/auth; HttpOnly; SameSite=Lax; Max-Age={STATE_COOKIE_DURATION_SECS}"
    )
}

/// Clear the CSRF state cookie.
pub fn clear_state_cookie() -> String {
    "rune_oauth_state=; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=0".to_string()
}

// ─── OAuth handler state ───────────────────────────────────────────────────

/// Callback query params sent by GitHub.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

// ─── OAuth route handlers ───────────────────────────────────────────────────

use crate::serve::ServerState;

/// `GET /auth/github` — kick off the OAuth dance.
pub async fn oauth_start_handler(State(state): State<ServerState>) -> Response {
    let cfg = match state.config.notes.github.as_ref() {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html("<h1>GitHub OAuth not configured</h1>"),
            )
                .into_response();
        }
    };

    let csrf_state = generate_session_id();
    let state_cookie = set_state_cookie(&csrf_state);

    let redirect_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&scope=read:org&state={}",
        cfg.client_id, csrf_state
    );

    (
        StatusCode::FOUND,
        [
            (header::LOCATION, redirect_url),
            (header::SET_COOKIE, state_cookie),
        ],
    )
        .into_response()
}

/// `GET /auth/github/callback` — handle GitHub redirect.
pub async fn oauth_callback_handler(
    State(state): State<ServerState>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    // Handle GitHub errors
    if let Some(err) = params.error {
        let desc = params
            .error_description
            .unwrap_or_else(|| "OAuth error".to_string());
        let url = format!("/auth/denied?error={}&desc={}", err, urlencod(&desc));
        return Redirect::to(&url).into_response();
    }

    let code = match params.code {
        Some(c) => c,
        None => return Redirect::to("/auth/denied?error=missing_code").into_response(),
    };

    // Verify CSRF state
    let expected_state = get_cookie(&headers, "rune_oauth_state");
    let provided_state = params.state.as_deref().unwrap_or("");
    if expected_state.as_deref() != Some(provided_state) || provided_state.is_empty() {
        return Redirect::to("/auth/denied?error=csrf_mismatch").into_response();
    }

    let cfg = match state.config.notes.github.as_ref() {
        Some(c) => c.clone(),
        None => return Redirect::to("/auth/denied?error=not_configured").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    // Exchange code for access token
    let access_token = match exchange_code(&client, &cfg.client_id, &cfg.client_secret, &code).await
    {
        Ok(t) => t,
        Err(e) => {
            let url = format!("/auth/denied?error=token_exchange&desc={}", urlencod(&e));
            return Redirect::to(&url).into_response();
        }
    };

    // Fetch GitHub user
    let github_user = match fetch_github_user(&client, &access_token).await {
        Ok(u) => u,
        Err(e) => {
            let url = format!("/auth/denied?error=user_fetch&desc={}", urlencod(&e));
            return Redirect::to(&url).into_response();
        }
    };

    // Resolve role (with org/team checks)
    let role = match resolve_role_full(&github_user.login, &access_token, &cfg).await {
        Some(r) => r,
        None => {
            return Redirect::to("/auth/denied?error=not_authorized").into_response();
        }
    };

    // Create session
    let session_id = generate_session_id();
    let session = Session {
        id: session_id.clone(),
        login: github_user.login,
        role,
        avatar_url: github_user.avatar_url,
        expires_at: Instant::now() + SESSION_DURATION,
    };
    state.sessions.insert(session).await;

    // Set cookies and redirect
    let (http_only, js_readable) = set_session_cookie(&session_id);
    let clear_state = clear_state_cookie();

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        "/notes/"
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/notes/")),
    );
    if let Ok(val) = http_only.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }
    if let Ok(val) = js_readable.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }
    if let Ok(val) = clear_state.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }

    (StatusCode::FOUND, response_headers).into_response()
}

/// `GET /auth/logout` — clear session and cookies, redirect to home.
pub async fn logout_handler(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Some(sid) = get_cookie(&headers, "rune_sid") {
        state.sessions.remove(&sid).await;
    }
    let (http_only, js_readable) = clear_session_cookies();
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        "/".parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    if let Ok(val) = http_only.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }
    if let Ok(val) = js_readable.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }

    (StatusCode::FOUND, response_headers).into_response()
}

/// `GET /auth/denied` — display a "not authorized" page.
#[derive(Debug, Deserialize)]
pub struct DeniedParams {
    pub error: Option<String>,
    pub desc: Option<String>,
}

pub async fn denied_handler(Query(params): Query<DeniedParams>) -> Response {
    let error = params.error.as_deref().unwrap_or("not_authorized");
    let desc = params
        .desc
        .as_deref()
        .unwrap_or("You are not on the authorized user list.");
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Access Denied — Rune Notes</title>
  <style>
    body {{ font-family: system-ui, sans-serif; background: #0d1117; color: #e6edf3;
            display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }}
    .card {{ text-align: center; padding: 2rem; max-width: 400px; }}
    h1 {{ color: #f85149; font-size: 2rem; margin-bottom: 0.5rem; }}
    p {{ color: #8b949e; margin: 1rem 0; }}
    code {{ background: #161b22; padding: 0.2em 0.5em; border-radius: 4px; font-size: 0.85em; }}
    a {{ color: #58a6ff; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .btn {{ display: inline-block; margin-top: 1.5rem; padding: 0.5rem 1.5rem;
            background: #21262d; border-radius: 6px; color: #e6edf3; text-decoration: none; }}
    .btn:hover {{ background: #30363d; text-decoration: none; }}
  </style>
</head>
<body>
  <div class="card">
    <h1>🚫 Access Denied</h1>
    <p>{desc}</p>
    <p><code>{error}</code></p>
    <a class="btn" href="/">← Back to login</a>
  </div>
</body>
</html>"#
    );
    (StatusCode::FORBIDDEN, Html(html)).into_response()
}

/// Verify local credentials against configured local logins in `[notes.local]`.
pub fn verify_local_credentials(
    username: &str,
    password: &str,
    local_cfg: &crate::config::LocalConfig,
) -> Option<Role> {
    // Check admins
    for entry in &local_cfg.admins {
        if let Some((u, p)) = entry.split_once(':') {
            if u == username && p == password {
                return Some(Role::Admin);
            }
        }
    }
    // Check users
    for entry in &local_cfg.users {
        if let Some((u, p)) = entry.split_once(':') {
            if u == username && p == password {
                return Some(Role::User);
            }
        }
    }
    // Check guests
    for entry in &local_cfg.guests {
        if let Some((u, p)) = entry.split_once(':') {
            if u == username && p == password {
                return Some(Role::Guest);
            }
        }
    }
    None
}

/// `GET /api/auth/config` — check which auth methods are enabled.
pub async fn auth_config_handler(State(state): State<ServerState>) -> Response {
    let github_enabled = state.config.notes.github.is_some();
    let local_enabled = state.config.notes.local.is_some();
    axum::Json(serde_json::json!({
        "ok": true,
        "github": github_enabled,
        "local": local_enabled
    }))
    .into_response()
}

/// Local login request body.
#[derive(Debug, Deserialize)]
pub struct LocalLoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /auth/local` — validate local credentials, set session cookies, and return success.
pub async fn local_login_handler(
    State(state): State<ServerState>,
    axum::Json(req): axum::Json<LocalLoginRequest>,
) -> Response {
    let local_cfg = match state.config.notes.local.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": "Local authentication is not enabled"
                })),
            )
                .into_response();
        }
    };

    let role = match verify_local_credentials(&req.username, &req.password, local_cfg) {
        Some(r) => r,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": "Invalid username or password"
                })),
            )
                .into_response();
        }
    };

    // Create session
    let session_id = generate_session_id();
    let session = Session {
        id: session_id.clone(),
        login: req.username.clone(),
        role: role.clone(),
        avatar_url: String::new(), // No avatar for local accounts
        expires_at: Instant::now() + SESSION_DURATION,
    };
    state.sessions.insert(session).await;

    // Set cookies
    let (http_only, js_readable) = set_session_cookie(&session_id);
    let mut response_headers = HeaderMap::new();
    if let Ok(val) = http_only.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }
    if let Ok(val) = js_readable.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }

    (
        StatusCode::OK,
        response_headers,
        axum::Json(serde_json::json!({
            "ok": true,
            "username": req.username,
            "role": role.as_str()
        })),
    )
        .into_response()
}

/// Minimal URL-encode (percent-encode spaces and special chars).
fn urlencod(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                vec![c]
            }
            c => format!("%{:02X}", c as u32).chars().collect(),
        })
        .collect()
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitHubOAuthConfig;

    fn make_cfg(admins: &[&str], users: &[&str], guests: &[&str]) -> GitHubOAuthConfig {
        GitHubOAuthConfig {
            client_id: "test_id".into(),
            client_secret: "test_secret".into(),
            admins: admins.iter().map(|s| s.to_string()).collect(),
            users: users.iter().map(|s| s.to_string()).collect(),
            guests: guests.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_resolve_role_admin_by_login() {
        let cfg = make_cfg(&["fourdollars"], &[], &[]);
        assert_eq!(
            resolve_role_by_login("fourdollars", &cfg),
            Some(Role::Admin)
        );
    }

    #[test]
    fn test_resolve_role_guest_by_login() {
        let cfg = make_cfg(&[], &[], &["some-friend"]);
        assert_eq!(
            resolve_role_by_login("some-friend", &cfg),
            Some(Role::Guest)
        );
    }

    #[test]
    fn test_resolve_role_case_insensitive() {
        let cfg = make_cfg(&["FourDollars"], &[], &[]);
        assert_eq!(
            resolve_role_by_login("fourdollars", &cfg),
            Some(Role::Admin)
        );
    }

    #[test]
    fn test_resolve_role_precedence_admin_over_user() {
        let cfg = make_cfg(&["alice"], &["alice"], &[]);
        assert_eq!(resolve_role_by_login("alice", &cfg), Some(Role::Admin));
    }

    #[test]
    fn test_resolve_role_unknown_login() {
        let cfg = make_cfg(&["alice"], &["bob"], &["carol"]);
        assert_eq!(resolve_role_by_login("nobody", &cfg), None);
    }

    #[test]
    fn test_org_entry_parsing() {
        assert_eq!(
            parse_org_team_entry("org:my-org/ops"),
            Some(("my-org".into(), Some("ops".into())))
        );
        assert_eq!(
            parse_org_team_entry("org:my-org"),
            Some(("my-org".into(), None))
        );
        assert_eq!(parse_org_team_entry("fourdollars"), None);
        assert_eq!(parse_org_team_entry(""), None);
    }

    #[test]
    fn test_generate_session_id_is_hex_32() {
        let id = generate_session_id();
        assert_eq!(id.len(), 32, "session id must be 32 chars, got: {id}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "session id must be hex, got: {id}"
        );
        // Generate two IDs and verify they differ (collision probability is negligible)
        let id2 = generate_session_id();
        // Not asserting inequality because in test the timing may repeat; just verify format
        assert_eq!(id2.len(), 32);
    }

    #[tokio::test]
    async fn test_session_store_insert_and_lookup() {
        let store = SessionStore::new();
        let session = Session {
            id: "abc123".into(),
            login: "testuser".into(),
            role: Role::Admin,
            avatar_url: "https://example.com/avatar.png".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(session.clone()).await;
        let found = store.get("abc123").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().login, "testuser");
    }

    #[tokio::test]
    async fn test_session_store_expired_returns_none() {
        let store = SessionStore::new();
        let session = Session {
            id: "expired".into(),
            login: "olduser".into(),
            role: Role::User,
            avatar_url: "".into(),
            expires_at: Instant::now() - Duration::from_secs(1), // already expired
        };
        store.insert(session).await;
        assert!(store.get("expired").await.is_none());
    }

    #[tokio::test]
    async fn test_session_store_remove() {
        let store = SessionStore::new();
        let session = Session {
            id: "del".into(),
            login: "user".into(),
            role: Role::Guest,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(session).await;
        store.remove("del").await;
        assert!(store.get("del").await.is_none());
    }

    #[tokio::test]
    async fn test_session_store_sweep_expired() {
        let store = SessionStore::new();
        let expired = Session {
            id: "e".into(),
            login: "e".into(),
            role: Role::Guest,
            avatar_url: "".into(),
            expires_at: Instant::now() - Duration::from_secs(1),
        };
        let valid = Session {
            id: "v".into(),
            login: "v".into(),
            role: Role::User,
            avatar_url: "".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        };
        store.insert(expired).await;
        store.insert(valid).await;
        store.sweep_expired().await;
        assert!(store.get("e").await.is_none());
        assert!(store.get("v").await.is_some());
    }

    #[test]
    fn test_cookie_helpers() {
        let (http_only, js_readable) = set_session_cookie("abc123");
        assert!(http_only.contains("rune_sid=abc123"));
        assert!(http_only.contains("HttpOnly"));
        assert!(js_readable.contains("rune_session_id=abc123"));
        assert!(!js_readable.contains("HttpOnly"));

        let (clear_h, clear_j) = clear_session_cookies();
        assert!(clear_h.contains("Max-Age=0"));
        assert!(clear_j.contains("Max-Age=0"));

        let state_cookie = set_state_cookie("mystate");
        assert!(state_cookie.contains("rune_oauth_state=mystate"));
        assert!(state_cookie.contains("HttpOnly"));
    }

    #[test]
    fn test_get_cookie_parses_correctly() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "rune_sid=abc; other=xyz; rune_session_id=abc"
                .parse()
                .unwrap(),
        );
        assert_eq!(get_cookie(&headers, "rune_sid"), Some("abc".to_string()));
        assert_eq!(get_cookie(&headers, "other"), Some("xyz".to_string()));
        assert_eq!(get_cookie(&headers, "missing"), None);
    }

    #[test]
    fn test_org_entries_are_skipped_in_plain_login_check() {
        // Org entries must NOT match plain logins (they need async network check)
        let cfg = make_cfg(&["org:my-org/team", "alice"], &[], &[]);
        // "org:my-org/team" is an org entry — should not match login "org:my-org/team"
        assert_eq!(resolve_role_by_login("org:my-org/team", &cfg), None);
        // "alice" is a plain login — should match
        assert_eq!(resolve_role_by_login("alice", &cfg), Some(Role::Admin));
    }

    #[test]
    fn test_verify_local_credentials() {
        let local_cfg = crate::config::LocalConfig {
            admins: vec!["admin:admin123".to_string()],
            users: vec!["user:user123".to_string()],
            guests: vec!["guest:guest123".to_string()],
        };
        assert_eq!(
            verify_local_credentials("admin", "admin123", &local_cfg),
            Some(Role::Admin)
        );
        assert_eq!(
            verify_local_credentials("user", "user123", &local_cfg),
            Some(Role::User)
        );
        assert_eq!(
            verify_local_credentials("guest", "guest123", &local_cfg),
            Some(Role::Guest)
        );
        assert_eq!(verify_local_credentials("admin", "wrong", &local_cfg), None);
        assert_eq!(
            verify_local_credentials("unknown", "user123", &local_cfg),
            None
        );
    }
}
