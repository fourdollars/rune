import './state.js';
globalThis.showArchiveDialog = function showArchiveDialog() {
    document.getElementById('archive-modal').classList.remove('hidden');
};
globalThis.hideArchiveDialog = function hideArchiveDialog() {
    document.getElementById('archive-modal').classList.add('hidden');
};
globalThis.confirmArchive = function confirmArchive() {
    const sessId = typeof currentSessionId !== 'undefined' && currentSessionId ? currentSessionId : 'main';
    api('chat/archive', { note_id: currentNoteId, session_id: sessId });
}

// --- Search ---
globalThis.showSearchDialog = function showSearchDialog() {
    document.getElementById('search-modal').classList.remove('hidden');
    document.getElementById('search-input').focus();
};
globalThis.hideSearchDialog = function hideSearchDialog() {
    document.getElementById('search-modal').classList.add('hidden');
};
globalThis.doSearch = function doSearch() {
    const q = document.getElementById('search-input').value.trim();
    if (!q) return;
    document.getElementById('search-results').innerHTML = '<div class="search-loading">Searching…</div>';
    api('chat/search', { note_id: currentNoteId, query: q });
};
globalThis.renderSearchResults = function renderSearchResults(query, results) {
    const el = document.getElementById('search-results');
    el.replaceChildren();
    if (!results || !results.length) {
        const empty = document.createElement('div');
        empty.className = 'search-empty';
        empty.textContent = `No results for "${query}"`;
        el.appendChild(empty);
        return;
    }
    // Sort results newest first (most recent on top, oldest at the bottom)
    results.sort((a, b) => (b.created_at || 0) - (a.created_at || 0) || (b.id || 0) - (a.id || 0));
    const count = document.createElement('div');
    count.className = 'search-count';
    count.textContent = `${results.length} result(s)`;
    el.appendChild(count);
    results.forEach((r, i) => {
        const ts = (typeof fmtTime === 'function') ? fmtTime(r.created_at || null) : new Date(r.created_at * 1000).toLocaleString();
        const item = document.createElement('div');
        item.className = 'search-item';
        const meta = document.createElement('div');
        meta.className = 'search-meta';
        const role = document.createElement('span');
        role.className = 'search-role';
        role.appendChild(runeIcon(r.role === 'assistant' ? 'bot' : 'user'));
        role.title = r.role === 'assistant' ? 'Assistant' : 'User';
        const name = document.createElement('strong');
        name.textContent = r.nickname;
        const time = document.createElement('span');
        time.className = 'search-time';
        time.textContent = ts;
        meta.append(role, name, time);

        const copy = document.createElement('button');
        copy.type = 'button';
        copy.className = 'search-copy-btn';
        copy.title = 'Copy message';
        copy.setAttribute('aria-label', 'Copy message');
        copy.appendChild(runeIcon('copy'));
        copy.dataset.action = 'copy-search';
        copy.dataset.content = r.content;
        meta.append(copy);

        if (r.archive_file) {
            const restore = document.createElement('button');
            restore.type = 'button';
            restore.className = 'search-copy-btn search-restore-btn';
            restore.title = `Restore conversation session (${r.archive_file})`;
            restore.setAttribute('aria-label', restore.title);
            restore.appendChild(runeIcon('rotate-ccw'));
            restore.dataset.action = 'restore-search';
            restore.dataset.archiveFile = r.archive_file;
            meta.append(restore);
        }

        const content = document.createElement('div');
        content.className = 'search-content';
        appendHighlighted(content, r.content, query);
        item.append(meta, content);
        el.appendChild(item);
    });
}

function appendHighlighted(container, content, query) {
    const regex = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
    let cursor = 0;
    for (const match of content.matchAll(regex)) {
        container.append(document.createTextNode(content.slice(cursor, match.index)));
        const mark = document.createElement('mark');
        mark.textContent = match[0];
        container.appendChild(mark);
        cursor = match.index + match[0].length;
    }
    container.append(document.createTextNode(content.slice(cursor)));
};

globalThis.copySearchResult = function copySearchResult(button) {
    navigator.clipboard.writeText(button.dataset.content).then(() => {
        setRuneIcon(button, 'check');
        setTimeout(() => setRuneIcon(button, 'copy'), 1500);
    });
};

globalThis.restoreSearchResult = async function restoreSearchResult(button) {
    const archiveFile = button.dataset.archiveFile;
    const noteId = typeof currentNoteId !== 'undefined' ? currentNoteId : null;
    if (!noteId || !archiveFile) return;

    const confirmed = typeof globalThis.showDialog === 'function'
        ? await globalThis.showDialog({
            title: 'Restore Session',
            message: `Are you sure you want to restore the entire conversation session (${archiveFile}) to the current chat? Current active messages will be automatically backed up.`,
            okLabel: 'Restore',
        })
        : true;

    if (!confirmed) return;

    hideSearchDialog();
    const sessId = typeof currentSessionId !== 'undefined' && currentSessionId ? currentSessionId : 'main';
    api('chat/restore', { note_id: noteId, session_id: sessId, archive_file: archiveFile });
};
globalThis.escapeHtml = function escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

// --- Logout ---
globalThis.showLogoutDialog = function showLogoutDialog() {
    document.getElementById('logout-modal').classList.remove('hidden');
};

globalThis.hideLogoutDialog = function hideLogoutDialog() {
    document.getElementById('logout-modal').classList.add('hidden');
};

globalThis.confirmLogout = function confirmLogout() {
    loggedOut = true;
    // Close SSE
    if (evtSource) { evtSource.close(); evtSource = null; }
    // Clear session from localStorage
    localStorage.removeItem('rune_session_id');
    localStorage.removeItem('rune_nickname');
    // Server clears HttpOnly cookie via /auth/logout redirect
    const path = window.location.pathname;
    const nextParam = (path && path.startsWith('/edit'))
        ? ('?next=' + encodeURIComponent(path + window.location.search))
        : '';
    window.location.href = '/auth/logout' + nextParam;
}
