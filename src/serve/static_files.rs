//! Embedded static files for the WebUI.
//!
//! All assets are compressed with zstd (level 19) at compile time by build.rs
//! and stored as raw bytes in the binary. On first access (LazyLock init),
//! they are decompressed entirely into memory — no files are written to disk.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Expands to the zstd-compressed bytes of a web asset embedded in the binary.
macro_rules! zst_asset {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/web-zst/", $name, ".zst"))
    };
}

fn decompress(data: &'static [u8]) -> Vec<u8> {
    zstd::decode_all(data).expect("embedded asset decompression failed")
}

static ASSETS: LazyLock<HashMap<&'static str, Vec<u8>>> = LazyLock::new(|| {
    let mut m: HashMap<&'static str, Vec<u8>> = HashMap::new();
    m.insert("index.html", decompress(zst_asset!("index.html")));
    m.insert("login.html", decompress(zst_asset!("login.html")));
    m.insert("favicon.svg", decompress(zst_asset!("favicon.svg")));
    for path in [
        "js/actions.js",
        "js/api.js",
        "js/bootstrap.js",
        "js/chat-history.js",
        "js/chat-stream.js",
        "js/chat.js",
        "js/dom.js",
        "js/editor-modes.js",
        "js/editor.js",
        "js/emoji-data.js",
        "js/emoji.js",
        "js/files.js",
        "js/format.js",
        "js/icons.js",
        "js/keyboard.js",
        "js/layout.js",
        "js/main.js",
        "js/markdown.js",
        "js/models.js",
        "js/notes.js",
        "js/palette.js",
        "js/panels.js",
        "js/preview.js",
        "js/responsive.js",
        "js/row-menu.js",
        "js/router.js",
        "js/scroll-sync.js",
        "js/search.js",
        "js/settings.js",
        "js/sse-events.js",
        "js/sse.js",
        "js/state.js",
        "js/status.js",
        "js/tree-actions.js",
        "js/tree.js",
    ] {
        let compressed: &'static [u8] = match path {
            "js/actions.js" => zst_asset!("js/actions.js"),
            "js/api.js" => zst_asset!("js/api.js"),
            "js/bootstrap.js" => zst_asset!("js/bootstrap.js"),
            "js/chat-history.js" => zst_asset!("js/chat-history.js"),
            "js/chat-stream.js" => zst_asset!("js/chat-stream.js"),
            "js/chat.js" => zst_asset!("js/chat.js"),
            "js/dom.js" => zst_asset!("js/dom.js"),
            "js/editor-modes.js" => zst_asset!("js/editor-modes.js"),
            "js/editor.js" => zst_asset!("js/editor.js"),
            "js/emoji-data.js" => zst_asset!("js/emoji-data.js"),
            "js/emoji.js" => zst_asset!("js/emoji.js"),
            "js/files.js" => zst_asset!("js/files.js"),
            "js/format.js" => zst_asset!("js/format.js"),
            "js/icons.js" => zst_asset!("js/icons.js"),
            "js/keyboard.js" => zst_asset!("js/keyboard.js"),
            "js/layout.js" => zst_asset!("js/layout.js"),
            "js/main.js" => zst_asset!("js/main.js"),
            "js/markdown.js" => zst_asset!("js/markdown.js"),
            "js/models.js" => zst_asset!("js/models.js"),
            "js/notes.js" => zst_asset!("js/notes.js"),
            "js/palette.js" => zst_asset!("js/palette.js"),
            "js/panels.js" => zst_asset!("js/panels.js"),
            "js/preview.js" => zst_asset!("js/preview.js"),
            "js/responsive.js" => zst_asset!("js/responsive.js"),
            "js/row-menu.js" => zst_asset!("js/row-menu.js"),
            "js/router.js" => zst_asset!("js/router.js"),
            "js/scroll-sync.js" => zst_asset!("js/scroll-sync.js"),
            "js/search.js" => zst_asset!("js/search.js"),
            "js/settings.js" => zst_asset!("js/settings.js"),
            "js/sse-events.js" => zst_asset!("js/sse-events.js"),
            "js/sse.js" => zst_asset!("js/sse.js"),
            "js/state.js" => zst_asset!("js/state.js"),
            "js/status.js" => zst_asset!("js/status.js"),
            "js/tree.js" => zst_asset!("js/tree.js"),
            "js/tree-actions.js" => zst_asset!("js/tree-actions.js"),
            _ => unreachable!(),
        };
        m.insert(path, decompress(compressed));
    }
    m.insert("style.css", decompress(zst_asset!("style.css")));
    m.insert("marked.min.js", decompress(zst_asset!("marked.min.js")));
    m.insert(
        "highlight.min.js",
        decompress(zst_asset!("highlight.min.js")),
    );
    m.insert(
        "highlight-dark.min.css",
        decompress(zst_asset!("highlight-dark.min.css")),
    );
    m.insert(
        "highlight-light.min.css",
        decompress(zst_asset!("highlight-light.min.css")),
    );
    m.insert("katex.min.css", decompress(zst_asset!("katex.min.css")));
    m.insert("katex.min.js", decompress(zst_asset!("katex.min.js")));
    m.insert(
        "katex-auto-render.min.js",
        decompress(zst_asset!("katex-auto-render.min.js")),
    );
    m.insert("codemirror.css", decompress(zst_asset!("codemirror.css")));
    m.insert("codemirror.js", decompress(zst_asset!("codemirror.min.js")));
    m.insert(
        "codemirror-modes.js",
        decompress(zst_asset!("codemirror-modes.min.js")),
    );
    m.insert(
        "codemirror-markdown.js",
        decompress(zst_asset!("codemirror-markdown.min.js")),
    );
    m.insert("mermaid.min.js", decompress(zst_asset!("mermaid.min.js")));
    m
});

