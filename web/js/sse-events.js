import './state.js';
globalThis.handleMessage = function handleMessage(msg) {
    switch (msg.type) {
        case 'file_content':
            // Only accept file_content SSE for current note + current file
            // (these come from file mutations like AI edits, not from file/switch)
            if (msg.note_id && msg.note_id !== currentNoteId) break;
            if (msg.filename !== currentFilename) break;
            // If the user has unsaved local edits, the incoming content is a stale
            // echo of a previous save — discard it to prevent deleted/typed chars
            // from reappearing. Remote updates from other users are still applied
            // because they arrive when the user is idle (editorDirty === false).
            if (editorDirty) break;
            specContent = msg.content;
            setEditorValue(msg.content);
            if (showPreview) renderPreview();
            break;
        case 'chat_message':
            addChatMessage(msg.nickname, msg.content);
            break;
        case 'chat_token':
            appendToLastAssistant(msg.content);
            break;
        case 'chat_meta':
            attachMetaToLastAssistant(msg.model, msg.tokens_in, msg.tokens_out, msg.context_tokens, msg.context_window, msg.steps, msg.tool_calls, msg.thinking);
            break;
        case 'chat_done':
            finalizeAssistantMessage();
            removeAllApprovalButtons();
            break;
        case 'status':
            setStatus(msg.state);
            break;
        case 'tool_status':
            if (msg.state === 'start') {
                setToolStatus(msg.tool);
            } else {
                clearToolStatus();
            }
            break;
        case 'file_list':
            // msg.files is now Vec<FileEntry> with {name, public}
            fileList = (msg.files || []).map(f => typeof f === 'string' ? f : f.name);
            // Update per-file visibility for current note in notes array
            {
                const noteEntry = notes.find(n => n.id === currentNoteId);
                if (noteEntry) {
                    noteEntry.files = fileList;
                    noteEntry.fileVisibility = {};
                    (msg.files || []).forEach(f => {
                        if (typeof f === 'object') noteEntry.fileVisibility[f.name] = f.public;
                    });
                    renderNoteList();
                }
            }
            // Don't re-fetch current file on every file_list update — that causes SSE race.
            // Only act when file selection state needs to change.
            if (!currentFilename && fileList.length > 0) {
                // No file selected yet, pick first
                switchFile(fileList[0]);
            } else if (currentFilename && !fileList.includes(currentFilename)) {
                // Current file was deleted — fall back
                if (fileList.length > 0) {
                    switchFile(fileList[0]);
                } else {
                    currentFilename = '';
                    specContent = '';
                    setEditorValue('');
                }
            }
            // If current file still exists, keep showing it (content updates
            // arrive via file_content SSE from actual mutations).
            updateDocTitle(currentFilename);
            try { localStorage.setItem('rune_file', currentFilename); } catch {}
            updateEditorVisibility(fileList.length);
            break;
        case 'file_deleted':
            break;
        case 'archive_done':
            hideArchiveDialog();
            document.getElementById('chat-messages').innerHTML = '';
            addSystemMessage('Archived ' + (msg.count || 0) + ' message(s) → ' + msg.filename);
            break;
        case 'search_results':
            renderSearchResults(msg.query, msg.results || []);
            break;
        case 'auth_result':
            isAdmin = msg.is_admin;
            isGuest = !!msg.is_guest;
            // Set nickname from GitHub login
            if (msg.login) myNickname = msg.login;
            // If auth failed and not intentionally logged out, redirect to login
            if (!msg.ok && !loggedOut) {
                localStorage.removeItem('rune_session_id');
                window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
                break;
            }
            // Rainbow title for admin
            const runeTitle = document.getElementById('rune-title');
            if (runeTitle && isAdmin) {
                runeTitle.classList.add('rune-title-rainbow');
            }
            if (isAdmin) addSystemMessageOnce('You are connected as admin');
            if (isGuest) {
                addSystemMessageOnce('Read-only guest mode');
                // Hide chat input, new-note button, and edit button
                const chatInput = document.getElementById('chat-input');
                if (chatInput) chatInput.closest('.chat-input-area').style.display = 'none';
                const newNoteBtn = document.getElementById('btn-new-note');
                if (newNoteBtn) newNoteBtn.style.display = 'none';
                const editBtn = document.getElementById('btn-edit');
                if (editBtn) editBtn.style.display = 'none';
                document.querySelectorAll('#compact-menu [data-action="toggle-edit"]')
                    .forEach(item => { item.style.display = 'none'; });
            }
            break;
        case 'model_list':
            availableModels = msg.models || [];  // [{id, context_window, reasoning_efforts}, ...]
            activeModel = msg.active || '';
            currentThinking = msg.thinking || 'off';
            updateModelIndicator();
            updateThinkingSelect();
            break;
        case 'model_changed':
            activeModel = msg.model || '';
            currentThinking = msg.thinking || 'off';
            updateModelIndicator();
            updateThinkingSelect();
            addSystemMessage('Model switched to: ' + activeModel + ' ' + currentThinking);
            if (lastContextTokens !== null) {
                const newModel = availableModels.find(m => m.id === activeModel);
                if (newModel && newModel.context_window) {
                    updateContextOverlay(lastContextTokens, newModel.context_window);
                }
            }
            break;
        case 'thinking_changed':
            currentThinking = msg.thinking || 'off';
            updateThinkingSelect();
            addSystemMessage("Model switched to: " + activeModel + " " + currentThinking);
            break;
        case 'note_list':
            // Always rebuild fileVisibility from authoritative public_files in SSE payload.
            // Do NOT preserve stale prevVisibility — that caused visibility toggles from
            // other notes to have no effect on the sidebar.
            notes = msg.notes || [];
            notes.forEach(n => {
                n.fileVisibility = {};
                (n.files || []).forEach(f => { n.fileVisibility[f] = false; });
                (n.public_files || []).forEach(f => { n.fileVisibility[f] = true; });
            });
            if (currentNoteId && !notes.find(s => s.id === currentNoteId)) {
                currentNoteId = '';
            }
            renderNoteList();
            updateChatInputState();
            if (!currentNoteId) {
                const saved = localStorage.getItem('rune_note');
                const target = (saved && notes.find(s => s.id === saved)) ? saved : (notes.length > 0 ? notes[0].id : '');
                if (target) switchNote(target);
            }
            updateDocTitle(currentFilename);
            const newBtn = document.getElementById('btn-new-note');
            if (newBtn && isAdmin) newBtn.classList.remove('hidden');
            break;
        case 'note_switched':
            currentNoteId = msg.note_id;
            updateChatInputState();
            document.getElementById('chat-messages').innerHTML = '';
            renderNoteList();
            updateDocTitle(currentFilename);
            const overlay2 = document.getElementById('context-overlay');
            if (overlay2) overlay2.classList.add('hidden');
            break;
        case 'dir_browse_result':
            renderDirBrowser(msg.path, msg.parent, msg.entries || []);
            break;
        case 'system':
            addPresenceSystemMessage(msg.content);
            break;
        case 'history':
            replayHistory(msg.messages);
            break;
        case 'users_update':
            updateOnlineCount(msg.count);
            break;
        case 'approval_request':
            showApprovalRequest(msg.id, msg.detail);
            break;
        case 'error':
            addSystemMessage('Error: ' + (msg.message || 'Unknown error'));
            finalizeAssistantMessage();
            clearToolStatus();
            setStatus('idle');
            updateChatInputState();
            break;
    }
}
