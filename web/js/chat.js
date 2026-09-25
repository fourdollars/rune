import './state.js';
// --- Chat ---
globalThis.sendMessage = function sendMessage() {
    const text = chatInput.value.trim();
    if (!text || !isConnected || !currentNoteId) return;

    const sessId = typeof currentSessionId !== 'undefined' && currentSessionId ? currentSessionId : 'main';
    // Send to server — do NOT optimistic render; wait for broadcast echo
    api('chat', { note_id: currentNoteId, session_id: sessId, content: text, nickname: myNickname });
    chatInput.value = '';
    chatInput.style.height = 'auto';
};

globalThis.updateSessionButton = function updateSessionButton() {
    const btn = document.getElementById('session-btn');
    const iconEl = document.getElementById('session-btn-icon');
    const labelEl = document.getElementById('session-btn-label');
    if (!btn || !labelEl) return;

    const sess = (typeof currentSessionId !== 'undefined' && currentSessionId) ? currentSessionId : 'main';
    let icon = '#';
    let label = sess;
    if (sess.startsWith('user:')) {
        icon = '👤';
        label = sess.slice(5);
    } else if (sess.startsWith('group:')) {
        icon = '👥';
        label = sess.slice(6);
    } else if (sess === 'main') {
        icon = '💬';
        label = 'main';
    }

    const meta = (Array.isArray(sessionsMeta)) ? sessionsMeta.find(m => m.session_id === sess) : null;
    if (meta && meta.custom_title && meta.custom_title.trim()) {
        label = meta.custom_title.trim();
    } else if (meta && meta.title && meta.title.trim()) {
        label = meta.title.trim();
    }

    if (iconEl) iconEl.textContent = icon;
    labelEl.textContent = label;
    btn.title = `Current session: ${label} (${sess}) (Click to manage sessions)`;

    const hasUnread = typeof unreadSessions !== 'undefined' && unreadSessions && unreadSessions.size > 0;
    if (hasUnread) {
        btn.classList.add('has-unread');
    } else {
        btn.classList.remove('has-unread');
    }
};

globalThis.renderSessionBar = function renderSessionBar() {
    updateSessionButton();
};

globalThis.showSessionModal = async function showSessionModal() {
    const modal = document.getElementById('session-modal');
    if (!modal) return;
    modal.classList.remove('hidden');
    const searchInput = document.getElementById('session-modal-search-input');
    if (searchInput) {
        searchInput.value = '';
        searchInput.focus();
    }
    renderSessionModal();
    if (typeof hydrateIcons === 'function') {
        hydrateIcons(modal);
    }

    if (currentNoteId) {
        try {
            const data = await api('session', { note: currentNoteId, session_id: currentSessionId || 'main' }, 'PUT');
            if (data && data.ok) {
                if (Array.isArray(data.sessions)) sessions = data.sessions;
                if (Array.isArray(data.sessions_meta)) sessionsMeta = data.sessions_meta;
                if (!modal.classList.contains('hidden')) {
                    renderSessionModal(searchInput?.value || '');
                }
            }
        } catch (_) {}
    }
};

globalThis.hideSessionModal = function hideSessionModal() {
    const modal = document.getElementById('session-modal');
    if (modal) modal.classList.add('hidden');
};

