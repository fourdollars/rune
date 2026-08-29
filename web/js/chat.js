import './state.js';
// --- Chat ---
globalThis.sendMessage = function sendMessage() {
    const text = chatInput.value.trim();
    if (!text || !isConnected || !currentNoteId) return;

    // Send to server — do NOT optimistic render; wait for broadcast echo
    api('chat', { note_id: currentNoteId, content: text, nickname: myNickname });
    chatInput.value = '';
    chatInput.style.height = 'auto';
};

globalThis.updateChatInputState = function updateChatInputState() {
    if (!currentNoteId) {
        chatInput.disabled = true;
        chatInput.placeholder = 'Create a session first...';
    } else {
        chatInput.disabled = false;
        chatInput.placeholder = 'Type a message...';
    }
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
