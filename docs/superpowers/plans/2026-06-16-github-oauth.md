# GitHub OAuth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the static `admin_token` / `user_token` / `guest_token` auth in Rune Notes with GitHub OAuth 2.0, using allowlist-based role mapping and in-memory HttpOnly-cookie sessions.

**Architecture:** The server exchanges a GitHub OAuth code for a user identity, resolves a role (admin/user/guest) by matching the GitHub login against configured allowlists (supporting plain logins and `"org:org/team"` refs), and issues a cryptographically random session ID stored in an HttpOnly cookie. A second JS-readable cookie `rune_session_id` (same value) lets the frontend detect login state. Custom session-cookie auth replaces the old `auth_middleware` token checking. All old token fields are removed entirely.

**Tech Stack:** Rust / Axum 0.8, reqwest (already a dep), sha2 + hex (already a dep), tokio, in-memory HashMap sessions, plain JS frontend (no new npm deps).

---

## Task 1: Config — Remove Token Fields, Add GitHubOAuthConfig

**Files:**
- Modify: `src/config/mod.rs`

Replace the `NotesConfig` struct (around line 84-98) by removing `user_token`, `admin_token`, `guest_token` fields and adding `github_oauth: Option<GitHubOAuthConfig>`. Add the `GitHubOAuthConfig` struct. Fix any tests in `src/config/mod.rs` that reference the removed fields. Do NOT touch other source files yet.

The new structs:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotesConfig {
    pub port: Option<u16>,
    pub bind: Option<String>,
    pub model: Option<String>,
    pub github_oauth: Option<GitHubOAuthConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GitHubOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub admins: Vec<String>,
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub guests: Vec<String>,
}
```

Add tests:
```rust
#[test]
fn test_github_oauth_config_parses() {
    let toml_str = r#"
[notes.github_oauth]
client_id = "Ov23liABC"
client_secret = "secret123"
admins = ["fourdollars", "org:my-org/ops"]
users = ["org:my-org"]
guests = ["some-friend"]
"#;
    let cfg: crate::config::RuneConfig = toml::from_str(toml_str).unwrap();
    let oauth = cfg.notes.github_oauth.expect("github_oauth must be present");
    assert_eq!(oauth.client_id, "Ov23liABC");
    assert_eq!(oauth.admins, vec!["fourdollars", "org:my-org/ops"]);
    assert_eq!(oauth.users, vec!["org:my-org"]);
    assert_eq!(oauth.guests, vec!["some-friend"]);
}
```

Run: `cargo test -p rune --lib config 2>&1 | tail -20`
Commit: `git commit -m "feat(config): replace token auth fields with GitHubOAuthConfig"`

---

## Task 2: OAuth Module — Sessions, Role Resolution, GitHub API, Handlers

**Files:**
- Create: `src/serve/oauth.rs`

Create the full OAuth module with:

1. `Role` enum: `Admin`, `User`, `Guest`
2. `Session` struct: `{ id, login, role, avatar_url, expires_at: Instant }`
3. `SessionStore`: `Arc<RwLock<HashMap<String, Session>>>` with `insert/get/remove/sweep_expired` methods; `get` returns `None` for expired sessions
4. Constants: `SESSION_DURATION = 24h`, `STATE_COOKIE_DURATION = 5min`
5. `parse_org_team_entry(entry: &str) -> Option<(String, Option<String>)>` — parses `"org:my-org/team"` format
6. `resolve_role_by_login(login: &str, cfg: &GitHubOAuthConfig) -> Option<Role>` — case-insensitive match against plain-login entries; admin > user > guest precedence
7. `generate_session_id() -> String` — 32-char lowercase hex using sha2
8. `exchange_code(client, client_id, client_secret, code) -> Result<String, String>` — POST to GitHub token endpoint
9. `fetch_github_user(client, access_token) -> Result<GitHubUser, String>` — GET /user
10. `check_github_membership(client, access_token, login, org, team) -> bool` — checks org/team membership
11. `resolve_role_full(login, access_token, cfg) -> Option<Role>` — login check first, then async org/team checks
12. Cookie helpers: `set_session_cookie`, `set_state_cookie`, `clear_session_cookie`, `clear_state_cookie`, `get_cookie`
13. `oauth_start_handler` — generates CSRF state, sets state cookie, redirects to GitHub
14. `oauth_callback_handler` — verifies state, exchanges code, fetches user, resolves role, creates session, sets `rune_sid` (HttpOnly) AND `rune_session_id` (JS-readable, not HttpOnly) cookies, redirects to /notes/ or /auth/denied
15. `logout_handler` — removes session, clears cookies, redirects to /
16. `denied_handler` — serves a 403 HTML page

The two cookies set on successful login:
```rust
// HttpOnly — used for actual auth
format!("rune_sid={session_id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}", SESSION_DURATION.as_secs())
// JS-readable — lets frontend detect login state
format!("rune_session_id={session_id}; Path=/; SameSite=Lax; Max-Age={}", SESSION_DURATION.as_secs())
```

Tests to include (all in `#[cfg(test)] mod tests`):
- `test_resolve_role_admin_by_login`
- `test_resolve_role_guest_by_login`
- `test_resolve_role_case_insensitive`
- `test_resolve_role_precedence_admin_over_user`
- `test_org_entry_parsing`
- `test_generate_session_id_is_hex_32`
- `test_session_store_insert_and_lookup` (tokio::test)
- `test_session_store_expired_returns_none` (tokio::test)
- `test_session_store_remove` (tokio::test)
- `test_cookie_helpers`

