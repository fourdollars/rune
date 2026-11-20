//! First-run Web UI setup and Admin configuration management + hot-reload.

use crate::config::RuneConfig;
use crate::serve::oauth::{
    generate_session_id, set_session_cookie, AuthenticatedUser, Role, Session,
    SESSION_DURATION_SECS,
};
use crate::serve::ServerState;
use axum::{
    extract::{Extension, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of reloading configuration from disk.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadResult {
    pub restart_required: bool,
    pub models_count: usize,
}

/// Structured configuration payload shared by first-run setup and admin settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredConfigPayload {
    pub provider: Option<String>,
    /// Empty or None when updating existing config means "keep existing api_key".
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    #[serde(default)]
    pub openrouter_zdr: bool,
    pub skills_dir: Option<String>,
    #[serde(default)]
    pub embedding_enabled: bool,
    pub embedding_model: Option<String>,
    // [notes] section
    pub notes_port: Option<u16>,
    pub notes_bind: Option<String>,
    pub notes_model: Option<String>,
    pub notes_title: Option<String>,
    pub notes_desc: Option<String>,
    #[serde(default)]
    pub agent_skills: bool,
    #[serde(default)]
    pub persona_files: bool,
    #[serde(default)]
    pub cron_jobs: bool,
    // [notes.local]
    #[serde(default)]
    pub local_enabled: bool,
    #[serde(default)]
    pub local_admins: Vec<String>,
    #[serde(default)]
    pub local_users: Vec<String>,
    #[serde(default)]
    pub local_guests: Vec<String>,
    // [notes.github]
    #[serde(default)]
    pub github_enabled: bool,
    pub github_client_id: Option<String>,
    /// Empty or None when updating existing config means "keep existing client_secret".
    pub github_client_secret: Option<String>,
    #[serde(default)]
    pub github_admins: Vec<String>,
    #[serde(default)]
    pub github_users: Vec<String>,
    #[serde(default)]
    pub github_guests: Vec<String>,
}

/// Request body for `POST /api/setup/complete`.
#[derive(Debug, Deserialize)]
pub struct SetupCompleteRequest {
    pub setup_token: Option<String>,
    #[serde(flatten)]
    pub config: StructuredConfigPayload,
}

/// Request body for `PUT /api/admin/config` (structured fields only; raw TOML is not exposed).
#[derive(Debug, Deserialize)]
pub struct AdminConfigUpdateRequest {
    pub config: StructuredConfigPayload,
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn clean_opt(opt: Option<String>) -> Option<String> {
    opt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn clean_list(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn set_opt_string(table: &mut toml::Table, key: &str, value: Option<String>) {
    match value {
        Some(v) => {
            table.insert(key.to_string(), toml::Value::String(v));
        }
        None => {
            table.remove(key);
        }
    }
}

fn toml_string_array(items: Vec<String>) -> toml::Value {
    toml::Value::Array(items.into_iter().map(toml::Value::String).collect())
}

fn extract_local_usernames(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|e| {
            let trimmed = e.trim();
            if trimmed.is_empty() {
                return None;
            }
            let user = trimmed
                .split_once(':')
                .map(|(u, _)| u)
                .unwrap_or(trimmed)
                .trim();
            if user.is_empty() {
                None
            } else {
                Some(user.to_string())
            }
        })
        .collect()
}

fn collect_existing_local_passwords(
    notes_table: &toml::Table,
    fallback_cfg: &RuneConfig,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(ref local) = fallback_cfg.notes.local {
        for entry in local
            .admins
            .iter()
            .chain(local.users.iter())
            .chain(local.guests.iter())
        {
            if let Some((u, p)) = entry.trim().split_once(':') {
                let u = u.trim();
                if !u.is_empty() && !p.is_empty() {
                    map.insert(u.to_string(), entry.trim().to_string());
                }
            }
        }
    }
    if let Some(local_tbl) = notes_table.get("local").and_then(|v| v.as_table()) {
        for key in ["admins", "users", "guests"] {
            if let Some(arr) = local_tbl.get(key).and_then(|v| v.as_array()) {
                for val in arr.iter().filter_map(|v| v.as_str()) {
                    if let Some((u, p)) = val.trim().split_once(':') {
                        let u = u.trim();
                        if !u.is_empty() && !p.is_empty() {
                            map.insert(u.to_string(), val.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    map
}

fn resolve_local_entries(
    entries: Vec<String>,
    existing_passwords: &std::collections::HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut resolved = Vec::new();
    for raw in clean_list(entries) {
        if let Some((u, p)) = raw.split_once(':') {
            let u = u.trim();
            if u.is_empty() || p.is_empty() {
                return Err(format!(
                    "Invalid local account '{}': expected 'username:password' or existing 'username'",
                    raw
                ));
            }
            resolved.push(format!("{}:{}", u, p));
        } else if let Some(existing) = existing_passwords.get(&raw) {
            resolved.push(existing.clone());
        } else {
            return Err(format!(
                "New local account '{}' requires a password ('username:password')",
                raw
            ));
        }
    }
    Ok(resolved)
}

/// Merge a `StructuredConfigPayload` into an existing TOML document (preserving
/// unmanaged sections such as `[policy]`, `[mcp]`, `[[notes.oauth]]`, `[[notes.line]]`, etc.).
pub fn merge_structured_into_toml(
    existing_raw: Option<&str>,
    payload: &StructuredConfigPayload,
    fallback_cfg: &RuneConfig,
) -> Result<String, String> {
    let mut root: toml::Table = match existing_raw {
        Some(raw) if !raw.trim().is_empty() => {
            toml::from_str(raw).map_err(|e| format!("Failed to parse existing config: {}", e))?
        }
        _ => toml::Table::new(),
    };

    // Top-level provider & model settings
    set_opt_string(&mut root, "provider", clean_opt(payload.provider.clone()));

    if payload.clear_api_key {
        root.remove("api_key");
    } else if let Some(key) = clean_opt(payload.api_key.clone()) {
        root.insert("api_key".to_string(), toml::Value::String(key));
    } else if !root.contains_key("api_key") {
        if let Some(ref existing_key) = fallback_cfg.api_key {
            if !existing_key.is_empty() {
                root.insert(
                    "api_key".to_string(),
                    toml::Value::String(existing_key.clone()),
                );
            }
        }
    }

    set_opt_string(&mut root, "base_url", clean_opt(payload.base_url.clone()));
    set_opt_string(&mut root, "model", clean_opt(payload.model.clone()));
    set_opt_string(&mut root, "thinking", clean_opt(payload.thinking.clone()));
    set_opt_string(
        &mut root,
        "skills_dir",
        clean_opt(payload.skills_dir.clone()),
    );

    if payload.openrouter_zdr {
        root.insert("openrouter_zdr".to_string(), toml::Value::Boolean(true));
    } else {
        root.remove("openrouter_zdr");
    }

    // [embedding]
    if payload.embedding_enabled {
        let emb = root
            .entry("embedding")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(emb_table) = emb.as_table_mut() {
            emb_table.insert("enabled".to_string(), toml::Value::Boolean(true));
            if let Some(model) = clean_opt(payload.embedding_model.clone()) {
                emb_table.insert("model".to_string(), toml::Value::String(model));
            } else if !emb_table.contains_key("model") {
                emb_table.insert(
                    "model".to_string(),
                    toml::Value::String("text-embedding-3-small".to_string()),
                );
            }
            if !emb_table.contains_key("threshold") {
                emb_table.insert("threshold".to_string(), toml::Value::Float(0.3));
            }
        }
    } else if let Some(emb_table) = root.get_mut("embedding").and_then(|v| v.as_table_mut()) {
        emb_table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }

    // [notes]
    let notes_val = root
        .entry("notes")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let notes_table = notes_val
        .as_table_mut()
        .ok_or_else(|| "[notes] in config must be a table".to_string())?;

    if let Some(port) = payload.notes_port {
        notes_table.insert("port".to_string(), toml::Value::Integer(port as i64));
    }
    set_opt_string(notes_table, "bind", clean_opt(payload.notes_bind.clone()));
    set_opt_string(notes_table, "model", clean_opt(payload.notes_model.clone()));
    set_opt_string(notes_table, "title", clean_opt(payload.notes_title.clone()));
    set_opt_string(notes_table, "desc", clean_opt(payload.notes_desc.clone()));
    notes_table.insert(
        "agent_skills".to_string(),
        toml::Value::Boolean(payload.agent_skills),
    );
    notes_table.insert(
        "persona_files".to_string(),
        toml::Value::Boolean(payload.persona_files),
    );
    notes_table.insert(
        "cron_jobs".to_string(),
        toml::Value::Boolean(payload.cron_jobs),
    );

    // [notes.local] — only emit when enabled AND at least one account is provided
    let existing_local_passwords = collect_existing_local_passwords(notes_table, fallback_cfg);
    let local_admins = if payload.local_enabled {
        resolve_local_entries(payload.local_admins.clone(), &existing_local_passwords)?
    } else {
        Vec::new()
    };
    let local_users = if payload.local_enabled {
        resolve_local_entries(payload.local_users.clone(), &existing_local_passwords)?
    } else {
        Vec::new()
    };
    let local_guests = if payload.local_enabled {
        resolve_local_entries(payload.local_guests.clone(), &existing_local_passwords)?
    } else {
        Vec::new()
    };
    let has_local_accounts =
        !local_admins.is_empty() || !local_users.is_empty() || !local_guests.is_empty();
    if payload.local_enabled && has_local_accounts {
        let mut local_table = toml::Table::new();
        local_table.insert("admins".to_string(), toml_string_array(local_admins));
        local_table.insert("users".to_string(), toml_string_array(local_users));
        local_table.insert("guests".to_string(), toml_string_array(local_guests));
        notes_table.insert("local".to_string(), toml::Value::Table(local_table));
    } else {
        notes_table.remove("local");
    }

    // [notes.github] — only emit when enabled AND at least one credential or role entry is provided
    if payload.github_enabled {
        let existing_secret = notes_table
            .get("github")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("client_secret"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                fallback_cfg
                    .notes
                    .github
                    .as_ref()
                    .map(|g| g.client_secret.clone())
            })
            .unwrap_or_default();

        let client_id = clean_opt(payload.github_client_id.clone()).unwrap_or_default();
        let client_secret =
            clean_opt(payload.github_client_secret.clone()).unwrap_or(existing_secret);
        let github_admins = clean_list(payload.github_admins.clone());
        let github_users = clean_list(payload.github_users.clone());
        let github_guests = clean_list(payload.github_guests.clone());
        let has_github_data = !client_id.is_empty()
            || !client_secret.is_empty()
            || !github_admins.is_empty()
            || !github_users.is_empty()
            || !github_guests.is_empty();

        if has_github_data {
            let mut gh_table = toml::Table::new();
            gh_table.insert("client_id".to_string(), toml::Value::String(client_id));
            gh_table.insert(
                "client_secret".to_string(),
                toml::Value::String(client_secret),
            );
            gh_table.insert("admins".to_string(), toml_string_array(github_admins));
            gh_table.insert("users".to_string(), toml_string_array(github_users));
            gh_table.insert("guests".to_string(), toml_string_array(github_guests));
            notes_table.insert("github".to_string(), toml::Value::Table(gh_table));
        } else {
            notes_table.remove("github");
        }
    } else {
        notes_table.remove("github");
    }

    let rendered = toml::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize configuration: {}", e))?;
    crate::config::validate_toml_str(&rendered)
        .map_err(|e| format!("Invalid configuration: {}", e))?;
    Ok(rendered)
}

/// Atomically write `content` to `path` with `0600` permissions on Unix.
pub fn write_config_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory {}: {}",
                parent.display(),
                e
            )
        })?;
    }

    let tmp_path = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temporary config file: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to save config to {}: {}", path.display(), e))?;
    Ok(())
}

fn validate_local_admin_entry(entry: &str) -> Option<String> {
    let trimmed = entry.trim();
    let (user, pass) = trimmed.split_once(':')?;
    let user = user.trim();
    if user.is_empty() || pass.is_empty() {
        return None;
    }
    Some(user.to_string())
}

/// `POST /api/setup/complete` — first-run initialization when `rune.toml` does not exist.
pub async fn setup_complete_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SetupCompleteRequest>,
) -> Response {
    if !state.needs_setup() {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": "Server is already configured"
            })),
        )
            .into_response();
    }

    if crate::serve::request_requires_setup_token(&state, &headers) {
        let expected = state.config_meta.setup_token.as_deref().unwrap_or("");
        let provided = req.setup_token.as_deref().unwrap_or("").trim();
        if expected.is_empty() || !constant_time_eq(expected, provided) {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": "Invalid setup token"
                })),
            )
                .into_response();
        }
    }

    let local_admins = clean_list(req.config.local_admins.clone());
    let valid_local_admin = if req.config.local_enabled {
        local_admins
            .iter()
            .find_map(|entry| validate_local_admin_entry(entry))
    } else {
        None
    };

    let github_admins = clean_list(req.config.github_admins.clone());
    let valid_github_admin = req.config.github_enabled
        && clean_opt(req.config.github_client_id.clone()).is_some()
        && clean_opt(req.config.github_client_secret.clone()).is_some()
        && !github_admins.is_empty();

    if valid_local_admin.is_none() && !valid_github_admin {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": "Please configure at least one admin account (Local admin in 'username:password' format or GitHub OAuth with Client ID, Client Secret, and Admin login)."
            })),
        )
            .into_response();
    }

    // Validate all non-empty local entries have username:password format
    if req.config.local_enabled {
        for entry in local_admins
            .iter()
            .chain(req.config.local_users.iter())
            .chain(req.config.local_guests.iter())
        {
            if !entry.trim().is_empty() && validate_local_admin_entry(entry).is_none() {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "ok": false,
                        "error": format!("Invalid local account '{}': expected 'username:password'", entry.trim())
                    })),
                )
                    .into_response();
            }
        }
    }

    let current_cfg = state.config_snapshot();
    let toml_content = match merge_structured_into_toml(None, &req.config, &current_cfg) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "ok": false, "error": e })),
            )
                .into_response();
        }
    };

    if let Err(e) = write_config_atomic(&state.config_meta.config_path, &toml_content) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response();
    }

    let reload = match state.reload_config().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "ok": false, "error": e })),
            )
                .into_response();
        }
    };

    // Auto-login as the created Local admin when Local auth is configured
    if let Some(admin_username) = valid_local_admin {
        let session_id = generate_session_id();
        let login = if admin_username.starts_with("local:") {
            admin_username
        } else {
            format!("local:{}", admin_username)
        };
        let session = Session {
            id: session_id.clone(),
            login: login.clone(),
            role: Role::Admin,
            avatar_url: String::new(),
            expires_at: crate::serve::db::now_secs() + SESSION_DURATION_SECS,
        };
        state.sessions.insert(session).await;

        let (http_only, js_readable) = set_session_cookie(&session_id);
        let mut response_headers = HeaderMap::new();
        if let Ok(val) = http_only.parse() {
            response_headers.append(header::SET_COOKIE, val);
        }
        if let Ok(val) = js_readable.parse() {
            response_headers.append(header::SET_COOKIE, val);
        }

        return (
            StatusCode::OK,
            response_headers,
            axum::Json(serde_json::json!({
                "ok": true,
                "auto_logged_in": true,
                "redirect": "/edit/",
                "restart_required": reload.restart_required,
                "models_count": reload.models_count,
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "ok": true,
            "auto_logged_in": false,
            "redirect": "/",
            "restart_required": reload.restart_required,
            "models_count": reload.models_count,
        })),
    )
        .into_response()
}

