use std::env;
use std::path::PathBuf;

/// Expand a leading `~` or `~/` in a path string to the value of `$HOME`.
/// Returns the original string unchanged if `HOME` is not set or the path
/// does not start with `~`.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            format!("{}/{}", home, rest)
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    }
}

/// Apply tilde expansion to every element of a `Vec<String>`.
pub fn expand_tilde_vec(v: &mut Vec<String>) {
    for item in v.iter_mut() {
        *item = expand_tilde(item);
    }
}

/// Safely truncates a string to a maximum byte length without panicking on UTF-8 character boundaries.
pub fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut idx = max_bytes;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

/// Get the Rune data directory (~/.rune).
pub fn data_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".rune")
}