globalThis.renderSessionModal = function renderSessionModal(filter = '') {
    const listEl = document.getElementById('session-modal-list');
    if (!listEl) return;
    listEl.replaceChildren();

    const sessList = (Array.isArray(sessions) && sessions.length > 0) ? sessions : ['main'];
    const normalizedFilter = (filter || '').toLowerCase().trim();
    const filteredSessions = sessList.filter(s => {
        if (!normalizedFilter) return true;
        const meta = (Array.isArray(sessionsMeta)) ? sessionsMeta.find(m => m.session_id === s) : null;
        const customTitle = (meta?.custom_title || '').toLowerCase();
        const title = (meta?.title || '').toLowerCase();
        return s.toLowerCase().includes(normalizedFilter) || customTitle.includes(normalizedFilter) || title.includes(normalizedFilter);
    });

    if (filteredSessions.length === 0) {
        const emptyDiv = document.createElement('div');
        emptyDiv.className = 'session-card-empty';
        emptyDiv.textContent = 'No matching sessions found';
        listEl.appendChild(emptyDiv);
        return;
    }

    filteredSessions.forEach(sess => {
        const isCurrent = sess === currentSessionId;
        const meta = (Array.isArray(sessionsMeta)) ? sessionsMeta.find(m => m.session_id === sess) : null;
        const row = document.createElement('div');
        row.className = `session-row ${isCurrent ? 'is-active' : ''}`;
        row.dataset.session = sess;

        // Click row to switch session and close modal
        const mainArea = document.createElement('div');
        mainArea.className = 'session-row-main';
        mainArea.dataset.action = 'switch-session-from-modal';
        mainArea.dataset.session = sess;

        let icon = '#';
        let defaultLabel = sess;
        if (sess.startsWith('user:')) {
            icon = '👤';
            defaultLabel = sess.slice(5);
        } else if (sess.startsWith('group:')) {
            icon = '👥';
            defaultLabel = sess.slice(6);
        } else if (sess === 'main') {
            icon = '💬';
            defaultLabel = 'main';
        }

        const autoTitle = (meta && meta.title && meta.title.trim()) ? meta.title.trim() : defaultLabel;
        const displayTitle = (meta && meta.custom_title && meta.custom_title.trim()) ? meta.custom_title.trim() : autoTitle;

        const iconEl = document.createElement('span');
        iconEl.className = 'session-row-icon';
        iconEl.textContent = icon;
        mainArea.appendChild(iconEl);

        const infoEl = document.createElement('div');
        infoEl.className = 'session-row-info';

        const titleEl = document.createElement('span');
        titleEl.className = 'session-row-title';
        titleEl.textContent = displayTitle;
        titleEl.title = 'Click text to rename (clear to reset)';

        // Click on title directly to enter inline edit mode
        titleEl.addEventListener('click', (e) => {
            e.stopPropagation();
            if (titleEl.querySelector('input')) return;

            titleEl.classList.add('is-editing');
            const input = document.createElement('input');
            input.type = 'text';
            input.className = 'session-row-title-input';
            input.value = (meta && meta.custom_title) ? meta.custom_title : '';
            input.placeholder = autoTitle;

            const originalDisplay = titleEl.textContent;
            titleEl.replaceChildren(input);
            input.focus();
            input.select();

            let finished = false;
            const finishEdit = async (save) => {
                if (finished) return;
                finished = true;
                titleEl.classList.remove('is-editing');
                const val = input.value.trim();
                if (save) {
                    await api('chat/session/rename', {
                        note_id: currentNoteId,
                        session_id: sess,
                        title: val || null
                    });
                    if (Array.isArray(sessionsMeta)) {
                        let m = sessionsMeta.find(item => item.session_id === sess);
                        if (!m) {
                            m = { session_id: sess, note_id: currentNoteId };
                            sessionsMeta.push(m);
                        }
                        m.custom_title = val || undefined;
                    }
                    updateSessionButton();
                    renderSessionModal(document.getElementById('session-modal-search-input')?.value || '');
                } else {
                    titleEl.textContent = originalDisplay;
                }
            };

            input.addEventListener('click', (ev) => ev.stopPropagation());
            input.addEventListener('keydown', (ev) => {
                if (ev.key === 'Enter') {
                    ev.preventDefault();
                    finishEdit(true);
                } else if (ev.key === 'Escape') {
                    ev.preventDefault();
                    finishEdit(false);
                }
            });
            input.addEventListener('blur', () => {
                finishEdit(true);
            });
        });

        infoEl.appendChild(titleEl);

        if (sess !== 'main' && displayTitle !== sess) {
            const subtitleEl = document.createElement('span');
            subtitleEl.className = 'session-row-subtitle';
            subtitleEl.textContent = sess;
            infoEl.appendChild(subtitleEl);
        }

        mainArea.appendChild(infoEl);

        if (isCurrent) {
            const badge = document.createElement('span');
            badge.className = 'session-row-badge active';
            badge.textContent = 'Active';
            mainArea.appendChild(badge);
        } else if (typeof unreadSessions !== 'undefined' && unreadSessions && unreadSessions.has && unreadSessions.has(sess)) {
            const badge = document.createElement('span');
            badge.className = 'session-row-badge unread';
            badge.textContent = 'Unread';
            mainArea.appendChild(badge);
        }

        row.appendChild(mainArea);

        // Delete icon button (for non-main sessions)
        if (sess !== 'main') {
            const delBtn = document.createElement('button');
            delBtn.type = 'button';
            delBtn.className = 'session-row-delete';
            delBtn.dataset.action = 'delete-session';
            delBtn.dataset.session = sess;
            delBtn.title = `Delete & archive session "${sess}"`;
            delBtn.setAttribute('aria-label', `Delete session ${sess}`);
            delBtn.dataset.icon = 'trash';
            row.appendChild(delBtn);
        }

        listEl.appendChild(row);
    });

    if (typeof hydrateIcons === 'function') {
        hydrateIcons(listEl);
    }
};

