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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_assets_present() {
        assert!(get("index.html").is_some(), "index.html missing");
        assert!(get("app.js").is_some(),     "app.js missing");
        assert!(get("style.css").is_some(),  "style.css missing");
        assert!(get("favicon.svg").is_some(),"favicon.svg missing");
    }

    #[test]
    fn test_binary_assets_present() {
        assert!(get_bytes("mermaid.min.js").is_some(), "mermaid.min.js missing");
        let bytes = get_bytes("mermaid.min.js").unwrap();
        assert!(bytes.len() > 1_000_000, "mermaid.min.js suspiciously small: {} bytes", bytes.len());
    }

    #[test]
    fn test_app_js_has_mermaid_retry() {
        let js = get("app.js").unwrap();
        assert!(js.contains("mermaid-block"), "app.js missing mermaid-block class");
        assert!(js.contains("doRender"), "app.js missing mermaid doRender retry logic");
    }

    #[test]
    fn test_app_js_has_svg_postprocess() {
        let js = get("app.js").unwrap();
        assert!(js.contains("postprocess"), "app.js missing SVG postprocess hook");
        assert!(js.contains(r"<\/svg>"), "app.js SVG postprocess regex missing");
    }

    #[test]
    fn test_app_js_has_mermaid_block_renderer() {
        let js = get("app.js").unwrap();
        assert!(js.contains("mermaid"), "app.js missing mermaid references");
        assert!(js.contains("data-src"), "app.js missing mermaid data-src attribute");
    }

    #[test]
    fn test_favicon_svg_content() {
        let svg = get("favicon.svg").unwrap();
        assert!(svg.contains("<svg"), "favicon.svg is not an SVG");
        assert!(svg.contains("ᚱ") || svg.contains("&#"), "favicon.svg missing rune character");
    }

    #[test]
    fn test_unknown_asset_returns_none() {
        assert!(get("nonexistent.xyz").is_none());
        assert!(get_bytes("nonexistent.xyz").is_none());
    }

    #[test]
    fn test_app_js_has_toggle_functions() {
        let js = get("app.js").unwrap();
        assert!(js.contains("toggleEdit"),    "app.js missing toggleEdit()");
        assert!(js.contains("togglePreview"), "app.js missing togglePreview()");
        assert!(js.contains("applyPanelLayout"), "app.js missing applyPanelLayout()");
        assert!(js.contains("showEdit"),      "app.js missing showEdit state");
        assert!(js.contains("showPreview"),   "app.js missing showPreview state");
    }

    #[test]
    fn test_app_js_has_split_view() {
        let js = get("app.js").unwrap();
        assert!(js.contains("split-view"), "app.js missing split-view class toggle");
        assert!(js.contains("center-body"), "app.js missing center-body reference");
    }

    #[test]
    fn test_index_html_has_toggle_buttons() {
        let html = get("index.html").unwrap();
        assert!(html.contains("toggleEdit()"),     "index.html missing toggleEdit()");
        assert!(html.contains("togglePreview()"),  "index.html missing togglePreview()");
        assert!(html.contains("center-body"),      "index.html missing center-body div");
        assert!(html.contains("chat-header-right"),"index.html missing chat-header-right");
    }

    #[test]
    fn test_style_has_split_view() {
        let css = get("style.css").unwrap();
        assert!(css.contains("split-view"),  "style.css missing split-view styles");
        assert!(css.contains("center-body"), "style.css missing center-body styles");
    }
}