/// `GET /api/admin/config` — return current config for admin editing (secrets masked in structured view).
pub async fn admin_config_get_handler(
    State(state): State<ServerState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }

    let cfg = state.config_snapshot();

    let structured = StructuredConfigPayload {
        provider: cfg.provider.clone(),
        api_key: None, // Never return api_key; use has_api_key flag
        clear_api_key: false,
        base_url: cfg.base_url.clone(),
        model: if cfg.model.is_empty() {
            None
        } else {
            Some(cfg.model.clone())
        },
        thinking: cfg.thinking.clone(),
        openrouter_zdr: cfg.openrouter_zdr,
        skills_dir: Some(cfg.skills_dir.clone()),
        embedding_enabled: cfg.embedding.enabled,
        embedding_model: cfg.embedding.model.clone(),
        notes_port: cfg.notes.port,
        notes_bind: cfg.notes.bind.clone(),
        notes_model: cfg.notes.model.clone(),
        notes_title: cfg.notes.title.clone(),
        notes_desc: cfg.notes.desc.clone(),
        agent_skills: cfg.notes.agent_skills,
        persona_files: cfg.notes.persona_files,
        cron_jobs: cfg.notes.cron_jobs,
        local_enabled: cfg.notes.local.is_some(),
        // Never return local account passwords; return usernames only.
        local_admins: cfg
            .notes
            .local
            .as_ref()
            .map(|l| extract_local_usernames(&l.admins))
            .unwrap_or_default(),
        local_users: cfg
            .notes
            .local
            .as_ref()
            .map(|l| extract_local_usernames(&l.users))
            .unwrap_or_default(),
        local_guests: cfg
            .notes
            .local
            .as_ref()
            .map(|l| extract_local_usernames(&l.guests))
            .unwrap_or_default(),
        github_enabled: cfg.notes.github.is_some(),
        github_client_id: cfg.notes.github.as_ref().map(|g| g.client_id.clone()),
        github_client_secret: None, // Never return client_secret; use has_github_client_secret flag
        github_admins: cfg
            .notes
            .github
            .as_ref()
            .map(|g| g.admins.clone())
            .unwrap_or_default(),
        github_users: cfg
            .notes
            .github
            .as_ref()
            .map(|g| g.users.clone())
            .unwrap_or_default(),
        github_guests: cfg
            .notes
            .github
            .as_ref()
            .map(|g| g.guests.clone())
            .unwrap_or_default(),
    };

    axum::Json(serde_json::json!({
        "ok": true,
        "has_api_key": cfg.api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        "has_github_client_secret": cfg
            .notes
            .github
            .as_ref()
            .map(|g| !g.client_secret.is_empty())
            .unwrap_or(false),
        "config": structured,
    }))
    .into_response()
}

