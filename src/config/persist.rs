use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Persist a new domain to the user's ~/.rune/rune.toml allowed_domains list.
/// Best-effort: if the file can't be read/written, silently skip.
pub fn persist_domain(domain: &str) {
    persist_policy_array("allowed_domains", domain);
}

/// Persist a new command to the user's ~/.rune/rune.toml allowed_commands list.
/// Best-effort: if the file can't be read/written, silently skip.
pub fn persist_command(command: &str) {
    persist_policy_array("allowed_commands", command);
}

/// Persist a path to allowed_paths_ro in ~/.rune/rune.toml.
pub fn persist_path_ro(path: &str) {
    persist_policy_array("allowed_paths_ro", path);
}

/// Persist a path to allowed_paths_rw in ~/.rune/rune.toml.
pub fn persist_path_rw(path: &str) {
    persist_policy_array("allowed_paths_rw", path);
}

/// Generic helper to persist a value into a policy array field.
pub fn persist_policy_array(field: &str, value: &str) {
    let config_path = match env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".rune").join("rune.toml"),
        Err(_) => return,
    };
    persist_policy_array_at(&config_path, field, value);
}

/// Inner helper: persist a value into a policy array field at an explicit config path.
/// Used by tests to avoid mutating the global HOME env var (which is not thread-safe).
pub fn persist_policy_array_at(config_path: &Path, field: &str, value: &str) {
    let content = match fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut doc: toml::Table = match content.parse() {
        Ok(d) => d,
        Err(_) => return,
    };

    let policy = doc
        .entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let arr = policy
        .entry(field)
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut();
    let arr = match arr {
        Some(a) => a,
        None => return,
    };

    if arr.iter().any(|v| v.as_str() == Some(value)) {
        return;
    }

    arr.push(toml::Value::String(value.to_string()));

    let new_content = doc.to_string();
    let _ = fs::write(config_path, new_content);
}
