globalThis.toggleNoteRow = function toggleNoteRow(row) {
    const children = row.closest('.explorer-section').querySelector('.explorer-children');
    const chevron = row.querySelector('.chevron');
    children.classList.toggle('collapsed');
    chevron.classList.toggle('open', !children.classList.contains('collapsed'));
    switchNote(row.dataset.note);
};

// Publishing is the one-way-feeling direction: it makes content readable with
// no authentication at all, so it never happens on a single click.
function confirmPublish(kind, name, extra) {
    return showDialog({
        title: `Publish this ${kind}?`,
        message: `“${name}” becomes readable by anyone who has the link — no sign-in required.${extra || ''} You can make it private again at any time.`,
        okLabel: 'Publish',
    });
}

globalThis.toggleNoteVisibility = async function toggleNoteVisibility(control) {
    const note = notes.find(item => item.id === control.dataset.note);
    if (!note) return;
    const next = !note.public;
    if (next) {
        const count = (note.public_files || []).length;
        const extra = count ? ` ${count} public file(s) inside it become reachable too.` : '';
        if (!(await confirmPublish('note', note.name, extra))) return;
    }
    note.public = next;
    renderNoteList();
    api('notes/' + encodeURIComponent(note.id), { public: next }, 'PATCH');
};

globalThis.createFileForNote = function createFileForNote(button) {
    if (button.dataset.note !== currentNoteId) switchNote(button.dataset.note);
    createFile();
};

globalThis.toggleFileVisibility = async function toggleFileVisibility(control) {
    const note = notes.find(item => item.id === control.dataset.note);
    if (!note) return;
    note.fileVisibility ||= {};
    const file = control.dataset.file;
    const next = !note.fileVisibility[file];
    if (next) {
        const extra = note.public ? '' : ` The note “${note.name}” is still private, so publish it too before the page is reachable.`;
        if (!(await confirmPublish('file', file, extra))) return;
    }
    note.fileVisibility[file] = next;
    renderNoteList();
    api('notes/' + encodeURIComponent(note.id) + '/files/' + encodeURIComponent(file), { public: next }, 'PATCH');
};

globalThis.renameFileFromTree = async function renameFileFromTree(button) {
    const name = await showDialog({ title: 'Rename File', input: true, inputValue: button.dataset.file, placeholder: 'new-name.md' });
    if (name && name !== button.dataset.file) {
        api('notes/' + encodeURIComponent(button.dataset.note) + '/files/' + encodeURIComponent(button.dataset.file), { name }, 'PATCH');
    }
};

globalThis.deleteFileFromTree = async function deleteFileFromTree(button) {
    const ok = await showDialog({ title: 'Delete File', message: 'Delete "' + button.dataset.file + '"?', danger: true });
    if (ok) api('notes/' + encodeURIComponent(button.dataset.note) + '/files/' + encodeURIComponent(button.dataset.file), undefined, 'DELETE');
};

globalThis.switchFileFromTree = function switchFileFromTree(row) {
    if (row.dataset.note !== currentNoteId) switchNote(row.dataset.note, row.dataset.file);
    else switchFile(row.dataset.file);
    document.querySelectorAll('#note-tree .explorer-row').forEach(item => item.classList.remove('active'));
    row.classList.add('active');
    if (!showPreview) {
        showPreview = true;
        applyPanelLayout();
    }
    closeTreeDrawer();
    if (isCompactViewport()) showPane(paneFocus === 'preview' ? 'preview' : 'editor');
};