/// `PUT /api/admin/config` — update structured fields in `rune.toml` and hot-reload.
pub async fn admin_config_put_handler(
    State(state): State<ServerState>,
    Extension(user): Extension<AuthenticatedUser>,
    axum::Json(req): axum::Json<AdminConfigUpdateRequest>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }

    let existing_raw = std::fs::read_to_string(&state.config_meta.config_path).ok();
    let current_cfg = state.config_snapshot();
    let toml_content =
        match merge_structured_into_toml(existing_raw.as_deref(), &req.config, &current_cfg) {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "ok": false, "error": e })),
                )
                    .into_response();
            }
        };

    if let Err(e) = write_config_atomic(&state.config_meta.config_path, &toml_content) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response();
    }

    match state.reload_config().await {
        Ok(reload) => axum::Json(serde_json::json!({
            "ok": true,
            "restart_required": reload.restart_required,
            "models_count": reload.models_count,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

/// `POST /api/admin/config/reload` — reload `rune.toml` from disk without modifying it.
pub async fn admin_config_reload_handler(
    State(state): State<ServerState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }

    if !state.config_meta.config_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": format!("Config file not found: {}", state.config_meta.config_path.display())
            })),
        )
            .into_response();
    }

    match state.reload_config().await {
        Ok(reload) => axum::Json(serde_json::json!({
            "ok": true,
            "restart_required": reload.restart_required,
            "models_count": reload.models_count,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

/// GitHub Copilot OAuth App client ID (same as `rune init`).
const COPILOT_GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Request body for `POST /api/setup/copilot/start`.
#[derive(Debug, Default, Deserialize)]
pub struct SetupCopilotStartRequest {
    pub setup_token: Option<String>,
}

/// Request body for `POST /api/setup/copilot/poll`.
#[derive(Debug, Deserialize)]
pub struct SetupCopilotPollRequest {
    pub setup_token: Option<String>,
    pub device_code: String,
}

/// Request body for `POST /api/admin/copilot/poll`.
#[derive(Debug, Deserialize)]
pub struct AdminCopilotPollRequest {
    pub device_code: String,
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

async fn copilot_device_start_impl() -> Response {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let resp = match client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "client_id={}&scope=read:user",
            COPILOT_GITHUB_CLIENT_ID
        ))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": format!("Failed to contact GitHub: {}", e)
                })),
            )
                .into_response();
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": format!("Invalid response from GitHub: {}", e)
                })),
            )
                .into_response();
        }
    };

    let Some(device_code) = body["device_code"].as_str() else {
        let err = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("GitHub did not return a device_code");
        return (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({ "ok": false, "error": err })),
        )
            .into_response();
    };
    let Some(user_code) = body["user_code"].as_str() else {
        return (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": "GitHub did not return a user_code"
            })),
        )
            .into_response();
    };

    let verification_uri = body["verification_uri"]
        .as_str()
        .unwrap_or("https://github.com/login/device");
    let expires_in = body["expires_in"].as_u64().unwrap_or(900);
    let interval = body["interval"].as_u64().unwrap_or(5);

    axum::Json(serde_json::json!({
        "ok": true,
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "expires_in": expires_in,
        "interval": interval,
    }))
    .into_response()
}

