import './state.js';
// --- File management ---
globalThis.updateDocTitle = function updateDocTitle(name) {
    updatePageTitle();
    const s = notes.find(x => x.id === currentNoteId);
    const noteName = s ? s.name : '';
    const file = (fileList && fileList.length > 0) ? currentFilename : null;

    // Check public visibility for link generation
    const notePublic = s && !!s.public;
    const filePublic = notePublic && file && s.fileVisibility && !!s.fileVisibility[file];

    function noteLink(label) {
        if (!notePublic) return document.createTextNode(label);
        const a = document.createElement('a');
        a.href = '/notes/' + encodeURIComponent(currentNoteId) + '/';
        a.target = '_blank'; a.rel = 'noopener';
        a.className = 'title-public-link';
        a.textContent = label;
        return a;
    }
    function fileLink(label) {
        const slug = (label || '').replace(/\.md$/, '');
        if (!filePublic) return document.createTextNode(label);
        const a = document.createElement('a');
        a.href = '/notes/' + encodeURIComponent(currentNoteId) + '/' + encodeURIComponent(slug);
        a.target = '_blank'; a.rel = 'noopener';
        a.className = 'title-public-link';
        a.textContent = label;
        return a;
    }

    // Build innerHTML fragment for an element
    function buildTitleNodes(el) {
        el.innerHTML = '';
        if (noteName && file) {
            el.appendChild(noteLink(noteName));
            const sep = document.createElement('span');
            sep.innerHTML = ' – ';
            el.appendChild(sep);
            el.appendChild(fileLink(file));
        } else if (noteName) {
            el.appendChild(noteLink(noteName));
        } else if (file) {
            el.appendChild(fileLink(file));
        }
    }

    // Compact top bar
    const compactTitle = document.getElementById('compact-title');
    if (compactTitle) buildTitleNodes(compactTitle);

    // Desktop split-view title bar
    const splitTitle = document.getElementById('split-title');
    if (splitTitle) buildTitleNodes(splitTitle);
};

globalThis.updatePageTitle = function updatePageTitle() {
    if (!currentNoteId) {
        document.title = 'Rune';
        return;
    }
    const s = notes.find(x => x.id === currentNoteId);
    const sessionName = s ? s.name : currentNoteId;
    const file = (fileList && fileList.length > 0) ? currentFilename : null;
    document.title = file
        ? 'Rune - ' + sessionName + ' - ' + file
        : 'Rune - ' + sessionName;
};

globalThis.createFile = async function createFile() {
    const name = await showDialog({ title: 'New File', message: 'Filename must end in .md', input: true, placeholder: 'example.md' });
    if (!name) return;
    if (!name.endsWith('.md')) { addSystemMessage('Error: filename must end in .md'); return; }
    if (!/^[\p{L}\p{N}_\-\.]+\.md$/u.test(name)) { addSystemMessage('Error: invalid filename'); return; }
    api('notes/' + encodeURIComponent(currentNoteId) + '/files', { name });
};

globalThis.deleteCurrentFile = async function deleteCurrentFile() {
    if (!currentFilename) return;
    const ok = await showDialog({ title: 'Delete File', message: 'Delete ' + currentFilename + '?', danger: true });
    if (!ok) return;
    api('notes/' + encodeURIComponent(currentNoteId) + '/files/' + encodeURIComponent(currentFilename), undefined, 'DELETE');
};

globalThis.switchFile = async function switchFile(name) {
    const data = await api('session', { note: currentNoteId, file: name }, 'PUT');
    if (!data || !data.ok) return;
    currentFilename = data.current_file || name;
    specContent = data.file_content || '';
    setEditorValue(specContent);
    // An open file must always have a surface to show it on, otherwise the
    // collapsed center leaves the chat as the only visible panel.
    if (!showEdit && !showPreview) {
        showPreview = true;
        paneFocus = 'preview';
        applyPanelLayout();
    }
    if (showPreview) renderPreview();
    updateDocTitle(currentFilename);
    try { localStorage.setItem('rune_file', currentFilename); } catch {}
    updateBrowserUrl(currentNoteId, currentFilename);
};

globalThis.renameCurrentFile = function renameCurrentFile(newName) {
    if (!currentFilename) return;
    const clean = newName.trim();
    if (!clean || clean === currentFilename) return;
    if (!clean.endsWith('.md')) { addSystemMessage('Error: filename must end in .md'); return; }
    if (!/^[\p{L}\p{N}_\-\.]+\.md$/u.test(clean)) { addSystemMessage('Error: invalid filename'); return; }
    api('notes/' + encodeURIComponent(currentNoteId) + '/files/' + encodeURIComponent(currentFilename), { name: clean }, 'PATCH');
};
