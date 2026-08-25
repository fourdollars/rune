use crate::serve::oauth::{get_cookie, Role};
use crate::serve::ServerState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    Form, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Verification of PKCE challenge
pub fn verify_pkce(code_verifier: &str, code_challenge: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    let computed_challenge = URL_SAFE_NO_PAD.encode(hash);
    computed_challenge == code_challenge
}

pub struct AuthCode {
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub role: Role,
    pub expires_at: std::time::Instant,
}

#[derive(Clone)]
pub struct AuthCodeStore(Arc<RwLock<HashMap<String, AuthCode>>>);

impl AuthCodeStore {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }

    pub async fn insert(&self, code: AuthCode) {
        self.0.write().await.insert(code.code.clone(), code);
    }

    pub async fn get(&self, code: &str) -> Option<AuthCode> {
        let mut store = self.0.write().await;
        if let Some(c) = store.get(code) {
            if std::time::Instant::now() > c.expires_at {
                store.remove(code);
                None
            } else {
                store.remove(code) // single use
            }
        } else {
            None
        }
    }

    pub async fn sweep_expired(&self) {
        let mut store = self.0.write().await;
        let now = std::time::Instant::now();
        store.retain(|_, v| v.expires_at > now);
    }
}

pub struct OAuthAccessToken {
    pub token: String,
    pub role: Role,
    pub expires_at: i64,
}

#[derive(Clone)]
pub struct OAuthTokenStore {
    inner: Arc<RwLock<HashMap<String, OAuthAccessToken>>>,
    db: Option<crate::serve::db::ChatDb>,
}

impl OAuthTokenStore {
    pub fn new() -> Self {
        Self::new_with_db(None)
    }

