//! Embedded static files for the WebUI.
//! Uses include_str! to embed files at compile time.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Embedded web assets. In production this would use rust-embed,
/// but for initial development we use include_str! for simplicity.
static ASSETS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("index.html", include_str!("../../web/index.html"));
    m.insert("app.js", include_str!("../../web/app.js"));
    m.insert("style.css", include_str!("../../web/style.css"));
    m
});

/// Get an embedded file by path.
pub fn get(path: &str) -> Option<String> {
    ASSETS.get(path).map(|s| s.to_string())
}