async fn copilot_device_poll_impl(device_code: &str) -> Response {
    let device_code = device_code.trim();
    if device_code.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": "device_code is required"
            })),
        )
            .into_response();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let resp = match client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "client_id={}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
            COPILOT_GITHUB_CLIENT_ID,
            percent_encode(device_code)
        ))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": format!("Failed to poll GitHub: {}", e)
                })),
            )
                .into_response();
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": format!("Invalid poll response from GitHub: {}", e)
                })),
            )
                .into_response();
        }
    };

    if let Some(token) = body["access_token"].as_str() {
        return axum::Json(serde_json::json!({
            "ok": true,
            "status": "authorized",
            "access_token": token,
        }))
        .into_response();
    }

    match body["error"].as_str() {
        Some("authorization_pending") => axum::Json(serde_json::json!({
            "ok": true,
            "status": "pending",
        }))
        .into_response(),
        Some("slow_down") => axum::Json(serde_json::json!({
            "ok": true,
            "status": "slow_down",
            "interval": body["interval"].as_u64(),
        }))
        .into_response(),
        Some("expired_token") => axum::Json(serde_json::json!({
            "ok": true,
            "status": "expired",
        }))
        .into_response(),
        Some("access_denied") => axum::Json(serde_json::json!({
            "ok": true,
            "status": "denied",
        }))
        .into_response(),
        other => {
            let msg = body["error_description"]
                .as_str()
                .or(other)
                .unwrap_or("GitHub device authorization failed");
            (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({ "ok": false, "error": msg })),
            )
                .into_response()
        }
    }
}