Run: `cargo test -p rune --lib serve::oauth 2>&1 | tail -20`
Commit: `git commit -m "feat(oauth): GitHub OAuth session management and role resolution"`

---

## Task 3: ServerState — Add Sessions, Remove Token Fields, Register Routes

**Files:**
- Modify: `src/serve/mod.rs`

Changes:
1. Add `pub mod oauth;` declaration at the top
2. Remove `user_token`, `admin_token`, `guest_token` from `ServerState` struct; add `pub sessions: crate::serve::oauth::SessionStore`
3. Remove token fields from `NotesOptions`; only keep `port` and `bind`
4. In `run()`: replace the token guard with `if config.notes.github_oauth.is_none() { eprintln!(...); return; }`
5. Update `ServerState { ... }` construction: remove token fields, add `sessions: crate::serve::oauth::SessionStore::new()`
6. Replace startup token print with `println!("  🔐 GitHub OAuth configured")`
7. Add session sweep background task (every 5 minutes)
8. Register OAuth routes in the Router: `/auth/github`, `/auth/github/callback`, `/auth/logout`, `/auth/denied`
9. Register `/api/me` route (GET)
10. Replace the body of `auth_middleware` to use session cookie:
    - Read `rune_sid` cookie from request headers using `oauth::get_cookie`
    - Look up session in `state.sessions`
    - If no valid session → 401
    - If guest + mutation → 403 (same allowed_guest_paths logic as before)
    - If non-admin + admin-only path → 403 (same admin_only_paths logic as before)

After changes, run `cargo check 2>&1 | grep "^error"` to see what still needs fixing in other files (will be api.rs and main.rs — handled in Tasks 4 and 5).
Commit: `git commit -m "feat(serve): add SessionStore to ServerState, OAuth routes, session middleware"`

---

## Task 4: API — Update auth, Add me_handler, Rewrite Tests

**Files:**
- Modify: `src/serve/api.rs`

Changes:
1. Delete `check_token`, `check_admin`, `check_guest` functions (lines ~359-380)
2. Update `EventsQuery` — remove `token: Option<String>` field
3. Update `SseMsg::AuthResult` — add `login: String` field
4. Update `events_handler`:
   - Add `headers: axum::http::HeaderMap` parameter
   - Replace token-based auth with session cookie lookup (`oauth::get_cookie(&headers, "rune_sid")` → `state.sessions.get(id)`)
   - Update `SseMsg::AuthResult { ok: true, is_admin, is_guest, login: session.login.clone() }`
   - Remove `token` usage from `EventsQuery` params
5. Add `me_handler`:
   ```rust
   pub async fn me_handler(
       State(state): State<ServerState>,
       headers: axum::http::HeaderMap,
   ) -> impl IntoResponse {
       use crate::serve::oauth::{get_cookie, Role};
       let sid = get_cookie(&headers, "rune_sid");
       let session = match sid {
           Some(ref id) => state.sessions.get(id).await,
           None => None,
       };
       match session {
           Some(s) => Json(serde_json::json!({
               "ok": true,
               "login": s.login,
               "role": s.role.as_str(),
               "avatar_url": s.avatar_url,
           })).into_response(),
           None => (StatusCode::UNAUTHORIZED,
               Json(serde_json::json!({"ok": false, "error": "Not authenticated"}))).into_response(),
       }
   }
   ```
6. In the test module, replace ALL old token-auth tests and `mock_state`/`test_state` helpers with session-based tests. Remove every reference to `user_token`, `admin_token`, `guest_token`. Add `mock_state_with_session(role: Role) -> ServerState` helper. Keep all non-auth tests unchanged.

The `mock_state_with_session` helper:
```rust
fn mock_state_with_session(tmp_path: std::path::PathBuf) -> ServerState {
    let (admin_tx, _) = tokio::sync::broadcast::channel(8);
    ServerState {
        config: crate::config::RuneConfig::default(),
        sessions: crate::serve::oauth::SessionStore::new(),
        files: std::sync::Arc::new(tokio::sync::RwLock::new(Default::default())),
        active_file: std::sync::Arc::new(tokio::sync::RwLock::new(String::new())),
        models: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        rooms: std::sync::Arc::new(tokio::sync::RwLock::new(Default::default())),
        global_default_model: std::sync::Arc::new(tokio::sync::RwLock::new("gpt-4o".to_string())),
        admin_broadcast_tx: admin_tx,
        chat_db: crate::serve::ChatDb::open(std::path::Path::new(":memory:")).unwrap(),
        data_dir: tmp_path,
    }
}
```

