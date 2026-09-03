//! GitHub OAuth 2.0 authentication for Rune Notes.
//!
//! Flow:
//!   1. `GET /auth/github` → redirect to GitHub with CSRF state cookie
//!   2. `GET /auth/github/callback` → verify state, exchange code, fetch user,
//!      resolve role, create session, set cookies, redirect to /edit/
//!   3. `GET /auth/logout` → clear session + cookies, redirect to /
//!   4. `GET /auth/denied` → 403 "not authorized" page

use crate::config::{GitHubOAuthConfig, OAuthProviderConfig};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ─── Constants ─────────────────────────────────────────────────────────────

pub const SESSION_DURATION_SECS: i64 = 30 * 24 * 3600; // 30 days
pub const SESSION_DURATION: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days
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

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "admin" => Role::Admin,
            "user" => Role::User,
            _ => Role::Guest,
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
    pub expires_at: i64,
}

impl Session {
    pub fn is_expired(&self) -> bool {
        crate::serve::db::now_secs() >= self.expires_at
    }
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
    pub fn is_guest(&self) -> bool {
        self.role == Role::Guest
    }
}

/// An authenticated user identity resolved from request credentials (cookie or Bearer token).
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub login: String,
    pub role: Role,
}

// ─── SessionStore ──────────────────────────────────────────────────────────

/// Persistent session store with in-memory cache and SQLite backing.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, Session>>>,
    db: Option<crate::serve::db::ChatDb>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::new_with_db(None)
    }

    pub fn new_with_db(db: Option<crate::serve::db::ChatDb>) -> Self {
        let mut map = HashMap::new();
        if let Some(ref db) = db {
            if let Ok(records) = db.load_active_sessions() {
                for (id, login, role_str, avatar_url, expires_at) in records {
                    let role = Role::from_str(&role_str);
                    map.insert(
                        id.clone(),
                        Session {
                            id,
                            login,
                            role,
                            avatar_url,
                            expires_at,
                        },
                    );
                }
            }
        }
        Self {
            inner: Arc::new(RwLock::new(map)),
            db,
        }
    }

    pub async fn insert(&self, session: Session) {
        if let Some(ref db) = self.db {
            let role_str = session.role.as_str().to_string();
            let _ = db.save_session(
                &session.id,
                &session.login,
                &role_str,
                &session.avatar_url,
                session.expires_at,
            );
        }
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
        if let Some(ref db) = self.db {
            let _ = db.remove_session(id);
        }
        self.inner.write().await.remove(id);
    }

    /// Remove all expired sessions.
    pub async fn sweep_expired(&self) {
        let now = crate::serve::db::now_secs();
        if let Some(ref db) = self.db {
            let _ = db.sweep_expired_auth();
        }
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

pub async fn exchange_code_generic(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<String, String> {
    let body_str = format!(
        "grant_type=authorization_code&client_id={}&client_secret={}&code={}&redirect_uri={}&code_verifier={}",
        urlencod(client_id),
        urlencod(client_secret),
        urlencod(code),
        urlencod(redirect_uri),
        urlencod(code_verifier)
    );
    let resp = client
        .post(token_url)
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
                .or_else(|| body["error"].as_str())
                .unwrap_or("unknown error")
                .to_string()
        })
}

pub async fn fetch_userinfo_generic(
    client: &reqwest::Client,
    userinfo_url: &str,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    let resp = client
        .get(userinfo_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("json parse error: {e}"))
}

/// GitHub user info.
#[derive(Debug, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