fn check_setup_access(
    state: &ServerState,
    headers: &HeaderMap,
    setup_token: Option<&str>,
) -> Option<Response> {
    if !state.needs_setup() {
        return Some(
            (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "error": "Server is already configured"
                })),
            )
                .into_response(),
        );
    }
    if crate::serve::request_requires_setup_token(state, headers) {
        let expected = state.config_meta.setup_token.as_deref().unwrap_or("");
        let provided = setup_token.unwrap_or("").trim();
        if expected.is_empty() || !constant_time_eq(expected, provided) {
            return Some(
                (
                    StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "ok": false,
                        "error": "Invalid setup token"
                    })),
                )
                    .into_response(),
            );
        }
    }
    None
}

/// `POST /api/setup/copilot/start` — begin GitHub Copilot OAuth device flow during first-run setup.
pub async fn setup_copilot_start_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SetupCopilotStartRequest>,
) -> Response {
    if let Some(err_resp) = check_setup_access(&state, &headers, req.setup_token.as_deref()) {
        return err_resp;
    }
    copilot_device_start_impl().await
}

/// `POST /api/setup/copilot/poll` — poll GitHub Copilot OAuth device flow during first-run setup.
pub async fn setup_copilot_poll_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SetupCopilotPollRequest>,
) -> Response {
    if let Some(err_resp) = check_setup_access(&state, &headers, req.setup_token.as_deref()) {
        return err_resp;
    }
    copilot_device_poll_impl(&req.device_code).await
}

/// `POST /api/admin/copilot/start` — begin GitHub Copilot OAuth device flow from Admin Settings.
pub async fn admin_copilot_start_handler(
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }
    copilot_device_start_impl().await
}

/// `POST /api/admin/copilot/poll` — poll GitHub Copilot OAuth device flow from Admin Settings.
pub async fn admin_copilot_poll_handler(
    Extension(user): Extension<AuthenticatedUser>,
    axum::Json(req): axum::Json<AdminCopilotPollRequest>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }
    copilot_device_poll_impl(&req.device_code).await
}

/// Request body for `POST /api/setup/verify-token`.
#[derive(Debug, Default, Deserialize)]
pub struct SetupVerifyTokenRequest {
    pub setup_token: Option<String>,
}

/// Request body for `POST /api/setup/models`.
#[derive(Debug, Default, Deserialize)]
pub struct SetupDetectModelsRequest {
    pub setup_token: Option<String>,
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    #[serde(default)]
    pub openrouter_zdr: bool,
}

/// Request body for `POST /api/admin/models`.
#[derive(Debug, Default, Deserialize)]
pub struct AdminDetectModelsRequest {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    #[serde(default)]
    pub openrouter_zdr: bool,
}

/// Model metadata entry returned by model detection endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedModelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<String>,
}

async fn fetch_copilot_models_bearer(session_token: &str) -> Vec<crate::provider::ModelInfo> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    for endpoint in [
        "https://api.githubcopilot.com/models",
        "https://api.business.githubcopilot.com/models",
    ] {
        let resp = match client
            .get(endpoint)
            .bearer_auth(session_token)
            .header("User-Agent", "rune/0.1.0")
            .header("Copilot-Integration-Id", "vscode-chat")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(arr) = body
            .get("data")
            .or_else(|| body.get("models"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let mut models: Vec<crate::provider::ModelInfo> = arr
            .iter()
            .filter(|m| {
                m.get("model_picker_enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .filter(|m| {
                m.get("supported_endpoints")
                    .and_then(|v| v.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            })
            .filter_map(|m| {
                let id = m
                    .get("id")
                    .or_else(|| m.get("name"))
                    .and_then(|v| v.as_str())?
                    .to_string();
                let context_window = m
                    .pointer("/capabilities/limits/max_context_window_tokens")
                    .and_then(|v| v.as_u64());
                let reasoning_efforts = m
                    .pointer("/capabilities/supports/reasoning_effort")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(crate::provider::ModelInfo {
                    id,
                    provider: Some("github-copilot".to_string()),
                    context_window,
                    reasoning_efforts,
                    supported_endpoints: vec![],
                })
            })
            .collect();
        if !models.is_empty() {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            return models;
        }
    }
    Vec::new()
}

async fn fetch_openrouter_models_public(openrouter_zdr: bool) -> Vec<crate::provider::ModelInfo> {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let zdr_set = if openrouter_zdr {
        match client
            .get("https://openrouter.ai/api/v1/endpoints/zdr")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v["data"].as_array().cloned())
                .map(|arr| {
                    arr.into_iter()
                        .filter_map(|e| e["model_id"].as_str().map(|s| s.to_string()))
                        .collect::<std::collections::HashSet<String>>()
                }),
            _ => None,
        }
    } else {
        None
    };

    let resp = match client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = body["data"].as_array() else {
        return Vec::new();
    };
    let mut models: Vec<crate::provider::ModelInfo> = arr
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            if let Some(ref zdr) = zdr_set {
                if id != "openrouter/auto" && !zdr.contains(&id) {
                    return None;
                }
            }
            let context_window = m["context_length"].as_u64();
            let supports_reasoning = m
                .get("supported_parameters")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|p| p.as_str() == Some("reasoning")))
                .unwrap_or(false);
            let reasoning_efforts = if id.starts_with("openrouter/auto") || supports_reasoning {
                vec!["low".to_string(), "medium".to_string(), "high".to_string()]
            } else {
                vec![]
            };
            Some(crate::provider::ModelInfo {
                id,
                provider: Some("openrouter".to_string()),
                context_window,
                reasoning_efforts,
                supported_endpoints: vec![],
            })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(pos) = models.iter().position(|m| m.id == "openrouter/auto") {
        let auto = models.remove(pos);
        models.insert(0, auto);
    }
    models
}