Run: `cargo test 2>&1 | tail -40`
Commit: `git commit -m "feat(api): session-cookie auth in events_handler; add me_handler; rewrite auth tests"`

---

## Task 5: main.rs — Remove Token CLI Flags

**Files:**
- Modify: `src/main.rs`

Remove from the `rune notes` subcommand section:
- `user_token` and `admin_token` fields from `NotesOptions` construction
- `--token` / `-t` CLI arg handling
- `--admin-token` / `-a` CLI arg handling
- `RUNE_NOTES_USER_TOKEN` env var block
- `RUNE_NOTES_ADMIN_TOKEN` env var block

The resulting `opts` construction should be:
```rust
let mut opts = serve::NotesOptions {
    port: notes_cfg.port.unwrap_or(9527),
    bind: notes_cfg.bind.as_deref().and_then(|b| b.parse().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
};
```

CLI loop only handles `--port`/`-p` and `--bind`/`-b`.

Run: `cargo build 2>&1 | tail -10` then `cargo test 2>&1 | tail -20`
Commit: `git commit -m "feat(main): remove token CLI flags; GitHub OAuth is the only auth method"`

---

## Task 6: Frontend — Replace login.html, Update app.js

**Files:**
- Modify: `web/login.html`
- Modify: `web/app.js`

### login.html changes:
Replace entire file with a minimal GitHub OAuth button page that:
- Shows the ᚱᚢᚾᛖ rune title
- Has a single green "Sign in with GitHub" button linking to `/auth/github`
- Has an error div that shows error messages from `?error=...` query params
- On load: reads `rune_session_id` from localStorage or cookie; if found, calls `/api/me` to verify; if valid, redirects to `/notes/`
- Has "browse public notes" link at bottom

### app.js changes:
1. Remove `let myToken = '';` variable declaration (keep `myNickname`)
2. Remove `submitNickname()` function entirely
3. Remove `loadStoredCredentials()` function entirely
4. Remove the `document.getElementById('nickname-input').addEventListener` block
5. In `fetchNoteListAndConnect()`: remove `myToken` Authorization header; add `credentials: 'include'`
6. In `connect()`: remove `if (myToken) params.set('token', myToken)` line; add `{ withCredentials: true }` to `EventSource` constructor
7. In `api()` helper: remove `if (myToken) headers['Authorization'] = ...`; add `credentials: 'include'` to fetch options
8. In `handleMessage` `auth_result` case: add `myNickname = msg.login || myNickname;` before the admin/guest checks
9. In `confirmLogout()`: change to `window.location.href = '/auth/logout'` (server handles cookie clearing); also `localStorage.removeItem('rune_session_id')`
10. Add `getSessionId()` helper:
    ```javascript
    function getSessionId() {
        const ls = localStorage.getItem('rune_session_id');
        if (ls) return ls;
        const match = document.cookie.match(/(?:^|;\s*)rune_session_id=([^;]+)/);
        if (match) {
            try { localStorage.setItem('rune_session_id', match[1]); } catch {}
            return match[1];
        }
        return null;
    }
    ```
11. Add `initSession()` IIFE at bottom of app.js (runs on page load):
    ```javascript
    (async function initSession() {
        const sessionId = getSessionId();
        if (!sessionId) {
            window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
            return;
        }
        try {
            const resp = await fetch('/api/me', { credentials: 'include' });
            const data = resp.ok ? await resp.json() : { ok: false };
            if (data.ok) {
                myNickname = data.login;
                isAdmin = data.role === 'admin';
                isGuest = data.role === 'guest';
                connect();
            } else {
                localStorage.removeItem('rune_session_id');
                window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
            }
        } catch {
            connect(); // network error — SSE will handle auth
        }
    })();
    ```
12. Remove old `loadStoredCredentials()` call that was at the bottom of the file

Run: `cargo build 2>&1 | tail -10`
Commit: `git commit -m "feat(frontend): GitHub OAuth login page; remove token auth from app.js"`

---

## Task 7: Final Verification and Docs Update

**Files:**
- Modify: `AGENTS.md`

Steps:
1. Run full test suite: `cargo test 2>&1 | grep -E "^(test result|FAILED|error)"` — must show 0 failed
2. Check formatting: `cargo fmt --all -- --check` — must produce no diff
3. Run clippy: `cargo clippy 2>&1 | grep "^error"` — must produce no errors
4. Release build: `cargo build --release 2>&1 | tail -5` — must succeed
5. Update `AGENTS.md`:
   - In `### Notes (Serve Mode) Configuration` section, replace the `admin_token`/`user_token`/`guest_token` example with the new `[notes.github_oauth]` TOML example
   - Remove references to old token fields throughout
   - Add `/auth/github`, `/auth/github/callback`, `/auth/logout`, `/auth/denied` to any route documentation
   - Add `GET /api/me` to any API documentation
6. Commit: `git commit -m "docs(agents): update Notes config docs for GitHub OAuth"`
7. Final test count verification: `cargo test 2>&1 | grep "test result"`
