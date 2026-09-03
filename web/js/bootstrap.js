import './state.js';
// --- Init ---
registerCodeMirrorModes();
initEditor();
initPreviewScrollSync();
initPanelResize();
initEmojiPicker();
// Restore edit/preview state
try {
    const se = localStorage.getItem('rune_show_edit');
    const sp = localStorage.getItem('rune_show_preview');
    if (se !== null) showEdit    = se === '1';
    if (sp !== null) showPreview = sp === '1';
} catch {}
try {
    const val = localStorage.getItem('rune_sync_scroll');
    if (val !== null) {
        syncScrollEnabled = val === '1';
    }
} catch {}
setToggleState(document.getElementById('btn-sync-scroll'), syncScrollEnabled);
applyPanelLayout();
// Session init: verify session via /api/me, then connect, or redirect to login
globalThis.getSessionId = function getSessionId() {
    const ls = localStorage.getItem('rune_session_id');
    if (ls) return ls;
    const match = document.cookie.match(/(?:^|;\s*)rune_session_id=([^;]+)/);
    if (match) {
        try { localStorage.setItem('rune_session_id', match[1]); } catch {}
        return match[1];
    }
    return null;
}

;(async function initSession() {
    const sessionId = getSessionId();
    if (!sessionId) {
        window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
        return;
    }
    try {
        const resp = await fetch('/api/me', { credentials: 'include' });
        const data = resp.ok ? await resp.json() : { ok: false };
        if (data.ok) {
            myNickname = data.login || '';
            isAdmin = data.role === 'admin';
            isGuest = data.role === 'guest';
            // If URL contains a specific note/file, use it as the initial target
            let initialTargetNote = _pendingNoteId;
            let initialTargetFile = _pendingFile ? (_pendingFile.endsWith('.md') ? _pendingFile : _pendingFile + '.md') : null;
            if (_pendingNoteId) {
                localStorage.setItem('rune_note', _pendingNoteId);
                if (_pendingFile) localStorage.setItem('rune_file', initialTargetFile);
                _pendingNoteId = null;
                _pendingFile   = null;
            }
            const savedNote = initialTargetNote || localStorage.getItem('rune_note');
            const savedFile = initialTargetFile || localStorage.getItem('rune_file');
            if (savedNote) {
                switchNote(savedNote, savedFile);
            } else {
                fetchNoteListAndConnect();
            }
        } else {
            localStorage.removeItem('rune_session_id');
            window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
        }
    } catch {
        // Network error — fetch note list and connect
        fetchNoteListAndConnect();
    }
})();