async fn detect_models_impl(
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    openrouter_zdr: bool,
) -> Result<Vec<DetectedModelEntry>, String> {
    let provider_clean = clean_opt(provider);
    let api_key_clean = clean_opt(api_key);
    let base_url_clean = clean_opt(base_url);

    let mut cfg = RuneConfig::default();
    cfg.provider = provider_clean.clone();
    cfg.api_key = api_key_clean.clone();
    cfg.base_url = base_url_clean.clone();
    cfg.openrouter_zdr = openrouter_zdr;

    let mut discovered = match crate::serve::api::build_provider_pub(&cfg) {
        Ok(registry) => registry.list_models().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    if discovered.is_empty() {
        match provider_clean.as_deref() {
            Some("github-copilot") => {
                if let Some(ref key) = api_key_clean {
                    discovered = fetch_copilot_models_bearer(key).await;
                }
            }
            Some("openrouter") => {
                discovered = fetch_openrouter_models_public(openrouter_zdr).await;
            }
            _ => {}
        }
    }

    if provider_clean.as_deref() == Some("openrouter") {
        if let Some(pos) = discovered.iter().position(|m| m.id == "openrouter/auto") {
            let auto = discovered.remove(pos);
            discovered.insert(0, auto);
        }
    }

    Ok(discovered
        .into_iter()
        .map(|m| DetectedModelEntry {
            id: m.id,
            context_window: m.context_window,
            reasoning_efforts: m.reasoning_efforts,
        })
        .collect())
}

/// `POST /api/setup/verify-token` — verify one-time setup token early on Step 1.
pub async fn setup_verify_token_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SetupVerifyTokenRequest>,
) -> Response {
    if let Some(err_resp) = check_setup_access(&state, &headers, req.setup_token.as_deref()) {
        return err_resp;
    }
    axum::Json(serde_json::json!({ "ok": true })).into_response()
}