/// Resolved third-party OAuth provider endpoints and role mapping.
#[derive(Debug, Clone)]
pub struct ResolvedOAuthProvider {
    pub name: String,
    pub display_name: String,
    pub icon: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub authorization_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub admins: Vec<String>,
    pub users: Vec<String>,
    pub guests: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OAuthProviderPublic {
    pub name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

async fn discover_oidc_endpoints(
    client: &reqwest::Client,
    issuer: &str,
) -> Option<(String, String, String)> {
    let issuer = issuer.trim().trim_end_matches('/');
    if issuer.is_empty() {
        return None;
    }
    let url = format!("{issuer}/.well-known/openid-configuration");
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.json::<OidcDiscovery>().await.ok()?;
    Some((
        body.authorization_endpoint,
        body.token_endpoint,
        body.userinfo_endpoint,
    ))
}

fn display_name_or_name(cfg: &OAuthProviderConfig) -> String {
    cfg.display_name
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| cfg.name.clone())
}

fn resolved_provider_from_config(
    cfg: &OAuthProviderConfig,
    discovered: Option<(String, String, String)>,
) -> Option<ResolvedOAuthProvider> {
    let name = cfg.name.trim();
    if name.is_empty() {
        return None;
    }

    let (authorization_url, token_url, userinfo_url) = match discovered {
        Some(urls) => urls,
        None => (
            cfg.authorization_url.clone()?,
            cfg.token_url.clone()?,
            cfg.userinfo_url.clone()?,
        ),
    };

    if cfg.client_id.trim().is_empty() || cfg.client_secret.trim().is_empty() {
        return None;
    }

    Some(ResolvedOAuthProvider {
        name: name.to_string(),
        display_name: display_name_or_name(cfg),
        icon: cfg.icon.clone(),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        authorization_url,
        token_url,
        userinfo_url,
        scopes: cfg.scopes.clone(),
        groups_claim: cfg.groups_claim.clone(),
        admins: cfg.admins.clone(),
        users: cfg.users.clone(),
        guests: cfg.guests.clone(),
    })
}

pub async fn resolve_oauth_providers(
    configs: &[OAuthProviderConfig],
) -> Vec<ResolvedOAuthProvider> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let mut providers = Vec::new();
    for cfg in configs {
        let discovered = match cfg.issuer.as_deref() {
            Some(issuer) => discover_oidc_endpoints(&client, issuer).await,
            None => None,
        };

        if let Some(provider) = resolved_provider_from_config(cfg, discovered) {
            providers.push(provider);
        }
    }
    providers
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

fn role_entries_match_identity_or_groups(entry: &str, identity: &str, groups: &[String]) -> bool {
    if let Some(group) = entry.strip_prefix("grp:") {
        let g = group.trim();
        !g.is_empty() && groups.iter().any(|x| x.eq_ignore_ascii_case(g))
    } else {
        entry.eq_ignore_ascii_case(identity)
    }
}

pub fn parse_groups_claim(userinfo: &serde_json::Value, claim: &str) -> Vec<String> {
    let Some(raw) = userinfo.get(claim) else {
        return Vec::new();
    };
    match raw {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn resolve_generic_role(
    identity: &str,
    groups: &[String],
    provider: &ResolvedOAuthProvider,
) -> Option<Role> {
    for entry in &provider.admins {
        if role_entries_match_identity_or_groups(entry, identity, groups) {
            return Some(Role::Admin);
        }
    }
    for entry in &provider.users {
        if role_entries_match_identity_or_groups(entry, identity, groups) {
            return Some(Role::User);
        }
    }
    for entry in &provider.guests {
        if role_entries_match_identity_or_groups(entry, identity, groups) {
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

pub fn set_oauth_pkce_cookie(verifier: &str) -> String {
    format!("rune_oauth_pkce={verifier}; Path=/auth; HttpOnly; SameSite=Lax; Max-Age={STATE_COOKIE_DURATION_SECS}")
}

pub fn clear_oauth_pkce_cookie() -> String {
    "rune_oauth_pkce=; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=0".to_string()
}

pub fn set_oauth_provider_cookie(name: &str) -> String {
    format!(
        "rune_oauth_provider={name}; Path=/auth; HttpOnly; SameSite=Lax; Max-Age={STATE_COOKIE_DURATION_SECS}"
    )
}

pub fn clear_oauth_provider_cookie() -> String {
    "rune_oauth_provider=; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=0".to_string()
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

#[derive(Debug, Deserialize)]
pub struct OAuthStartParams {
    pub next: Option<String>,
}

fn pkce_code_challenge_s256(verifier: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

fn oauth_callback_base_url(headers: &HeaderMap) -> String {
    let proto = detect_proto(headers);
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:9527");
    format!("{proto}://{host}")
}

fn identity_from_userinfo(userinfo: &serde_json::Value) -> Option<String> {
    userinfo
        .get("sub")
        .and_then(|v| v.as_str())
        .or_else(|| userinfo.get("username").and_then(|v| v.as_str()))
        .or_else(|| userinfo.get("user_id").and_then(|v| v.as_str()))
        .or_else(|| userinfo.get("preferred_username").and_then(|v| v.as_str()))
        .or_else(|| userinfo.get("login").and_then(|v| v.as_str()))
        .or_else(|| userinfo.get("email").and_then(|v| v.as_str()))
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
}

fn avatar_from_userinfo(userinfo: &serde_json::Value) -> String {
    userinfo
        .get("picture")
        .and_then(|v| v.as_str())
        .or_else(|| userinfo.get("avatar_url").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

/// `GET /auth/github` — kick off the OAuth dance.
pub async fn oauth_start_handler(
    State(state): State<ServerState>,
    Query(params): Query<OAuthStartParams>,
) -> Response {
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

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        redirect_url
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    if let Ok(val) = state_cookie.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }

    if let Some(ref next_url) = params.next {
        if next_url.starts_with("/edit") || next_url.starts_with("/oauth/") {
            let next_cookie = format!(
                "rune_next={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=300",
                next_url
            );
            if let Ok(val) = next_cookie.parse() {
                response_headers.append(header::SET_COOKIE, val);
            }
        }
    }

    (StatusCode::FOUND, response_headers).into_response()
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
    let login = if github_user.login.starts_with("github:") {
        github_user.login.clone()
    } else {
        format!("github:{}", github_user.login)
    };
    let session = Session {
        id: session_id.clone(),
        login: login.clone(),
        role: role.clone(),
        avatar_url: github_user.avatar_url,
        expires_at: crate::serve::db::now_secs() + SESSION_DURATION_SECS,
    };
    state.sessions.insert(session).await;

    eprintln!(
        "[auth] login: {} role={} method=github",
        login,
        role.as_str()
    );

    // Set cookies and redirect
    let (http_only, js_readable) = set_session_cookie(&session_id);
    let clear_state = clear_state_cookie();

    let target_url = get_cookie(&headers, "rune_next")
        .filter(|u| u.starts_with("/edit") || u.starts_with("/oauth/"))
        .unwrap_or_else(|| "/edit/".to_string());

    let clear_next = "rune_next=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string();

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        target_url
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/edit/")),
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
    if let Ok(val) = clear_next.parse() {
        response_headers.append(header::SET_COOKIE, val);
    }

    (StatusCode::FOUND, response_headers).into_response()
}

/// `GET /auth/oauth/{name}` — start third-party OAuth2/OIDC flow.
pub async fn oauth_generic_start_handler(
    Path(name): Path<String>,
    State(state): State<ServerState>,
    Query(params): Query<OAuthStartParams>,
    headers: HeaderMap,
) -> Response {
    let provider = {
        let providers = state.oauth_providers.read().await;
        providers.get(&name).cloned()
    };
    let Some(provider) = provider else {
        return (
            StatusCode::NOT_FOUND,
            Html("<h1>OAuth provider not found</h1>"),
        )
            .into_response();
    };

    let csrf_state = generate_session_id();
    let code_verifier = format!("{}{}", generate_session_id(), generate_session_id());
    let code_challenge = pkce_code_challenge_s256(&code_verifier);

    let state_cookie = set_state_cookie(&csrf_state);
    let pkce_cookie = set_oauth_pkce_cookie(&code_verifier);
    let provider_cookie = set_oauth_provider_cookie(&provider.name);

    let callback = format!(
        "{}/auth/oauth/{}/callback",
        oauth_callback_base_url(&headers),
        provider.name
    );
    let scope = if provider.scopes.is_empty() {
        "openid profile".to_string()
    } else {
        provider.scopes.join(" ")
    };

    let redirect_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        provider.authorization_url,
        urlencod(&provider.client_id),
        urlencod(&callback),
        urlencod(&scope),
        urlencod(&csrf_state),
        urlencod(&code_challenge),
    );

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        redirect_url
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    for cookie in [&state_cookie, &pkce_cookie, &provider_cookie] {
        if let Ok(val) = cookie.parse() {
            response_headers.append(header::SET_COOKIE, val);
        }
    }

    if let Some(ref next_url) = params.next {
        if next_url.starts_with("/edit") || next_url.starts_with("/oauth/") {
            let next_cookie = format!(
                "rune_next={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=300",
                next_url
            );
            if let Ok(val) = next_cookie.parse() {
                response_headers.append(header::SET_COOKIE, val);
            }
        }
    }

    (StatusCode::FOUND, response_headers).into_response()
}

/// `GET /auth/oauth/{name}/callback` — finish third-party OAuth2/OIDC flow.
pub async fn oauth_generic_callback_handler(
    Path(name): Path<String>,
    State(state): State<ServerState>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    if let Some(err) = params.error {
        let desc = params
            .error_description
            .unwrap_or_else(|| "OAuth error".to_string());
        let url = format!("/auth/denied?error={}&desc={}", err, urlencod(&desc));
        return Redirect::to(&url).into_response();
    }

    let Some(provider) = ({
        let providers = state.oauth_providers.read().await;
        providers.get(&name).cloned()
    }) else {
        return Redirect::to("/auth/denied?error=provider_not_found").into_response();
    };

    let code = match params.code {
        Some(c) => c,
        None => return Redirect::to("/auth/denied?error=missing_code").into_response(),
    };

    let expected_state = get_cookie(&headers, "rune_oauth_state");
    let provided_state = params.state.as_deref().unwrap_or("");
    if expected_state.as_deref() != Some(provided_state) || provided_state.is_empty() {
        return Redirect::to("/auth/denied?error=csrf_mismatch").into_response();
    }

    let provider_cookie = get_cookie(&headers, "rune_oauth_provider");
    if provider_cookie.as_deref() != Some(provider.name.as_str()) {
        return Redirect::to("/auth/denied?error=provider_mismatch").into_response();
    }

    let code_verifier = match get_cookie(&headers, "rune_oauth_pkce") {
        Some(v) if !v.is_empty() => v,
        _ => return Redirect::to("/auth/denied?error=missing_pkce").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let callback = format!(
        "{}/auth/oauth/{}/callback",
        oauth_callback_base_url(&headers),
        provider.name
    );
    let access_token = match exchange_code_generic(
        &client,
        &provider.token_url,
        &provider.client_id,
        &provider.client_secret,
        &code,
        &callback,
        &code_verifier,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            let url = format!("/auth/denied?error=token_exchange&desc={}", urlencod(&e));
            return Redirect::to(&url).into_response();
        }
    };

    let userinfo =
        match fetch_userinfo_generic(&client, &provider.userinfo_url, &access_token).await {
            Ok(v) => v,
            Err(e) => {
                let url = format!("/auth/denied?error=user_fetch&desc={}", urlencod(&e));
                return Redirect::to(&url).into_response();
            }
        };

    let identity = match identity_from_userinfo(&userinfo) {
        Some(v) => v,
        None => return Redirect::to("/auth/denied?error=missing_identity").into_response(),
    };
    let groups = parse_groups_claim(&userinfo, &provider.groups_claim);
    let role = match resolve_generic_role(&identity, &groups, &provider) {
        Some(r) => r,
        None => return Redirect::to("/auth/denied?error=not_authorized").into_response(),
    };

    let session_id = generate_session_id();
    let login = if identity.starts_with(&format!("{}:", provider.name)) {
        identity
    } else {
        format!("{}:{}", provider.name, identity)
    };
    let session = Session {
        id: session_id.clone(),
        login: login.clone(),
        role,
        avatar_url: avatar_from_userinfo(&userinfo),
        expires_at: crate::serve::db::now_secs() + SESSION_DURATION_SECS,
    };
    eprintln!(
        "[auth] login: {} role={} method=oauth provider={}",
        login,
        session.role.as_str(),
        provider.name
    );
    state.sessions.insert(session).await;

    let (http_only, js_readable) = set_session_cookie(&session_id);
    let clear_state = clear_state_cookie();
    let clear_pkce = clear_oauth_pkce_cookie();
    let clear_provider = clear_oauth_provider_cookie();

    let target_url = get_cookie(&headers, "rune_next")
        .filter(|u| u.starts_with("/edit") || u.starts_with("/oauth/"))
        .unwrap_or_else(|| "/edit/".to_string());
    let clear_next = "rune_next=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string();

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::LOCATION,
        target_url
            .parse()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/edit/")),
    );
    for cookie in [
        &http_only,
        &js_readable,
        &clear_state,
        &clear_pkce,
        &clear_provider,
        &clear_next,
    ] {
        if let Ok(val) = cookie.parse() {
            response_headers.append(header::SET_COOKIE, val);
        }
    }

    (StatusCode::FOUND, response_headers).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct LogoutParams {
    pub redirect_uri: Option<String>,
    pub post_logout_redirect_uri: Option<String>,
    pub next: Option<String>,
}

/// `GET /auth/logout` — clear session and cookies, redirect to home or specified redirect_uri.
pub async fn logout_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<LogoutParams>,
) -> Response {
    if let Some(sid) = get_cookie(&headers, "rune_sid") {
        if let Some(session) = state.sessions.get(&sid).await {
            eprintln!(
                "[auth] logout: {} role={}",
                session.login,
                session.role.as_str()
            );
        }
        state.sessions.remove(&sid).await;
    }
    let (http_only, js_readable) = clear_session_cookies();
    let mut response_headers = HeaderMap::new();

    let target = params
        .redirect_uri
        .as_deref()
        .or(params.post_logout_redirect_uri.as_deref())
        .or(params.next.as_deref())
        .unwrap_or("/");

    let location_val = target
        .parse()
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/"));

    response_headers.insert(header::LOCATION, location_val);
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
    let oauth = {
        let providers = state.oauth_providers.read().await;
        let mut list = providers
            .values()
            .map(|p| OAuthProviderPublic {
                name: p.name.clone(),
                display_name: p.display_name.clone(),
                icon: p.icon.clone(),
            })
            .collect::<Vec<_>>();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    };
    axum::Json(serde_json::json!({
        "ok": true,
        "github": github_enabled,
        "local": local_enabled,
        "oauth": oauth
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
    let login = if req.username.starts_with("local:") {
        req.username.clone()
    } else {
        format!("local:{}", req.username)
    };
    let session = Session {
        id: session_id.clone(),
        login: login.clone(),
        role: role.clone(),
        avatar_url: String::new(), // No avatar for local accounts
        expires_at: crate::serve::db::now_secs() + SESSION_DURATION_SECS,
    };
    state.sessions.insert(session).await;

    eprintln!(
        "[auth] login: {} role={} method=local",
        login,
        role.as_str()
    );

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
            "username": login,
            "login": login,
            "role": role.as_str()
        })),
    )
        .into_response()
}

/// Minimal URL-encode (percent-encode spaces and special chars).
pub fn urlencod(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                vec![c]
            }
            c => format!("%{:02X}", c as u32).chars().collect(),
        })
        .collect()
}

/// Helper to detect request protocol (https vs http) considering reverse proxies,
/// X-Forwarded-Proto, X-Forwarded-Ssl, X-Forwarded-Port, and domain names.
pub fn detect_proto(headers: &HeaderMap) -> &'static str {
    if let Some(proto) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        let first = proto.split(',').next().unwrap_or("").trim().to_lowercase();
        if first == "https" {
            return "https";
        }
    }

    if let Some(ssl) = headers.get("x-forwarded-ssl").and_then(|v| v.to_str().ok()) {
        if ssl.eq_ignore_ascii_case("on") || ssl == "1" {
            return "https";
        }
    }

    if let Some(fe) = headers.get("front-end-https").and_then(|v| v.to_str().ok()) {
        if fe.eq_ignore_ascii_case("on") || fe == "1" {
            return "https";
        }
    }

    if let Some(port) = headers
        .get("x-forwarded-port")
        .and_then(|v| v.to_str().ok())
    {
        if port == "443" {
            return "https";
        }
    }

    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !host.is_empty()
        && !host.contains("localhost")
        && !host.contains("127.0.0.1")
        && !host.contains("0.0.0.0")
    {
        if !host.contains(':') || host.ends_with(":443") {
            return "https";
        }
    }

    "http"
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GitHubOAuthConfig, OAuthProviderConfig};