    pub fn new_with_db(db: Option<crate::serve::db::ChatDb>) -> Self {
        let mut map = HashMap::new();
        if let Some(ref db) = db {
            if let Ok(records) = db.load_active_oauth_tokens() {
                for (token, role_str, expires_at) in records {
                    let role = Role::from_str(&role_str);
                    map.insert(
                        token.clone(),
                        OAuthAccessToken {
                            token,
                            role,
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

    pub async fn insert(&self, token: OAuthAccessToken) {
        if let Some(ref db) = self.db {
            let role_str = token.role.as_str().to_string();
            let _ = db.save_oauth_token(&token.token, &role_str, token.expires_at);
        }
        self.inner.write().await.insert(token.token.clone(), token);
    }

    pub async fn get(&self, token: &str) -> Option<Role> {
        let mut store = self.inner.write().await;
        if let Some(t) = store.get(token) {
            let now = crate::serve::db::now_secs();
            if now > t.expires_at {
                if let Some(ref db) = self.db {
                    let _ = db.remove_oauth_token(token);
                }
                store.remove(token);
                None
            } else {
                Some(t.role.clone())
            }
        } else {
            None
        }
    }

    /// Revoke (delete) an access token immediately, e.g. on user-initiated
    /// logout from an OAuth client such as the browser extension. Returns
    /// true if a token was actually present and removed.
    pub async fn remove(&self, token: &str) -> bool {
        if let Some(ref db) = self.db {
            let _ = db.remove_oauth_token(token);
        }
        self.inner.write().await.remove(token).is_some()
    }

    pub async fn sweep_expired(&self) {
        let now = crate::serve::db::now_secs();
        if let Some(ref db) = self.db {
            let _ = db.sweep_expired_auth();
        }
        self.inner.write().await.retain(|_, v| v.expires_at > now);
    }
}

impl Default for OAuthTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    pub response_type: String,
    pub client_id: Option<String>,
    pub redirect_uri: String,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
}

pub async fn oauth_authorize_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AuthorizeQuery>,
) -> impl IntoResponse {
    let state_param = query.state.clone().unwrap_or_default();

    let error_redirect = |err: &str, desc: &str| {
        let url = format!(
            "{}?error={}&error_description={}&state={}",
            query.redirect_uri,
            err,
            crate::serve::oauth::urlencod(desc),
            crate::serve::oauth::urlencod(&state_param)
        );
        Redirect::to(&url).into_response()
    };

    if query.response_type != "code" {
        return error_redirect(
            "unsupported_response_type",
            "Only code response type is supported",
        );
    }

    let ccm = query.code_challenge_method.as_deref().unwrap_or("S256");
    if ccm != "S256" && ccm != "plain" {
        return error_redirect("invalid_request", "Unsupported code_challenge_method");
    }

    let sid = get_cookie(&headers, "rune_sid");
    let session = match sid {
        Some(ref id) => state.sessions.get(id).await,
        None => None,
    };

    if let Some(sess) = session {
        let code = crate::serve::oauth::generate_session_id();
        let auth_code = AuthCode {
            code: code.clone(),
            client_id: query.client_id.clone().unwrap_or_default(),
            redirect_uri: query.redirect_uri.clone(),
            code_challenge: query.code_challenge.clone().unwrap_or_default(),
            role: sess.role.clone(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
        };
        state.oauth_codes.insert(auth_code).await;

        let redirect_url = format!(
            "{}?code={}&state={}",
            query.redirect_uri,
            code,
            crate::serve::oauth::urlencod(&state_param)
        );
        Redirect::to(&redirect_url).into_response()
    } else {
        // Not logged in -> Redirect to login page / with `next` parameter preserving the authorize URL
        let mut authorize_uri = format!(
            "/oauth/authorize?response_type={}&redirect_uri={}",
            crate::serve::oauth::urlencod(&query.response_type),
            crate::serve::oauth::urlencod(&query.redirect_uri)
        );
        if let Some(ref cid) = query.client_id {
            authorize_uri.push_str(&format!(
                "&client_id={}",
                crate::serve::oauth::urlencod(cid)
            ));
        }
        if let Some(ref cc) = query.code_challenge {
            authorize_uri.push_str(&format!(
                "&code_challenge={}",
                crate::serve::oauth::urlencod(cc)
            ));
        }
        if let Some(ref ccm_val) = query.code_challenge_method {
            authorize_uri.push_str(&format!(
                "&code_challenge_method={}",
                crate::serve::oauth::urlencod(ccm_val)
            ));
        }
        if let Some(ref st) = query.state {
            authorize_uri.push_str(&format!("&state={}", crate::serve::oauth::urlencod(st)));
        }
        if let Some(ref sc) = query.scope {
            authorize_uri.push_str(&format!("&scope={}", crate::serve::oauth::urlencod(sc)));
        }

        let login_url = format!("/?next={}", crate::serve::oauth::urlencod(&authorize_uri));
        Redirect::to(&login_url).into_response()
    }
}

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Serialize)]
pub struct TokenError {
    pub error: String,
    pub error_description: String,
}

pub async fn oauth_token_handler(
    State(state): State<ServerState>,
    Form(req): Form<TokenRequest>,
) -> impl IntoResponse {
    let err = |error: &str, desc: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(TokenError {
                error: error.to_string(),
                error_description: desc.to_string(),
            }),
        )
            .into_response()
    };

    if req.grant_type != "authorization_code" {
        return err(
            "unsupported_grant_type",
            "Only authorization_code grant type is supported",
        );
    }

    let auth_code = match state.oauth_codes.get(&req.code).await {
        Some(c) => c,
        None => return err("invalid_grant", "Invalid or expired authorization code"),
    };

    // client_id is not validated — no pre-registration required.

    if let Some(ref req_uri) = req.redirect_uri {
        if !auth_code.redirect_uri.is_empty() && &auth_code.redirect_uri != req_uri {
            return err("invalid_grant", "Redirect URI mismatch");
        }
    }

    let verifier = req.code_verifier.as_deref().unwrap_or_default();
    if !auth_code.code_challenge.is_empty() && !verify_pkce(verifier, &auth_code.code_challenge) {
        return err("invalid_grant", "PKCE verification failed");
    }

    let token = crate::serve::oauth::generate_session_id();
    let expires_in: u64 = 30 * 24 * 3600; // 30 days
    let expires_at = crate::serve::db::now_secs() + expires_in as i64;
    let access_token = OAuthAccessToken {
        token: token.clone(),
        role: auth_code.role,
        expires_at,
    };

    state.oauth_tokens.insert(access_token).await;

    Json(TokenResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        expires_in,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct RevokeRequest {
    pub token: String,
}

/// `POST /oauth/revoke` — RFC 7009 style token revocation. Always returns
/// 200 regardless of whether the token existed (per spec, to avoid leaking
/// whether a given token string is/was valid). Used by OAuth clients such as
/// the browser extension to implement an explicit "Logout" action, since
/// otherwise a leaked/forgotten access_token would remain valid for its full
/// (currently 30-day) lifetime with no way to invalidate it server-side.
pub async fn oauth_revoke_handler(
    State(state): State<ServerState>,
    Form(req): Form<RevokeRequest>,
) -> impl IntoResponse {
    let removed = state.oauth_tokens.remove(&req.token).await;
    if removed {
        info!("[oauth] token revoked");
    }
    StatusCode::OK
}


pub async fn oauth_metadata_handler(headers: HeaderMap) -> impl IntoResponse {
    let proto = crate::serve::oauth::detect_proto(&headers);
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:9527");

    let base_url = format!("{}://{}", proto, host);

    let metadata = serde_json::json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{}/oauth/authorize", base_url),
        "token_endpoint": format!("{}/oauth/token", base_url),
        "registration_endpoint": format!("{}/oauth/register", base_url),
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "response_types_supported": ["code"],
        "token_endpoint_auth_methods_supported": ["none"],
    });

    Json(metadata).into_response()
}

/// `GET /.well-known/oauth-protected-resource` — RFC 9728 protected resource metadata.
/// Tells OAuth clients which authorization server protects this resource.
pub async fn oauth_protected_resource_handler(headers: HeaderMap) -> impl IntoResponse {
    let proto = crate::serve::oauth::detect_proto(&headers);
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:9527");

    let base_url = format!("{}://{}", proto, host);

    let metadata = serde_json::json!({
        "resource": format!("{}/mcp", base_url),
        "authorization_servers": [base_url],
        "bearer_methods_supported": ["header"],
    });

    Json(metadata).into_response()
}

/// `POST /oauth/register` — RFC 7591 Dynamic Client Registration.
/// Accepts any registration without validation (open-client model: security
/// comes from PKCE + user session, not client identity).
#[derive(Deserialize, Default)]
pub struct RegistrationRequest {
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    // Accept but ignore any extra fields from the client
    #[serde(default)]
    pub client_id: Option<String>,
}

pub async fn oauth_register_handler(Json(req): Json<RegistrationRequest>) -> impl IntoResponse {
    // Issue a client_id (reuse the one the client sent, or generate a new one)
    let client_id = req
        .client_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::serve::oauth::generate_session_id());