globalThis.promptNewSession = async function promptNewSession() {
    if (!currentNoteId) return;
    const name = await showDialog({
        title: 'New Chat Session',
        message: 'Enter session name (e.g. #research, topic):',
        input: true,
        placeholder: 'research',
        okLabel: 'Create'
    });
    if (!name || !name.trim()) return;

    let cleanName = name.trim();
    if (cleanName.startsWith('#')) cleanName = cleanName.slice(1).trim();
    if (!cleanName) return;

    if (!sessions.includes(cleanName)) {
        sessions.push(cleanName);
    }
    await switchSession(cleanName);
    hideSessionModal();
};

globalThis.deleteSession = async function deleteSession(sessionId) {
    if (!sessionId || !currentNoteId || sessionId === 'main') return;

    const confirmed = await showDialog({
        title: 'Delete Session',
        message: `Delete and archive session "${sessionId}"?`,
        danger: true,
        okLabel: 'Delete'
    });
    if (!confirmed) return;

    const res = await api('chat/archive', { note_id: currentNoteId, session_id: sessionId });
    if (res && res.ok) {
        if (typeof unreadSessions !== 'undefined' && unreadSessions && unreadSessions.delete) {
            unreadSessions.delete(sessionId);
        }
        sessions = sessions.filter(s => s !== sessionId);
        if (Array.isArray(sessionsMeta)) {
            sessionsMeta = sessionsMeta.filter(m => m.session_id !== sessionId);
        }
        if (!sessions.includes('main')) {
            sessions.unshift('main');
        }
        addSystemMessage(`Session "${sessionId}" deleted and archived`);
        if (currentSessionId === sessionId) {
            await switchSession('main');
        } else {
            updateSessionButton();
            renderSessionModal(document.getElementById('session-modal-search-input')?.value || '');
        }
    }
};

globalThis.createSessionFromModal = globalThis.promptNewSession;
globalThis.archiveSessionFromModal = globalThis.deleteSession;

globalThis.switchSession = async function switchSession(sessionId) {
    if (!sessionId || !currentNoteId) return;
    if (sessionId === currentSessionId) return;
    currentSessionId = sessionId;
    if (typeof unreadSessions !== 'undefined' && unreadSessions && unreadSessions.delete) {
        unreadSessions.delete(sessionId);
    }
    updateSessionButton();

    // Fetch messages and session meta for this session
    const data = await api('session', { note: currentNoteId, session_id: sessionId }, 'PUT');
    if (!data || !data.ok) return;

    if (data.sessions_meta) {
        sessionsMeta = data.sessions_meta;
    }
    if (data.current_model) {
        activeModel = data.current_model;
        updateModelIndicator();
    }
    if (data.current_thinking) {
        currentThinking = data.current_thinking;
        updateThinkingSelect();
    }

    document.getElementById('chat-messages').innerHTML = '';
    currentAssistantEl = null;
    currentAssistantText = '';
    currentAssistantDiv = null;
    if (data.history && data.history.length) {
        replayHistory(data.history);
    }
    updateSessionButton();
};

globalThis.closeSession = globalThis.archiveSessionFromModal;

globalThis.showNewSessionDialog = globalThis.showSessionModal;
globalThis.hideNewSessionDialog = globalThis.hideSessionModal;
globalThis.createSession = globalThis.createSessionFromModal;

globalThis.updateChatInputState = function updateChatInputState() {
    if (!currentNoteId) {
        chatInput.disabled = true;
        chatInput.placeholder = 'Create a session first...';
    } else {
        chatInput.disabled = false;
        chatInput.placeholder = 'Type a message...';
    }
    if (typeof renderSessionBar === 'function') renderSessionBar();
    applyNoNoteLayout();
};

globalThis.applyNoNoteLayout = function applyNoNoteLayout() {
    const panelLeft = document.getElementById('panel-left');
    const panelCenter = document.getElementById('panel-center');
    const panelRight = document.getElementById('panel-right');

    if (!currentNoteId) {
        // No active note: hide Edit/Preview buttons
        updateEditorVisibility(0);
    }

    if (!currentNoteId && notes.length === 0) {
        // Truly no notes: expand note panel fullscreen (desktop only)
        panelCenter.classList.add('hidden');
        panelRight.classList.add('hidden');
        panelLeft.classList.remove('collapsed');
        panelLeft.classList.add('fullscreen');
    } else {
        // Note exists or active: restore normal layout
        panelLeft.classList.remove('fullscreen');
        if (showEdit || showPreview) {
            panelCenter.classList.remove('hidden');
        }
        panelRight.classList.remove('hidden');
    }
};

globalThis.fmtTime = function fmtTime(unixSec) {
    const d = unixSec ? new Date(unixSec * 1000) : new Date();
    const mm  = String(d.getMonth() + 1).padStart(2, '0');
    const dd  = String(d.getDate()).padStart(2, '0');
    const hh  = String(d.getHours()).padStart(2, '0');
    const min = String(d.getMinutes()).padStart(2, '0');
    return `${mm}-${dd} ${hh}:${min}`;
};

