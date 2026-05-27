//! Embedded static files for the WebUI.

use std::collections::HashMap;
use std::sync::LazyLock;

static ASSETS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("index.html",             include_str!("../../web/index.html"));
    m.insert("favicon.svg",            include_str!("../../web/favicon.svg"));
    m.insert("app.js",                 include_str!("../../web/app.js"));
    m.insert("style.css",              include_str!("../../web/style.css"));
    m.insert("marked.min.js",          include_str!("../../web/marked.min.js"));
    m.insert("highlight.min.js",       include_str!("../../web/highlight.min.js"));
    m.insert("highlight-dark.min.css", include_str!("../../web/highlight-dark.min.css"));
    m
});

/// Large binary assets served as bytes (e.g. mermaid.min.js ~3MB).
static BINARY_ASSETS: LazyLock<HashMap<&'static str, &'static [u8]>> = LazyLock::new(|| {
    let mut m: HashMap<&'static str, &'static [u8]> = HashMap::new();
    m.insert("mermaid.min.js", include_bytes!("../../web/mermaid.min.js"));
    m
});

pub fn get(path: &str) -> Option<String> {
    ASSETS.get(path).map(|s| s.to_string())
}

pub fn get_bytes(path: &str) -> Option<&'static [u8]> {
    BINARY_ASSETS.get(path).copied()
}
