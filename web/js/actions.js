const actions = {
    'hide-archive-dialog': () => hideArchiveDialog(),
    'confirm-archive': () => confirmArchive(),
    search: () => doSearch(),
    'hide-search-dialog': () => hideSearchDialog(),
    'hide-logout-dialog': () => hideLogoutDialog(),
    'confirm-logout': () => confirmLogout(),
    'hide-new-note-dialog': () => hideNewNoteDialog(),
    'create-note': () => createNote(),
    'navigate-dir': () => navigateDir(document.getElementById('dir-browser-path').value),
    'hide-dir-browser': () => hideDirBrowser(),
    'select-dir': () => selectDir(),
    'delete-note': () => deleteCurrentNote(),
    'hide-note-settings': () => hideNoteSettings(),
    'save-note-settings': () => saveNoteSettings(),
    'hide-model-dialog': () => hideModelDialog(),
    'show-new-note-dialog': () => showNewNoteDialog(),
    'show-logout-dialog': () => showLogoutDialog(),
    'toggle-edit': () => toggleEdit(),
    'toggle-sync-scroll': () => toggleSyncScroll(),
    'toggle-preview': () => togglePreview(),
    'toggle-chat': () => toggleChatPanel(),
    format: element => insertFormat(element.dataset.format),
    'show-model-dialog': () => showModelDialog(),
    'switch-thinking': element => switchThinking(element.value),
    'show-search-dialog': () => showSearchDialog(),
    'show-archive-dialog': () => showArchiveDialog(),
    'send-message': () => sendMessage(),
    'respond-approval': element => respondApproval(element.dataset.id, element.dataset.approved === 'true'),
    'copy-code': element => copyCodeBlock(element),
    'copy-search': element => copySearchResult(element),
    'switch-model': element => { switchModel(element.dataset.model); hideModelDialog(); },
    'toggle-note': element => toggleNoteRow(element),
    'toggle-note-visibility': element => toggleNoteVisibility(element),
    'create-file': element => createFileForNote(element),
    'note-settings': element => showNoteSettings(element.dataset.note),
    'toggle-file-visibility': element => toggleFileVisibility(element),
    'rename-file': element => renameFileFromTree(element),
    'delete-file': element => deleteFileFromTree(element),
    'switch-file': element => switchFileFromTree(element),
    'emoji-category': element => scrollEmojiCategory(element.dataset.category),
    'select-emoji': element => selectEmoji(element.dataset.emoji || null),
    'toggle-emoji-picker': () => toggleEmojiPicker(),
    'navigate-entry': element => navigateDir(element.dataset.path),
    'toggle-tree-drawer': () => toggleTreeDrawer(),
    'close-tree-drawer': () => closeTreeDrawer(),
    'toggle-compact-menu': () => toggleCompactMenu(),
    'toggle-toolbar-overflow': () => toggleToolbarOverflow(),
    'show-pane': element => showPane(element.dataset.pane),
    'row-menu': element => toggleRowMenu(element),
    'run-command': element => runCommand(element),
};

export function initActions() {
    document.addEventListener('click', event => {
        const target = event.target.closest('[data-action]');
        if (!target || target.tagName === 'SELECT') return;
        const handler = actions[target.dataset.action];
        if (!handler) return;
        event.preventDefault();
        event.stopPropagation();
        handler(target, event);
    });
    document.addEventListener('change', event => {
        const target = event.target.closest('[data-action]');
        if (target && actions[target.dataset.action]) actions[target.dataset.action](target, event);
    });
    document.addEventListener('input', event => {
        if (event.target.id === 'file-search-input') renderNoteList();
        if (event.target.id === 'model-search-input') filterModels(event.target.value);
        if (event.target.id === 'emoji-search-input') filterEmojis(event.target.value.trim().toLowerCase());
    });
    document.addEventListener('keydown', event => {
        if (event.key !== 'Enter') return;
        if (event.target.matches('[data-action="search-input"]')) doSearch();
        if (event.target.matches('[data-action="dir-path"]')) navigateDir(event.target.value);
    });
}