globalThis.formatDurationMs = function formatDurationMs(ms) {
    if (ms == null || isNaN(ms) || ms < 0) return '';
    const totalSeconds = Math.round(ms / 1000);
    if (totalSeconds === 0) return '0s';
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;

    let res = '';
    if (hours > 0) res += `${hours}h`;
    if (minutes > 0) res += `${minutes}m`;
    if (seconds > 0 || res === '') res += `${seconds}s`;
    return res;
};

globalThis.addChatMessage = function addChatMessage(nickname, content) {
    const isMe = nickname === myNickname;
    const div = document.createElement('div');
    div.className = `chat-msg ${isMe ? 'user' : 'other'}`;

    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.className = 'sender-name';
    nameSpan.append(runeIcon('user'), document.createTextNode(isMe ? `${nickname} (you)` : nickname));
    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    timeSpan.textContent = fmtTime(null);
    sender.appendChild(nameSpan);
    sender.appendChild(timeSpan);

    const body = document.createElement('div');
    body.className = 'body';
    if (typeof marked !== 'undefined') {
        body.replaceChildren(markdownFragment(content));
        if (typeof renderChatMath === 'function') renderChatMath(body);
        if (typeof renderMermaidBlocks === 'function') renderMermaidBlocks(body);
        if (typeof attachCodeCopyButtons === 'function') attachCodeCopyButtons(body);
    } else {
        body.textContent = content;
    }

    div.appendChild(sender);
    div.appendChild(body);
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}


// --- Generic Dialog (replaces native prompt/confirm) ---
globalThis.showDialog = function showDialog({ title, message, input, inputValue, placeholder, danger, okLabel }) {
    return new Promise((resolve) => {
        const modal = document.getElementById('generic-dialog-modal');
        const titleEl = document.getElementById('generic-dialog-title');
        const msgEl = document.getElementById('generic-dialog-message');
        const inputGroup = document.getElementById('generic-dialog-input-group');
        const inputEl = document.getElementById('generic-dialog-input');
        const okBtn = document.getElementById('generic-dialog-ok');
        const dangerBtn = document.getElementById('generic-dialog-danger');
        const cancelBtn = document.getElementById('generic-dialog-cancel');

        titleEl.textContent = title || 'Confirm';
        msgEl.textContent = message || '';
        msgEl.style.display = message ? '' : 'none';

        if (input) {
            inputGroup.style.display = '';
            inputEl.value = inputValue || '';
            inputEl.placeholder = placeholder || '';
            inputEl.focus();
        } else {
            inputGroup.style.display = 'none';
        }

        if (danger) {
            okBtn.style.display = 'none';
            dangerBtn.style.display = '';
            dangerBtn.textContent = okLabel || 'Delete';
        } else {
            okBtn.style.display = '';
            dangerBtn.style.display = 'none';
            okBtn.textContent = okLabel || 'OK';
        }

        function cleanup() {
            modal.classList.add('hidden');
            okBtn.onclick = null;
            dangerBtn.onclick = null;
            cancelBtn.onclick = null;
            inputEl.onkeydown = null;
        }

        okBtn.onclick = () => { cleanup(); resolve(input ? inputEl.value.trim() : true); };
        dangerBtn.onclick = () => { cleanup(); resolve(input ? inputEl.value.trim() : true); };
        cancelBtn.onclick = () => { cleanup(); resolve(input ? null : false); };
        inputEl.onkeydown = (e) => { if (e.key === 'Enter') { cleanup(); resolve(inputEl.value.trim()); } };

        modal.classList.remove('hidden');
        if (input) setTimeout(() => inputEl.focus(), 50);
    });
};

globalThis.addSystemMessage = function addSystemMessage(content) {
    const last = chatMessages.lastElementChild;
    if (last && last.classList.contains('system') && last.textContent === content) return;
    const div = document.createElement('div');
    div.className = 'chat-msg system';
    div.style.color = 'var(--text-muted)';
    div.style.fontSize = '11px';
    div.style.textAlign = 'center';
    div.textContent = content;
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

const PRESENCE_NOTICE = /^(.+) (?:joined|left)$/;

// api.rs broadcasts join/left to the whole room including the joiner, and the
// startup path opens the stream more than once — so without this filter a
// client is told about its own presence several times per page load.
globalThis.addPresenceSystemMessage = function addPresenceSystemMessage(content) {
    const who = PRESENCE_NOTICE.exec(content)?.[1];
    if (who && who === myNickname) return;
    addSystemMessage(content);
}

globalThis.addSystemMessageOnce = function addSystemMessageOnce(content) {
    const shown = [...chatMessages.querySelectorAll('.chat-msg.system')]
        .some(node => node.textContent === content);
    if (!shown) addSystemMessage(content);
}