    let redirect_uris = if req.redirect_uris.is_empty() {
        serde_json::Value::Array(vec![])
    } else {
        serde_json::Value::Array(
            req.redirect_uris
                .iter()
                .map(|u| serde_json::Value::String(u.clone()))
                .collect(),
        )
    };

    let response = serde_json::json!({
        "client_id": client_id,
        "client_name": req.client_name.unwrap_or_else(|| "MCP Client".to_string()),
        "redirect_uris": redirect_uris,
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "code_challenge_method": "S256",
    });

    (StatusCode::CREATED, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_pkce() {
        // verifier: "test_verifier_123"
        let verifier = "test_verifier_123";
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        assert!(verify_pkce(verifier, &challenge));
        assert!(!verify_pkce("wrong_verifier", &challenge));
        assert!(!verify_pkce(verifier, "wrong_challenge"));
    }

    #[tokio::test]
    async fn test_auth_code_store() {
        let store = AuthCodeStore::new();
        let code = AuthCode {
            code: "test_code".to_string(),
            client_id: "test_client".to_string(),
            redirect_uri: "http://localhost".to_string(),
            code_challenge: "challenge".to_string(),
            role: Role::Admin,
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
        };

        store.insert(code).await;

        let retrieved = store.get("test_code").await;
        assert!(retrieved.is_some());

        // Single use check
        let retrieved_again = store.get("test_code").await;
        assert!(retrieved_again.is_none());
    }

    #[tokio::test]
    async fn test_oauth_token_store() {
        let store = OAuthTokenStore::new();
        let token = OAuthAccessToken {
            token: "test_token".to_string(),
            role: Role::Admin,
            expires_at: crate::serve::db::now_secs() + 3600,
        };

        store.insert(token).await;

        let role = store.get("test_token").await;
        assert!(matches!(role, Some(Role::Admin)));

        // Token persists
        let role_again = store.get("test_token").await;
        assert!(matches!(role_again, Some(Role::Admin)));
    }

    #[tokio::test]
    async fn test_oauth_token_store_persistence_across_restarts() {
        let db = crate::serve::db::ChatDb::open(std::path::Path::new(":memory:")).unwrap();

        // 1. Create first store with DB, insert token & session
        let store1 = OAuthTokenStore::new_with_db(Some(db.clone()));
        let session_store1 = crate::serve::oauth::SessionStore::new_with_db(Some(db.clone()));

        let token = OAuthAccessToken {
            token: "restart_token_123".to_string(),
            role: Role::Admin,
            expires_at: crate::serve::db::now_secs() + 2592000,
        };
        store1.insert(token).await;

        let session = crate::serve::oauth::Session {
            id: "restart_session_456".to_string(),
            login: "alice".to_string(),
            role: Role::User,
            avatar_url: "https://example.com/avatar".to_string(),
            expires_at: crate::serve::db::now_secs() + 2592000,
        };
        session_store1.insert(session).await;

        // 2. Simulate server restart: create new store from SAME DB connection
        let store2 = OAuthTokenStore::new_with_db(Some(db.clone()));
        let session_store2 = crate::serve::oauth::SessionStore::new_with_db(Some(db.clone()));

        // 3. Verify token and session were restored from DB
        let role = store2.get("restart_token_123").await;
        assert_eq!(role, Some(Role::Admin));

        let restored_session = session_store2.get("restart_session_456").await;
        assert!(restored_session.is_some());
        assert_eq!(restored_session.unwrap().login, "alice");
    }
}