/// Returns the decompressed text content of a static web asset.
pub fn get(path: &str) -> Option<String> {
    ASSETS
        .get(path)
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Returns the decompressed bytes of a static web asset.
pub fn get_bytes(path: &str) -> Option<Vec<u8>> {
    ASSETS.get(path).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_assets_present() {
        assert!(get("index.html").is_some(), "index.html missing");
        assert!(get("js/main.js").is_some(), "js/main.js missing");
        assert!(get("style.css").is_some(), "style.css missing");
        assert!(get("favicon.svg").is_some(), "favicon.svg missing");
        assert!(get("katex.min.css").is_some(), "katex.min.css missing");
        assert!(get("katex.min.js").is_some(), "katex.min.js missing");
        assert!(
            get("katex-auto-render.min.js").is_some(),
            "katex-auto-render.min.js missing"
        );
    }

    #[test]
    fn test_index_loads_module_entry_without_inline_handlers() {
        let html = get("index.html").unwrap();
        assert!(html.contains(r#"type="module" src="/assets/js/main.js"#));
        assert!(!html.contains("onclick="));
        assert!(!html.contains("mobile-header"));
        assert!(!html.contains("mobile-drawer"));
        assert!(!html.contains("mobile-section"));
    }

    #[test]
    fn test_state_and_delegated_actions_modules_present() {
        let state = get("js/state.js").unwrap();
        let actions = get("js/actions.js").unwrap();
        assert!(state.contains("export const store"));
        assert!(state.contains("subscribe"));
        assert!(actions.contains("document.addEventListener('click'"));
        assert!(!actions.contains("rune_mobile_"));
    }

    #[test]
    fn test_codemirror_assets_present() {
        assert!(get("codemirror.css").is_some(), "codemirror.css missing");
        assert!(get("codemirror.js").is_some(), "codemirror.js missing");
        assert!(
            get("codemirror-markdown.js").is_some(),
            "codemirror-markdown.js missing"
        );
    }

    #[test]
    fn test_codemirror_modes_present() {
        let js = get("codemirror-modes.js").expect("codemirror-modes.js missing");
        assert!(
            js.contains("defineSimpleMode"),
            "codemirror-modes.js missing defineSimpleMode addon"
        );
        assert!(
            js.contains("htmlmixed"),
            "codemirror-modes.js missing htmlmixed mode dependency"
        );
        assert!(js.contains("toml"), "codemirror-modes.js missing toml mode");
    }

    #[test]
    fn test_app_js_has_mode_aliases_and_themes() {
        let js = get("js/editor-modes.js").expect("editor modes module missing")
            + &get("js/editor.js").expect("editor module missing");
        assert!(
            js.contains("registerCodeMirrorModes"),
            "app.js must define registerCodeMirrorModes"
        );
        assert!(
            js.contains("editorTheme"),
            "app.js must define editorTheme for dark/light switching"
        );
        assert!(
            js.contains("rune-dark") && js.contains("rune-light"),
            "app.js must reference rune-dark and rune-light themes"
        );
        assert!(
            js.contains("modeInfo"),
            "app.js must populate CodeMirror.modeInfo for findModeByName"
        );
    }

    #[test]
    fn test_katex_assets_valid() {
        let css = get("katex.min.css").unwrap();
        assert!(
            css.contains(".katex"),
            "katex.min.css doesn't contain .katex selector"
        );
        let js = get("katex.min.js").unwrap();
        assert!(
            js.len() > 100_000,
            "katex.min.js suspiciously small: {} bytes",
            js.len()
        );
        let ar = get("katex-auto-render.min.js").unwrap();
        assert!(
            ar.contains("renderMathInElement"),
            "auto-render missing renderMathInElement"
        );
    }

    #[test]
    fn test_binary_assets_present() {
        assert!(
            get_bytes("mermaid.min.js").is_some(),
            "mermaid.min.js missing"
        );
        let bytes = get_bytes("mermaid.min.js").unwrap();
        assert!(
            bytes.len() > 1_000_000,
            "mermaid.min.js suspiciously small: {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn test_app_js_has_mermaid_retry() {
        let js = get("js/preview.js").unwrap();
        assert!(
            js.contains("mermaid-block"),
            "app.js missing mermaid-block class"
        );
        assert!(
            js.contains("doRender"),
            "app.js missing mermaid doRender retry logic"
        );
    }

    #[test]
    fn test_app_js_has_svg_postprocess() {
        let js = get("js/markdown.js").unwrap();
        assert!(
            js.contains("postprocess"),
            "app.js missing SVG postprocess hook"
        );
        assert!(
            js.contains(r"<\/svg>"),
            "app.js SVG postprocess regex missing"
        );
    }

    #[test]
    fn test_app_js_has_mermaid_block_renderer() {
        let js = get("js/markdown.js").unwrap();
        assert!(js.contains("mermaid"), "app.js missing mermaid references");
        assert!(
            js.contains("data-src"),
            "app.js missing mermaid data-src attribute"
        );
    }

    #[test]
    fn test_favicon_svg_content() {
        let svg = get("favicon.svg").unwrap();
        assert!(svg.contains("<svg"), "favicon.svg is not an SVG");
        assert!(
            svg.contains("ᚱ") || svg.contains("&#"),
            "favicon.svg missing rune character"
        );
    }

    #[test]
    fn test_unknown_asset_returns_none() {
        assert!(get("nonexistent.xyz").is_none());
        assert!(get_bytes("nonexistent.xyz").is_none());
    }

    #[test]
    fn test_app_js_has_toggle_functions() {
        let js = get("js/layout.js").unwrap() + &get("js/scroll-sync.js").unwrap();
        assert!(js.contains("toggleEdit"), "app.js missing toggleEdit()");
        assert!(
            js.contains("togglePreview"),
            "app.js missing togglePreview()"
        );
        assert!(
            js.contains("toggleSyncScroll"),
            "app.js missing toggleSyncScroll()"
        );
        assert!(
            js.contains("handleEditorScroll"),
            "app.js missing handleEditorScroll()"
        );
        assert!(
            js.contains("handlePreviewScroll"),
            "app.js missing handlePreviewScroll()"
        );
        assert!(
            js.contains("applyPanelLayout"),
            "app.js missing applyPanelLayout()"
        );
        assert!(js.contains("showEdit"), "app.js missing showEdit state");
        assert!(
            js.contains("showPreview"),
            "app.js missing showPreview state"
        );
        assert!(
            js.contains("syncScrollEnabled"),
            "app.js missing syncScrollEnabled state"
        );
    }

    #[test]
    fn test_app_js_has_split_view() {
        let js = get("js/layout.js").unwrap();
        assert!(
            js.contains("split-view"),
            "app.js missing split-view class toggle"
        );
        assert!(
            js.contains("center-body"),
            "app.js missing center-body reference"
        );
    }

    #[test]
    fn test_index_html_has_toggle_buttons() {
        let html = get("index.html").unwrap();
        assert!(
            html.contains("data-action=\"toggle-edit\""),
            "index.html missing delegated edit toggle"
        );
        assert!(
            html.contains("data-action=\"toggle-preview\""),
            "index.html missing delegated preview toggle"
        );
        assert!(
            html.contains("data-action=\"toggle-sync-scroll\""),
            "index.html missing delegated sync-scroll toggle"
        );
        assert!(
            html.contains("center-body"),
            "index.html missing center-body div"
        );
    }

    #[test]
    fn test_style_has_split_view() {
        let css = get("style.css").unwrap();
        assert!(
            css.contains("split-view"),
            "style.css missing split-view styles"
        );
        assert!(
            css.contains("center-body"),
            "style.css missing center-body styles"
        );
    }

    #[test]
    fn test_app_js_has_status_emoji() {
        let js = get("js/status.js").unwrap();
        assert!(
            js.contains("STATUS_EMOJI"),
            "app.js missing STATUS_EMOJI map"
        );
        assert!(
            js.contains("thinking"),
            "app.js STATUS_EMOJI missing thinking"
        );
        assert!(js.contains("typing"), "app.js STATUS_EMOJI missing typing");
    }

    #[test]
    fn test_app_js_has_file_functions() {
        let js = get("js/files.js").unwrap() + &get("js/state.js").unwrap();
        assert!(js.contains("createFile"), "app.js missing createFile");
        assert!(
            js.contains("deleteCurrentFile"),
            "app.js missing deleteCurrentFile"
        );
        assert!(js.contains("switchFile"), "app.js missing switchFile");
        assert!(
            js.contains("renameCurrentFile"),
            "app.js missing renameCurrentFile"
        );
        assert!(
            js.contains("currentFilename"),
            "app.js missing currentFilename state"
        );
        assert!(js.contains("fileList"), "app.js missing fileList state");
    }

    #[test]
    fn test_app_js_has_file_list_handler() {
        let js = get("js/sse-events.js").unwrap();
        assert!(js.contains("file_list"), "app.js missing file_list handler");
        assert!(
            js.contains("file_content"),
            "app.js missing file_content handler"
        );
    }

    #[test]
    fn test_app_js_has_archive_search_functions() {
        let js = get("js/search.js").unwrap() + &get("js/sse-events.js").unwrap();
        assert!(
            js.contains("showArchiveDialog"),
            "missing showArchiveDialog"
        );
        assert!(js.contains("confirmArchive"), "missing confirmArchive");
        assert!(js.contains("showSearchDialog"), "missing showSearchDialog");
        assert!(js.contains("doSearch"), "missing doSearch");
        assert!(
            js.contains("renderSearchResults"),
            "missing renderSearchResults"
        );
        assert!(js.contains("chat/archive"), "missing chat/archive api call");
        assert!(js.contains("chat/search"), "missing chat/search api call");
        assert!(js.contains("archive_done"), "missing archive_done handler");
        assert!(
            js.contains("search_results"),
            "missing search_results handler"
        );
    }

    #[test]
    fn test_index_html_has_archive_search_ui() {
        let html = get("index.html").unwrap();
        assert!(html.contains("btn-archive"), "missing btn-archive");
        assert!(html.contains("btn-search"), "missing btn-search");
        assert!(html.contains("archive-modal"), "missing archive-modal");
        assert!(html.contains("search-modal"), "missing search-modal");
        assert!(html.contains("search-input"), "missing search-input");
    }

    #[test]
    fn test_app_js_has_logout_functions() {
        let js = get("js/search.js").unwrap();
        assert!(
            js.contains("showLogoutDialog"),
            "app.js missing showLogoutDialog"
        );
        assert!(
            js.contains("hideLogoutDialog"),
            "app.js missing hideLogoutDialog"
        );
        assert!(js.contains("confirmLogout"), "app.js missing confirmLogout");
        assert!(
            js.contains("rune_session_id"),
            "app.js missing rune_session_id localStorage key"
        );
    }

    #[test]
    fn test_index_html_has_logout_ui() {
        let html = get("index.html").unwrap();
        assert!(
            html.contains("logout-modal"),
            "index.html missing logout-modal"
        );
        assert!(html.contains("btn-logout"), "index.html missing btn-logout");
        assert!(
            html.contains("data-action=\"confirm-logout\""),
            "index.html missing delegated confirm logout action"
        );
        assert!(
            html.contains("modal-actions"),
            "index.html missing modal-actions"
        );
    }

    #[test]
    fn test_app_js_has_url_routing() {
        let js = get("js/router.js").unwrap() + &get("js/state.js").unwrap();
        assert!(js.contains("parseNotesUrl"), "app.js missing parseNotesUrl");
        assert!(
            js.contains("updateBrowserUrl"),
            "app.js missing updateBrowserUrl"
        );
        assert!(
            js.contains("_pendingNoteId"),
            "app.js missing _pendingNoteId"
        );
        assert!(js.contains("_pendingFile"), "app.js missing _pendingFile");
        assert!(js.contains("popstate"), "app.js missing popstate listener");
        assert!(
            js.contains("history.pushState"),
            "app.js missing history.pushState"
        );
        assert!(
            js.contains("history.replaceState"),
            "app.js missing history.replaceState for .md redirect"
        );
    }

    #[test]
    fn test_app_js_routing_in_switch_functions() {
        let js = get("js/notes.js").unwrap() + &get("js/files.js").unwrap();
        // switchNote and switchFile must call updateBrowserUrl
        // Check they each contain updateBrowserUrl (not just that it's defined)
        let switch_note_pos = js
            .find("async function switchNote")
            .expect("switchNote missing");
        let switch_file_pos = js
            .find("async function switchFile")
            .expect("switchFile missing");
        let next_fn_after_note = js[switch_note_pos + 20..]
            .find("\nasync function ")
            .map(|p| switch_note_pos + 20 + p)
            .unwrap_or(js.len());
        let next_fn_after_file = js[switch_file_pos + 20..]
            .find("\nasync function ")
            .map(|p| switch_file_pos + 20 + p)
            .unwrap_or(js.len());
        assert!(
            js[switch_note_pos..next_fn_after_note].contains("updateBrowserUrl"),
            "switchNote must call updateBrowserUrl"
        );
        assert!(
            js[switch_file_pos..next_fn_after_file].contains("updateBrowserUrl"),
            "switchFile must call updateBrowserUrl"
        );
    }

    #[test]
    fn test_login_html_present_and_correct() {
        let html = get("login.html").unwrap();
        assert!(html.contains("login-box"), "login.html missing login-box");
        assert!(
            html.contains("github-signin-btn"),
            "login.html missing github-signin-btn"
        );
        assert!(
            html.contains("local-login-form"),
            "login.html missing local-login-form"
        );
        assert!(
            html.contains("/notes/"),
            "login.html missing link to /notes/"
        );
        assert!(
            html.contains("/edit/"),
            "login.html missing redirect to /edit/"
        );
        assert!(
            !html.contains("nickname-modal"),
            "login.html must not use modal pattern"
        );
    }

    #[test]
    fn test_app_js_logout_redirects_to_root() {
        let js = get("js/search.js").unwrap();
        // confirmLogout must redirect to '/' (login page)
        assert!(
            js.contains("window.location.href = '/auth/logout';"),
            "confirmLogout must redirect to /auth/logout"
        );
    }

    // ── Theme tests ──────────────────────────────────────────────────

    #[test]
    fn test_style_css_has_color_scheme() {
        let css = get("style.css").unwrap();
        assert!(
            css.contains("color-scheme: light dark"),
            "style.css :root must declare color-scheme: light dark"
        );
    }

    #[test]
    fn test_style_css_has_light_mode_media_query() {
        let css = get("style.css").unwrap();
        assert!(
            css.contains("prefers-color-scheme: light"),
            "style.css must have @media (prefers-color-scheme: light)"
        );
        // Light mode must override the key colour tokens
        let light_pos = css.find("prefers-color-scheme: light").unwrap();
        let light_block = &css[light_pos..light_pos + 600];
        assert!(
            light_block.contains("--bg-primary"),
            "light mode block must override --bg-primary"
        );
        assert!(
            light_block.contains("--text-primary"),
            "light mode block must override --text-primary"
        );
        assert!(
            light_block.contains("--accent"),
            "light mode block must override --accent"
        );
        assert!(
            light_block.contains("--border"),
            "light mode block must override --border"
        );
    }

    #[test]
    fn test_index_html_has_color_scheme_meta() {
        let html = get("index.html").unwrap();
        assert!(
            html.contains("color-scheme\" content=\"light dark\""),
            "index.html must have <meta name=\"color-scheme\" content=\"light dark\">"
        );
    }

    #[test]
    fn test_index_html_highlight_dark_css_media_scoped() {
        let html = get("index.html").unwrap();
        // highlight-dark.min.css must only load in dark mode
        assert!(
            html.contains("highlight-dark.min.css\" media=\"(prefers-color-scheme: dark)\""),
            "highlight-dark.min.css must have media=(prefers-color-scheme: dark) to avoid overriding light mode code colours"
        );
    }

    #[test]
    fn test_index_html_highlight_light_css_media_scoped() {
        let html = get("index.html").unwrap();
        // highlight-light.min.css must only load in light mode
        assert!(
            html.contains(r#"highlight-light.min.css" media="(prefers-color-scheme: light)""#),
            "highlight-light.min.css must have media=(prefers-color-scheme: light)"
        );
    }

    #[test]
    fn test_static_assets_has_highlight_light_css() {
        assert!(
            get("highlight-light.min.css").is_some(),
            "highlight-light.min.css must be embedded in static assets"
        );
    }

    #[test]
    fn test_login_html_has_color_scheme_meta() {
        let html = get("login.html").unwrap();
        assert!(
            html.contains("color-scheme\" content=\"light dark\""),
            "login.html must have <meta name=\"color-scheme\" content=\"light dark\">"
        );
    }

    #[test]
    fn test_app_js_has_formatting_and_keyboard_shortcuts() {
        let js = get("js/editor.js").unwrap() + &get("js/format.js").unwrap();
        assert!(
            js.contains("insertFormat"),
            "app.js must contain insertFormat function"
        );
        assert!(
            js.contains("extraKeys: {"),
            "app.js must define extraKeys config for CodeMirror"
        );
        assert!(
            js.contains("\"Ctrl-B\": () => insertFormat('bold')"),
            "app.js must bind Ctrl-B"
        );
        assert!(
            js.contains("\"Cmd-B\": () => insertFormat('bold')"),
            "app.js must bind Cmd-B"
        );
        assert!(
            js.contains("\"Ctrl-I\": () => insertFormat('italic')"),
            "app.js must bind Ctrl-I"
        );
        assert!(
            js.contains("\"Cmd-I\": () => insertFormat('italic')"),
            "app.js must bind Cmd-I"
        );
        assert!(
            js.contains("\"Ctrl-H\": () => insertFormat('header')"),
            "app.js must bind Ctrl-H"
        );
        assert!(
            js.contains("\"Cmd-H\": () => insertFormat('header')"),
            "app.js must bind Cmd-H"
        );
        assert!(
            js.contains("\"Ctrl-K\": () => insertFormat('link')"),
            "app.js must bind Ctrl-K"
        );
        assert!(
            js.contains("\"Cmd-K\": () => insertFormat('link')"),
            "app.js must bind Cmd-K"
        );

        // Check for specific formatting types handled in switch
        assert!(js.contains("case 'bold':"), "insertFormat must handle bold");
        assert!(
            js.contains("case 'italic':"),
            "insertFormat must handle italic"
        );
        assert!(
            js.contains("case 'header':"),
            "insertFormat must handle header"
        );
        assert!(js.contains("case 'link':"), "insertFormat must handle link");
        assert!(
            js.contains("case 'image':"),
            "insertFormat must handle image"
        );
        assert!(js.contains("case 'code':"), "insertFormat must handle code");
        assert!(js.contains("case 'ul':"), "insertFormat must handle ul");
        assert!(js.contains("case 'ol':"), "insertFormat must handle ol");
        assert!(js.contains("case 'task':"), "insertFormat must handle task");
        assert!(
            js.contains("case 'table':"),
            "insertFormat must handle table"
        );
    }
}
