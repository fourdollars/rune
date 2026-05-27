// Rune WebUI — app.js
// Three-panel layout: Sessions | spec.md | Chat

'use strict';

// --- State ---
let ws = null;
let currentMode = 'edit'; // 'edit' | 'preview'
let specContent = '';
let isConnected = false;
let editorDirty = false;
let debounceTimer = null;

// --- DOM refs ---
const editor = document.getElementById('editor');
const preview = document.getElementById('preview');
const editorContainer = document.getElementById('editor-container');
const previewContainer = document.getElementById('preview-container');
const chatMessages = document.getElementById('chat-messages');
const chatInput = document.getElementById('chat-input');
const statusIndicator = document.getElementById('status-indicator');
const btnEdit = document.getElementById('btn-edit');
const btnPreview = document.getElementById('btn-preview');

// --- WebSocket ---
function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/ws`;
    ws = new WebSocket(url);

    ws.onopen = () => {
        isConnected = true;
        setStatus('idle');
        addSystemMessage('Connected to Rune');
    };

    ws.onmessage = (event) => {
        try {
            const msg = JSON.parse(event.data);
            handleServerMessage(msg);
        } catch (e) {
            console.error('Invalid message:', e);
        }
    };

    ws.onclose = () => {
        isConnected = false;
        setStatus('disconnected');
        addSystemMessage('Disconnected. Reconnecting...');
        setTimeout(connect, 2000);
    };

    ws.onerror = (e) => {
        console.error('WebSocket error:', e);
    };
}

function handleServerMessage(msg) {
    switch (msg.type) {
        case 'spec_full':
            specContent = msg.content;
            editor.value = specContent;
            if (currentMode === 'preview') renderPreview();
            break;

        case 'spec_patch':
            specContent = msg.content;
            editor.value = specContent;
            if (currentMode === 'preview') renderPreview();
            break;

        case 'chat_token':
            appendToLastAssistant(msg.content);
            break;

        case 'chat_done':
            finalizeAssistantMessage();
            break;

        case 'status':
            setStatus(msg.state);
            break;

        case 'approval_request':
            showApprovalRequest(msg.id, msg.detail);
            break;

        case 'error':
            addSystemMessage(`Error: ${msg.message}`);
            break;
    }
}

// --- Chat ---
function sendMessage() {
    const text = chatInput.value.trim();
    if (!text || !isConnected) return;

    addChatMessage('user', text);
    ws.send(JSON.stringify({ type: 'chat_send', content: text }));
    chatInput.value = '';
    chatInput.style.height = 'auto';
}

function addChatMessage(role, content) {
    const div = document.createElement('div');
    div.className = `chat-msg ${role}`;

    const sender = document.createElement('div');
    sender.className = 'sender';
    sender.textContent = role === 'user' ? '🧑 You' : 'ᚱ Rune';

    const body = document.createElement('div');
    body.className = 'body';
    body.textContent = content;

    div.appendChild(sender);
    div.appendChild(body);
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function addSystemMessage(content) {
    const div = document.createElement('div');
    div.className = 'chat-msg system';
    div.style.color = 'var(--text-muted)';
    div.style.fontSize = '11px';
    div.style.textAlign = 'center';
    div.textContent = content;
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

let currentAssistantEl = null;

function appendToLastAssistant(token) {
    if (!currentAssistantEl) {
        const div = document.createElement('div');
        div.className = 'chat-msg assistant';

        const sender = document.createElement('div');
        sender.className = 'sender';
        sender.textContent = 'ᚱ Rune';

        const body = document.createElement('div');
        body.className = 'body';

        div.appendChild(sender);
        div.appendChild(body);
        chatMessages.appendChild(div);
        currentAssistantEl = body;
    }
    currentAssistantEl.textContent += token;
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function finalizeAssistantMessage() {
    currentAssistantEl = null;
}

function showApprovalRequest(id, detail) {
    const div = document.createElement('div');
    div.className = 'chat-msg assistant';
    div.innerHTML = `
        <div class="sender">🔒 Approval Required</div>
        <div class="body">${detail}</div>
        <div style="margin-top:8px;display:flex;gap:8px">
            <button onclick="respondApproval('${id}',true)" style="background:var(--success);color:#000;border:none;padding:4px 12px;border-radius:4px;cursor:pointer">✓ Allow</button>
            <button onclick="respondApproval('${id}',false)" style="background:var(--error);color:#000;border:none;padding:4px 12px;border-radius:4px;cursor:pointer">✗ Deny</button>
        </div>
    `;
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function respondApproval(id, approved) {
    ws.send(JSON.stringify({ type: 'approval_response', id, approved }));
    addSystemMessage(approved ? `Approved: ${id}` : `Denied: ${id}`);
}

// --- Spec Editor ---
function setMode(mode) {
    currentMode = mode;
    if (mode === 'edit') {
        editorContainer.classList.remove('hidden');
        previewContainer.classList.add('hidden');
        btnEdit.classList.add('active');
        btnPreview.classList.remove('active');
    } else {
        editorContainer.classList.add('hidden');
        previewContainer.classList.remove('hidden');
        btnEdit.classList.remove('active');
        btnPreview.classList.add('active');
        renderPreview();
    }
}

function renderPreview() {
    if (typeof marked !== 'undefined') {
        preview.innerHTML = marked.parse(specContent);
    } else {
        preview.textContent = specContent;
    }
}

// Editor change handling with debounce
editor.addEventListener('input', () => {
    specContent = editor.value;
    editorDirty = true;

    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
        if (editorDirty && isConnected) {
            ws.send(JSON.stringify({ type: 'spec_update', content: specContent }));
            editorDirty = false;
        }
    }, 300);
});

// --- Status ---
function setStatus(state) {
    statusIndicator.className = `status ${state}`;
    statusIndicator.textContent = `● ${state}`;
}

// --- Panel Toggle ---
function togglePanel(side) {
    const panel = document.getElementById(`panel-${side}`);
    panel.classList.toggle('collapsed');

    const icon = panel.querySelector('.toggle-icon');
    if (side === 'left') {
        icon.textContent = panel.classList.contains('collapsed') ? '▶' : '◀';
    } else {
        icon.textContent = panel.classList.contains('collapsed') ? '◀' : '▶';
    }
}

// --- Keyboard shortcuts ---
chatInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
    }
});

// Ctrl+Shift+E → toggle edit/preview
document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key === 'E') {
        e.preventDefault();
        setMode(currentMode === 'edit' ? 'preview' : 'edit');
    }
});

// --- Init ---
connect();