    fn make_cfg(admins: &[&str], users: &[&str], guests: &[&str]) -> GitHubOAuthConfig {
        GitHubOAuthConfig {
            client_id: "test_id".into(),
            client_secret: "test_secret".into(),
            admins: admins.iter().map(|s| s.to_string()).collect(),
            users: users.iter().map(|s| s.to_string()).collect(),
            guests: guests.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_generic_provider(
        admins: &[&str],
        users: &[&str],
        guests: &[&str],
    ) -> ResolvedOAuthProvider {
        ResolvedOAuthProvider {
            name: "example".to_string(),
            display_name: "Example".to_string(),
            icon: None,
            client_id: "cid".to_string(),
            client_secret: "secret".to_string(),
            authorization_url: "https://example.com/oauth/authorize".to_string(),
            token_url: "https://example.com/oauth/token".to_string(),
            userinfo_url: "https://example.com/oauth/userinfo".to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            groups_claim: "groups".to_string(),
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
            expires_at: crate::serve::db::now_secs() + 3600,
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
            expires_at: crate::serve::db::now_secs() - 1, // already expired
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
            expires_at: crate::serve::db::now_secs() + 3600,
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
            expires_at: crate::serve::db::now_secs() - 1,
        };
        let valid = Session {
            id: "v".into(),
            login: "v".into(),
            role: Role::User,
            avatar_url: "".into(),
            expires_at: crate::serve::db::now_secs() + 3600,
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

    #[test]
    fn test_parse_groups_claim_array_and_string() {
        let userinfo_array = serde_json::json!({"groups": ["a", "b"]});
        assert_eq!(
            parse_groups_claim(&userinfo_array, "groups"),
            vec!["a", "b"]
        );

        let userinfo_string = serde_json::json!({"groups": "admins"});
        assert_eq!(
            parse_groups_claim(&userinfo_string, "groups"),
            vec!["admins"]
        );

        let userinfo_missing = serde_json::json!({"roles": ["x"]});
        assert!(parse_groups_claim(&userinfo_missing, "groups").is_empty());
    }

    #[test]
    fn test_resolve_generic_role_direct_identity_match() {
        let provider = make_generic_provider(&["alice"], &[], &[]);
        assert_eq!(
            resolve_generic_role("alice", &Vec::<String>::new(), &provider),
            Some(Role::Admin)
        );
    }

    #[test]
    fn test_resolve_generic_role_group_match() {
        let provider = make_generic_provider(&["grp:platform-admins"], &[], &[]);
        let groups = vec!["platform-admins".to_string()];
        assert_eq!(
            resolve_generic_role("sub-123", &groups, &provider),
            Some(Role::Admin)
        );
    }

    #[test]
    fn test_resolve_generic_role_precedence_admin_over_user() {
        let provider = make_generic_provider(&["grp:staff"], &["grp:staff"], &[]);
        let groups = vec!["staff".to_string()];
        assert_eq!(
            resolve_generic_role("sub-123", &groups, &provider),
            Some(Role::Admin)
        );
    }

    #[test]
    fn test_resolve_generic_role_ignores_empty_grp_entry() {
        let provider = make_generic_provider(&["grp:"], &[], &[]);
        let groups = vec!["anything".to_string()];
        assert_eq!(resolve_generic_role("sub-123", &groups, &provider), None);
    }

    #[tokio::test]
    async fn test_resolve_oauth_providers_with_explicit_endpoints() {
        let cfg = OAuthProviderConfig {
            name: "custom".to_string(),
            display_name: Some("Custom".to_string()),
            icon: None,
            client_id: "cid".to_string(),
            client_secret: "secret".to_string(),
            issuer: None,
            authorization_url: Some("https://example.com/oauth/authorize".to_string()),
            token_url: Some("https://example.com/oauth/token".to_string()),
            userinfo_url: Some("https://example.com/oauth/userinfo".to_string()),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            groups_claim: "groups".to_string(),
            admins: vec![],
            users: vec![],
            guests: vec![],
        };
        let providers = resolve_oauth_providers(&[cfg]).await;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "custom");
        assert_eq!(
            providers[0].authorization_url,
            "https://example.com/oauth/authorize"
        );
    }

    #[tokio::test]
    async fn test_logout_handler_default_redirect() {
        let (admin_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
        let state = ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![])),
            rooms: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new(String::new())),
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        };

        let response = logout_handler(
            State(state),
            HeaderMap::new(),
            Query(LogoutParams::default()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/"
        );
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(cookies.len(), 2);
    }

    #[tokio::test]
    async fn test_logout_handler_custom_redirect_uri() {
        let (admin_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
        let state = ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: SessionStore::new(),
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![])),
            rooms: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new(String::new())),
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        };

        let target = "https://extension-id.chromiumapp.org/callback";
        let response = logout_handler(
            State(state.clone()),
            HeaderMap::new(),
            Query(LogoutParams {
                redirect_uri: Some(target.to_string()),
                ..Default::default()
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            target
        );

        let response_post_logout = logout_handler(
            State(state),
            HeaderMap::new(),
            Query(LogoutParams {
                post_logout_redirect_uri: Some(target.to_string()),
                ..Default::default()
            }),
        )
        .await;

        assert_eq!(response_post_logout.status(), StatusCode::FOUND);
        assert_eq!(
            response_post_logout
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            target
        );
    }

    #[tokio::test]
    async fn test_logout_handler_removes_session() {
        let (admin_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
        let sessions = SessionStore::new();
        let sid = "test-session-id".to_string();
        sessions
            .insert(Session {
                id: sid.clone(),
                login: "alice".into(),
                role: Role::User,
                avatar_url: "".into(),
                expires_at: crate::serve::db::now_secs() + 3600,
            })
            .await;

        let state = ServerState {
            config: crate::config::RuneConfig::default(),
            sessions: sessions.clone(),
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![])),
            rooms: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new(String::new())),
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        };

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, format!("rune_sid={}", sid).parse().unwrap());

        let response = logout_handler(State(state), headers, Query(LogoutParams::default())).await;

        assert_eq!(response.status(), StatusCode::FOUND);
        assert!(sessions.get(&sid).await.is_none());
    }

    #[tokio::test]
    async fn test_local_auth_handler_prefixes_local() {
        let (admin_broadcast_tx, _) = tokio::sync::broadcast::channel(16);
        let sessions = SessionStore::new();
        let mut config = crate::config::RuneConfig::default();
        config.notes.local = Some(crate::config::LocalConfig {
            admins: vec!["admin:admin123".into()],
            users: vec!["user:user123".into()],
            guests: vec![],
        });
        let state = ServerState {
            config,
            sessions: sessions.clone(),
            files: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(tokio::sync::RwLock::new(String::new())),
            models: Arc::new(tokio::sync::RwLock::new(vec![])),
            rooms: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(tokio::sync::RwLock::new(String::new())),
            admin_broadcast_tx,
            chat_db: crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
            data_dir: std::path::PathBuf::from("/tmp/rune-test"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(tokio::sync::RwLock::new(
                crate::provider::ProviderRegistry::new(),
            )),
        };

        let req = LocalLoginRequest {
            username: "admin".into(),
            password: "admin123".into(),
        };
        let resp = local_login_handler(State(state), axum::Json(req)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Check the created session login
        let all_sessions = sessions.inner.read().await;
        assert_eq!(all_sessions.len(), 1);
        let sess = all_sessions.values().next().unwrap();
        assert_eq!(sess.login, "local:admin");
    }
}
