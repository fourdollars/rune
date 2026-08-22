import './state.js';
// --- URL Routing helpers ---
// Parse /edit/{note}/{file} from pathname; strip trailing .md
globalThis.parseNotesUrl = function parseNotesUrl() {
    const path = window.location.pathname;
    // Strip .md suffix and redirect to clean URL
    if (path.endsWith('.md')) {
        const clean = path.slice(0, -3);
        history.replaceState(null, '', clean);
        return parseNotesUrlFromPath(clean);
    }
    return parseNotesUrlFromPath(path);
};
globalThis.parseNotesUrlFromPath = function parseNotesUrlFromPath(path) {
    const m = path.match(/^\/edit\/([^\/]+)\/([^\/]+)$/);
    if (m) return { noteId: decodeURIComponent(m[1]), file: decodeURIComponent(m[2]) };
    const m2 = path.match(/^\/edit\/([^\/]+)\/?$/);
    if (m2) return { noteId: decodeURIComponent(m2[1]), file: null };
    return { noteId: null, file: null };
};
globalThis.updateBrowserUrl = function updateBrowserUrl(noteId, filename) {
    if (!noteId) return;
    const slug = filename ? filename.replace(/\.md$/, '') : null;
    const url = slug
        ? '/edit/' + encodeURIComponent(noteId) + '/' + encodeURIComponent(slug)
        : '/edit/' + encodeURIComponent(noteId) + '/';
    if (window.location.pathname !== url) {
        history.pushState({ noteId, filename }, '', url);
    }
};

// Pending note/file from URL (set before auth, consumed after login)
;(function initRouting() {
    const parsed = parseNotesUrl();
    if (parsed.noteId) {
        _pendingNoteId = parsed.noteId;
        _pendingFile   = parsed.file;
    }
    // Handle browser back/forward
    window.addEventListener('popstate', (e) => {
        if (e.state && e.state.noteId) {
            if (e.state.noteId !== currentNoteId) {
                switchNote(e.state.noteId, e.state.filename || null);
            } else if (e.state.filename && e.state.filename !== currentFilename) {
                switchFile(e.state.filename);
            }
        }
    });
})();