/// `POST /api/setup/models` — detect available models for the selected provider during first-run setup.
pub async fn setup_models_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SetupDetectModelsRequest>,
) -> Response {
    if let Some(err_resp) = check_setup_access(&state, &headers, req.setup_token.as_deref()) {
        return err_resp;
    }
    match detect_models_impl(req.provider, req.api_key, req.base_url, req.openrouter_zdr).await {
        Ok(models) => axum::Json(serde_json::json!({
            "ok": true,
            "models": models,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

/// `POST /api/admin/models` — detect available models from Admin Settings (reuses saved api_key if omitted).
pub async fn admin_models_handler(
    State(state): State<ServerState>,
    Extension(user): Extension<AuthenticatedUser>,
    axum::Json(req): axum::Json<AdminDetectModelsRequest>,
) -> Response {
    if user.role != Role::Admin {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "ok": false, "error": "Admin access required" })),
        )
            .into_response();
    }

    let snap = state.config_snapshot();
    let effective_provider = clean_opt(req.provider).or_else(|| snap.provider.clone());
    let effective_api_key = clean_opt(req.api_key).or_else(|| {
        if effective_provider == snap.provider || snap.provider.is_none() {
            snap.api_key.clone()
        } else {
            None
        }
    });
    let effective_base_url = clean_opt(req.base_url).or_else(|| {
        if effective_provider == snap.provider {
            snap.base_url.clone()
        } else {
            None
        }
    });

    match detect_models_impl(
        effective_provider,
        effective_api_key,
        effective_base_url,
        req.openrouter_zdr,
    )
    .await
    {
        Ok(models) => axum::Json(serde_json::json!({
            "ok": true,
            "models": models,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::{ChatDb, ConfigMeta, MountOverrides};
    use axum::body::to_bytes;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{broadcast, RwLock};

    fn make_state(
        tmp: &TempDir,
        needs_setup: bool,
        setup_token: Option<String>,
        bind_is_loopback: bool,
    ) -> ServerState {
        let (admin_tx, _) = broadcast::channel(8);
        let chat_db = ChatDb::open(Path::new(":memory:")).unwrap();
        ServerState {
            config: Arc::new(std::sync::RwLock::new(RuneConfig::default())),
            config_meta: Arc::new(ConfigMeta {
                config_path: tmp.path().join("rune.toml"),
                needs_setup: AtomicBool::new(needs_setup),
                setup_token,
                bind_is_loopback,
                mount_overrides: MountOverrides::default(),
            }),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(RwLock::new(Vec::new())),
            rooms: Arc::new(RwLock::new(HashMap::new())),
            global_default_model: Arc::new(RwLock::new(String::new())),
            admin_broadcast_tx: admin_tx,
            chat_db,
            data_dir: tmp.path().to_path_buf(),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(RwLock::new(crate::provider::ProviderRegistry::new())),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(RwLock::new(HashMap::new())),
            running_cron_jobs: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    async fn resp_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let val = serde_json::from_slice(&bytes).unwrap_or_default();
        (status, val)
    }

    #[tokio::test]
    async fn setup_requires_token_on_non_localhost_and_auto_logs_in_local_admin() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(&tmp, true, Some("secret-token-123".to_string()), false);
        let empty_headers = HeaderMap::new();

        // Wrong token rejected
        let bad_req = SetupCompleteRequest {
            setup_token: Some("wrong".to_string()),
            config: StructuredConfigPayload {
                local_enabled: true,
                local_admins: vec!["admin:pass123".to_string()],
                notes_model: Some("test/model".to_string()),
                ..Default::default()
            },
        };
        let (status, body) = resp_json(
            setup_complete_handler(
                State(state.clone()),
                empty_headers.clone(),
                axum::Json(bad_req),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "Invalid setup token");
        assert!(state.needs_setup());

        // Valid token succeeds, writes rune.toml, clears needs_setup, sets session cookies
        let ok_req = SetupCompleteRequest {
            setup_token: Some("secret-token-123".to_string()),
            config: StructuredConfigPayload {
                provider: Some("openrouter".to_string()),
                api_key: Some("sk-or-test-key".to_string()),
                notes_model: Some("test/model".to_string()),
                notes_title: Some("Team Wiki".to_string()),
                local_enabled: true,
                local_admins: vec!["admin:pass123".to_string()],
                ..Default::default()
            },
        };
        let resp = setup_complete_handler(
            State(state.clone()),
            empty_headers.clone(),
            axum::Json(ok_req),
        )
        .await;
        let cookies: Vec<String> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
            .collect();
        assert!(cookies.iter().any(|c| c.starts_with("rune_sid=")));
        assert!(cookies.iter().any(|c| c.starts_with("rune_session_id=")));

        let (status, body) = resp_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["auto_logged_in"], true);
        assert!(!state.needs_setup());
        assert!(state.config_meta.config_path.exists());
        assert_eq!(
            state.config_snapshot().notes.title.as_deref(),
            Some("Team Wiki")
        );

        // Subsequent setup call is blocked with 403
        let again = SetupCompleteRequest {
            setup_token: Some("secret-token-123".to_string()),
            config: StructuredConfigPayload {
                local_enabled: true,
                local_admins: vec!["admin:pass123".to_string()],
                ..Default::default()
            },
        };
        let (status, _) = resp_json(
            setup_complete_handler(State(state.clone()), empty_headers, axum::Json(again)).await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn setup_token_required_for_reverse_proxied_or_remote_host() {
        let tmp = TempDir::new().unwrap();
        let loopback_state = make_state(&tmp, true, Some("tok123".to_string()), true);

        let mut local_headers = HeaderMap::new();
        local_headers.insert(header::HOST, "127.0.0.1:9527".parse().unwrap());
        assert!(!crate::serve::request_requires_setup_token(
            &loopback_state,
            &local_headers
        ));

        let mut tailscale_headers = HeaderMap::new();
        tailscale_headers.insert(header::HOST, "pico.tail0a1999.ts.net".parse().unwrap());
        assert!(crate::serve::request_requires_setup_token(
            &loopback_state,
            &tailscale_headers
        ));

        let mut proxied_headers = HeaderMap::new();
        proxied_headers.insert(header::HOST, "localhost:9527".parse().unwrap());
        proxied_headers.insert("x-forwarded-for", "100.64.0.1".parse().unwrap());
        assert!(crate::serve::request_requires_setup_token(
            &loopback_state,
            &proxied_headers
        ));
    }

    #[tokio::test]
    async fn admin_config_rbac_secret_preservation_and_disk_reload() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(&tmp, false, None, true);
        let initial_toml = r#"
provider = "openrouter"
api_key = "sk-or-keep-me"

[policy]
mode = "allowlist"
allowed_commands = ["ls"]

[notes]
port = 9527
bind = "127.0.0.1"
model = "test/model"
title = "Initial Title"

[notes.local]
admins = ["admin:pass"]
"#;
        std::fs::write(&state.config_meta.config_path, initial_toml).unwrap();
        state.reload_config().await.unwrap();

        let non_admin = AuthenticatedUser {
            login: "local:user".to_string(),
            role: Role::User,
        };
        let admin = AuthenticatedUser {
            login: "local:admin".to_string(),
            role: Role::Admin,
        };

        // Non-admin blocked
        let (status, _) = resp_json(
            admin_config_get_handler(State(state.clone()), Extension(non_admin.clone())).await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Admin GET masks api_key, strips local passwords, and never returns raw_toml
        let (status, body) = resp_json(
            admin_config_get_handler(State(state.clone()), Extension(admin.clone())).await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["has_api_key"], true);
        assert!(body["config"]["api_key"].is_null());
        assert!(body.get("raw_toml").is_none());
        assert_eq!(body["config"]["local_admins"], serde_json::json!(["admin"]));

        // Admin PUT structured update with blank api_key and username-only local admin
        // preserves api_key, local password, and [policy]
        let put_req = AdminConfigUpdateRequest {
            config: StructuredConfigPayload {
                provider: Some("openrouter".to_string()),
                api_key: None,
                notes_port: Some(9999),
                notes_bind: Some("127.0.0.1".to_string()),
                notes_model: Some("test/model".to_string()),
                notes_title: Some("Updated Title".to_string()),
                local_enabled: true,
                local_admins: vec!["admin".to_string()],
                ..Default::default()
            },
        };
        let (status, body) = resp_json(
            admin_config_put_handler(
                State(state.clone()),
                Extension(admin.clone()),
                axum::Json(put_req),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["restart_required"], true);
        let snap = state.config_snapshot();
        assert_eq!(snap.api_key.as_deref(), Some("sk-or-keep-me"));
        assert_eq!(snap.notes.title.as_deref(), Some("Updated Title"));
        assert_eq!(snap.policy.allowed_commands, vec!["ls".to_string()]);
        assert_eq!(
            snap.notes.local.as_ref().unwrap().admins,
            vec!["admin:pass".to_string()]
        );

        // Adding a brand new local account without a password is rejected
        let bad_local = AdminConfigUpdateRequest {
            config: StructuredConfigPayload {
                local_enabled: true,
                local_admins: vec!["newuser_without_pass".to_string()],
                ..Default::default()
            },
        };
        let (status, _) = resp_json(
            admin_config_put_handler(
                State(state.clone()),
                Extension(admin.clone()),
                axum::Json(bad_local),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // External edit on disk + POST /api/admin/config/reload picks up changes
        let external = initial_toml.replace("Initial Title", "Reloaded From Disk");
        std::fs::write(&state.config_meta.config_path, external).unwrap();
        let (status, body) =
            resp_json(admin_config_reload_handler(State(state.clone()), Extension(admin)).await)
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(
            state.config_snapshot().notes.title.as_deref(),
            Some("Reloaded From Disk")
        );
    }

    #[test]
    fn admin_settings_js_is_embedded() {
        let asset = crate::serve::static_files::get("js/admin-settings.js");
        assert!(
            asset.is_some(),
            "js/admin-settings.js must be embedded in static_files::ASSETS"
        );
    }

    #[test]
    fn empty_notes_local_is_not_written_to_toml() {
        let payload = StructuredConfigPayload {
            local_enabled: true,
            local_admins: vec![],
            local_users: vec![],
            local_guests: vec![],
            github_enabled: true,
            github_client_id: Some("Ov23li123".to_string()),
            github_client_secret: Some("secret".to_string()),
            github_admins: vec!["alice".to_string()],
            ..Default::default()
        };
        let rendered = merge_structured_into_toml(None, &payload, &RuneConfig::default()).unwrap();
        assert!(
            !rendered.contains("[notes.local]"),
            "empty [notes.local] should not be emitted:\n{}",
            rendered
        );
        assert!(rendered.contains("[notes.github]"));
    }

    #[tokio::test]
    async fn copilot_device_endpoints_enforce_setup_token_and_admin_rbac() {
        let tmp = TempDir::new().unwrap();
        let setup_state = make_state(&tmp, true, Some("tok-xyz".to_string()), false);
        let headers = HeaderMap::new();

        // Setup copilot start blocked without valid setup_token
        let (status, _) = resp_json(
            setup_copilot_start_handler(
                State(setup_state.clone()),
                headers.clone(),
                axum::Json(SetupCopilotStartRequest {
                    setup_token: Some("wrong".to_string()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Setup copilot poll blocked once setup is complete
        let done_state = make_state(&tmp, false, None, true);
        let (status, _) = resp_json(
            setup_copilot_poll_handler(
                State(done_state),
                headers,
                axum::Json(SetupCopilotPollRequest {
                    setup_token: None,
                    device_code: "dc".to_string(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Admin copilot endpoints reject non-admin users
        let non_admin = AuthenticatedUser {
            login: "local:user".to_string(),
            role: Role::User,
        };
        let (status, _) =
            resp_json(admin_copilot_start_handler(Extension(non_admin.clone())).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = resp_json(
            admin_copilot_poll_handler(
                Extension(non_admin),
                axum::Json(AdminCopilotPollRequest {
                    device_code: "dc".to_string(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn verify_token_and_model_detection_enforce_security() {
        let tmp = TempDir::new().unwrap();
        let setup_state = make_state(&tmp, true, Some("tok-verify".to_string()), false);
        let headers = HeaderMap::new();

        // Wrong token rejected on verify-token
        let (status, _) = resp_json(
            setup_verify_token_handler(
                State(setup_state.clone()),
                headers.clone(),
                axum::Json(SetupVerifyTokenRequest {
                    setup_token: Some("bad".to_string()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Valid token accepted on verify-token
        let (status, body) = resp_json(
            setup_verify_token_handler(
                State(setup_state.clone()),
                headers.clone(),
                axum::Json(SetupVerifyTokenRequest {
                    setup_token: Some("tok-verify".to_string()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);

        // Setup models rejected with wrong token
        let (status, _) = resp_json(
            setup_models_handler(
                State(setup_state.clone()),
                headers,
                axum::Json(SetupDetectModelsRequest {
                    setup_token: Some("wrong".to_string()),
                    provider: Some("openrouter".to_string()),
                    ..Default::default()
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Admin models rejected for non-admin
        let non_admin = AuthenticatedUser {
            login: "local:user".to_string(),
            role: Role::User,
        };
        let (status, _) = resp_json(
            admin_models_handler(
                State(setup_state),
                Extension(non_admin),
                axum::Json(AdminDetectModelsRequest {
                    provider: Some("openrouter".to_string()),
                    ..Default::default()
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
