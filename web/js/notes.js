import './state.js';
globalThis.switchNote = async function switchNote(sessionId, forceFile = null) {
    if (sessionId === currentNoteId) return;
    currentNoteId = sessionId;
    localStorage.setItem('rune_note', sessionId);
    renderNoteList();
    updateChatInputState();
    updatePageTitle();

    // Close existing SSE immediately (stop receiving events from old room)
    if (evtSource) { evtSource.close(); evtSource = null; }

    const data = await api('session', { note: sessionId }, 'PUT');
    if (!data || !data.ok) return;

    // Update active model for this note
    if (data.current_model) {
        activeModel = data.current_model;
        updateModelIndicator();
    }

    // Replay history from response
    document.getElementById('chat-messages').innerHTML = '';
    currentAssistantEl = null;
    currentAssistantText = '';
    currentAssistantDiv = null;
    if (data.history && data.history.length) {
        replayHistory(data.history);
    }

    // Reconnect SSE AFTER history replay — streaming recovery tokens
    // (if AI task is mid-stream) will append correctly to chat area
    connect(sessionId);

    // Update file list
    fileList = data.files || [];
    updateEditorVisibility(fileList.length);

    // File priority: forceFile (from direct click) > savedFile > server default
    const savedFile = localStorage.getItem('rune_file');
    const preferredFile = (savedFile && fileList.includes(savedFile)) ? savedFile : null;
    const targetFile = (forceFile && fileList.includes(forceFile))
        ? forceFile
        : (preferredFile || data.current_file);

    if (targetFile && fileList.includes(targetFile)) {
        currentFilename = targetFile;
        // If not the one server sent, fetch it
        if (targetFile !== data.current_file || data.file_content === undefined) {
            const fileData = await api('session', { note: sessionId, file: targetFile }, 'PUT');
            specContent = (fileData && fileData.file_content) || '';
        } else {
            specContent = data.file_content || '';
        }
        updateDocTitle(currentFilename);
        renderPreview();
        setEditorValue(specContent);
    } else {
        currentFilename = '';
        specContent = '';
        updateDocTitle('');
    }
    try { localStorage.setItem('rune_file', currentFilename); } catch {}
    updateBrowserUrl(currentNoteId, currentFilename);
};

globalThis.showNewNoteDialog = function showNewNoteDialog() {
    document.getElementById('new-note-modal').classList.remove('hidden');
    document.getElementById('new-note-name').value = '';
    document.getElementById('new-note-name').focus();
};

globalThis.hideNewNoteDialog = function hideNewNoteDialog() {
    document.getElementById('new-note-modal').classList.add('hidden');
};

globalThis.createNote = function createNote() {
    const name = document.getElementById('new-note-name').value.trim();
    if (!name) return;
    api('notes', { name }).then(() => switchNote(name));
    hideNewNoteDialog();
}
