import './state.js';
// --- Note Settings Dialog ---

globalThis.showNoteSettings = function showNoteSettings(sessionId) {
    const s = notes.find(x => x.id === sessionId);
    if (!s) return;
    settingsNoteId = sessionId;
    document.getElementById('note-settings-title').textContent = 'Note: ' + s.name;
    document.getElementById('note-settings-name').value = s.name;
    
    selectedNoteIcon = s.icon || null;
    renderNoteIconTrigger();
    markSelectedEmoji();
    closeEmojiPicker();

    // Hide delete button for default session
    const delBtn = document.getElementById('btn-delete-note');
    if (delBtn) delBtn.style.display = sessionId === 'default' ? 'none' : '';
    document.getElementById('note-settings-modal').classList.remove('hidden');
};

globalThis.hideNoteSettings = function hideNoteSettings() {
    document.getElementById('note-settings-modal').classList.add('hidden');
    settingsNoteId = null;
};

globalThis.saveNoteSettings = function saveNoteSettings() {
    if (!settingsNoteId) return;
    const name = document.getElementById('note-settings-name').value.trim();
    const s = notes.find(x => x.id === settingsNoteId);
    if (s && name) {
        if (name !== s.name || selectedNoteIcon !== (s.icon || null)) {
            api('notes/' + encodeURIComponent(settingsNoteId), { name, icon: selectedNoteIcon }, 'PATCH');
        }
    }
    hideNoteSettings();
};

globalThis.deleteCurrentNote = async function deleteCurrentNote() {
    if (!settingsNoteId) return;
    const ok = await showDialog({ title: 'Delete Note', message: 'Delete this note? Chat history will be preserved.', danger: true, okLabel: 'Delete Note' });
    if (!ok) return;
    const deletedId = settingsNoteId;
    api('notes/' + encodeURIComponent(deletedId), undefined, 'DELETE');
    hideNoteSettings();
    if (currentNoteId === deletedId) {
        // Close current SSE to prevent reconnect loop to deleted note
        if (evtSource) { evtSource.close(); evtSource = null; }
        isConnected = false;
        currentNoteId = '';
        localStorage.removeItem('rune_note');
        updateChatInputState();
        // Switch to another available note
        fetchNoteListAndConnect();
    }
}

// --- Directory Browser ---

globalThis.openDirBrowser = function openDirBrowser(targetInputId) {
    dirBrowserTargetInput = document.getElementById(targetInputId);
    const startPath = dirBrowserTargetInput ? (dirBrowserTargetInput.value || '/') : '/';
    document.getElementById('dir-browser-modal').classList.remove('hidden');
    navigateDir(startPath || '/');
};

globalThis.hideDirBrowser = function hideDirBrowser() {
    document.getElementById('dir-browser-modal').classList.add('hidden');
    dirBrowserTargetInput = null;
};

globalThis.navigateDir = function navigateDir(path) {
    document.getElementById('dir-browser-path').value = path;
    api('dirs?path=' + encodeURIComponent(path), undefined, 'GET').then(r => { if (r.ok && r.data) handleMessage(r.data); });
};

globalThis.renderDirBrowser = function renderDirBrowser(path, parent, entries) {
    document.getElementById('dir-browser-path').value = path;
    const list = document.getElementById('dir-browser-list');
    list.replaceChildren();
    // Parent directory entry
    if (parent) {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        appendDirEntry(el, 'arrow-up', '..', parent);
        list.appendChild(el);
    }
    entries.forEach(e => {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        appendDirEntry(el, 'folder', e.name, path + (path.endsWith('/') ? '' : '/') + e.name);
        list.appendChild(el);
    });
};

globalThis.selectDir = function selectDir() {
    const path = document.getElementById('dir-browser-path').value;
    if (dirBrowserTargetInput) {
        dirBrowserTargetInput.value = path;
    }
    hideDirBrowser();
}

function appendDirEntry(row, iconName, nameText, targetPath) {
    const icon = document.createElement('span');
    icon.className = 'dir-entry-icon';
    icon.appendChild(runeIcon(iconName));
    const name = document.createElement('span');
    name.className = 'dir-entry-name';
    name.textContent = nameText;
    row.role = 'button';
    row.tabIndex = 0;
    row.setAttribute('aria-label', `Open ${nameText}`);
    row.dataset.action = 'navigate-entry';
    row.dataset.path = targetPath;
    row.append(icon, name);
}
