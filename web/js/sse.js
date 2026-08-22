import './state.js';
// --- Connection ---
globalThis.fetchNoteListAndConnect = async function fetchNoteListAndConnect() {
    // Try saved note first
    const saved = localStorage.getItem('rune_note');
    if (saved) { connect(saved); return; }

    // No saved note — fetch list via REST and connect to first available
    try {
        const res = await fetch('/api/notes', { credentials: 'include' });
        const data = await res.json();
        if (data.ok && data.notes && data.notes.length > 0) {
            const firstNote = data.notes[0].id;
            connect(firstNote);
        }
    } catch {}
};

globalThis.connect = function connect(noteId) {
    if (evtSource) { evtSource.close(); evtSource = null; }

    // note_id is required by the server — use provided or current
    const targetNote = noteId || currentNoteId;
    if (!targetNote) {
        // No note selected yet — fetch note list via REST, then connect to first available
        fetchNoteListAndConnect();
        return;
    }

    const params = new URLSearchParams();
    if (myNickname) params.set('nickname', myNickname);
    params.set('note_id', targetNote);

    evtSource = new EventSource('/api/events?' + params.toString(), { withCredentials: true });
    let authFailed = false;

    evtSource.onopen = () => {
        isConnected = true;
        setStatus('idle');
    };

    evtSource.onerror = (e) => {
        isConnected = false;
        setStatus('disconnected');
        if (authFailed) {
            // Don't reconnect on auth failure — show login
            evtSource.close();
            evtSource = null;
            return;
        }
        console.error('SSE error:', e);
        addSystemMessage('Stream interrupted — retrying…');
        if (evtSource && evtSource.readyState === EventSource.CLOSED) {
            evtSource.close();
            evtSource = null;
            setTimeout(() => {
                if (!isConnected && !authFailed) {
                    connect(currentNoteId);
                }
            }, 3000);
        }
    };

    // Listen for all event types
    const eventTypes = [
        'auth_result', 'model_list', 'note_list', 'note_switched',
        'history', 'file_list', 'file_content', 'file_deleted',
        'chat_token', 'chat_done', 'chat_meta', 'chat_message',
        'status', 'tool_status', 'system', 'users_update', 'error',
        'model_changed', 'thinking_changed', 'approval_request', 'archive_done',
        'search_results', 'dir_browse_result', 'auth_error'
    ];

    eventTypes.forEach(type => {
        evtSource.addEventListener(type, (e) => {
            try {
                const msg = JSON.parse(e.data);
                // Handle auth failure: redirect to login
                if ((msg.type === 'error' || msg.type === 'auth_error') &&
                    msg.message && (msg.message.includes('Authentication') || msg.message.includes('not authenticated'))) {
                    authFailed = true;
                    evtSource.close();
                    evtSource = null;
                    isConnected = false;
                    localStorage.removeItem('rune_session_id');
                    window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
                    return;
                }
                // Handle note not found: stop reconnect, switch to another note
                if (msg.type === 'error' && msg.message && (msg.message.includes('Note not found') || msg.message.includes('note_id is required'))) {
                    if (evtSource) { evtSource.close(); evtSource = null; }
                    isConnected = false;
                    currentNoteId = '';
                    localStorage.removeItem('rune_note');
                    addSystemMessage('Note was deleted. Switching...');
                    fetchNoteListAndConnect();
                    return;
                }
                // Handle guest access to private note: clear saved note, switch to a visible one
                if (msg.type === 'auth_error' && msg.message && msg.message.includes('private')) {
                    if (evtSource) { evtSource.close(); evtSource = null; }
                    isConnected = false;
                    currentNoteId = '';
                    localStorage.removeItem('rune_note');
                    addSystemMessage('This note is private. Switching to an accessible note...');
                    fetchNoteListAndConnect();
                    return;
                }
                handleMessage(msg);
            } catch(err) {
                console.error('Parse error:', err, e.data);
            }
        });
    });
}
