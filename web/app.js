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
let specVersion = 0; // track spec updates for flash indicator

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

// --- marked.js configuration ---
if (typeof marked !== 'undefined') {
    marked.setOptions({
        breaks: true,
        gfm: true,
    });
}

// --- WebSocket ---
function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const params = new URLSearchParams(location.search);
    const tokenParam = params.get('token') ? `?token=${params.get('token')}` : '';
    const url = `${proto}//${location.host}/ws${tokenParam}`;
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
            const isAgentUpdate = specVersion > 0 && msg.content !== specContent;
            specContent = msg.content;
            editor.value = specContent;
            if (currentMode === 'preview') renderPreview();
            if (isAgentUpdate) flashSpecIndicator();
            specVersion++;
            break;

        case 'spec_patch':
            specContent = msg.content;
            editor.value = specContent;
            if (currentMode === 'preview') renderPreview();
            flashSpecIndicator();
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
let currentAssistantText = '';

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
        currentAssistantText = '';
    }
    currentAssistantText += token;
    // Render as markdown for rich formatting
    if (typeof marked !== 'undefined') {
        currentAssistantEl.innerHTML = marked.parse(currentAssistantText);
    } else {
        currentAssistantEl.textContent = currentAssistantText;
    }
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function finalizeAssistantMessage() {
    // Final render with markdown
    if (currentAssistantEl && typeof marked !== 'undefined') {
        currentAssistantEl.innerHTML = marked.parse(currentAssistantText);
    }
    currentAssistantEl = null;
    currentAssistantText = '';
}

function showApprovalRequest(id, detail) {
    const div = document.createElement('div');
    div.className = 'chat-msg assistant approval';
    div.innerHTML = `
        <div class="sender">🔒 Approval Required</div>
        <div class="body"><code>${escapeHtml(detail)}</code></div>
        <div style="margin-top:8px;display:flex;gap:8px">
            <button onclick="respondApproval('${id}',true)" class="btn-approve">✓ Allow</button>
            <button onclick="respondApproval('${id}',false)" class="btn-deny">✗ Deny</button>
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
        // Add copy buttons to code blocks
        preview.querySelectorAll('pre code').forEach(block => {
            const btn = document.createElement('button');
            btn.className = 'copy-btn';
            btn.textContent = '📋';
            btn.title = 'Copy';
            btn.onclick = () => {
                navigator.clipboard.writeText(block.textContent);
                btn.textContent = '✓';
                setTimeout(() => btn.textContent = '📋', 1500);
            };
            block.parentElement.style.position = 'relative';
            block.parentElement.appendChild(btn);
        });
    } else {
        preview.textContent = specContent;
    }
}

// Flash indicator when spec is updated by the agent
function flashSpecIndicator() {
    const toolbar = document.querySelector('.toolbar');
    toolbar.classList.add('spec-updated');
    setTimeout(() => toolbar.classList.remove('spec-updated'), 1200);
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

// Tab key support in editor
editor.addEventListener('keydown', (e) => {
    if (e.key === 'Tab') {
        e.preventDefault();
        const start = editor.selectionStart;
        const end = editor.selectionEnd;
        editor.value = editor.value.substring(0, start) + '    ' + editor.value.substring(end);
        editor.selectionStart = editor.selectionEnd = start + 4;
        specContent = editor.value;
    }
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

// --- Utilities ---
function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
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

// Ctrl+Enter → send message (from anywhere)
document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'Enter') {
        e.preventDefault();
        chatInput.focus();
        sendMessage();
    }
});

// --- Init ---
connect();
