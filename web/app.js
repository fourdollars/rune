// Rune WebUI — app.js
// Three-panel layout: Sessions | spec.md | Chat

'use strict';

// --- State ---
let ws = null;
let showEdit    = true;
let showPreview = false;
let currentFilename = 'spec.md';
let fileList = [];
let specContent = '';
let isConnected = false;
let loggedOut   = false;
let editorDirty = false;
let debounceTimer = null;
let specVersion = 0;
let myNickname = '';
let myToken = '';
let isAdmin = false;
let availableModels = [];

// --- Session state ---
let sessions = [];
let currentSessionId = 'default';
let dirBrowserTargetInput = null;
let settingsSessionId = null;

let activeModel = '';

// --- DOM refs ---
const preview = document.getElementById('preview');
const editorContainer = document.getElementById('editor-container');
const previewContainer = document.getElementById('preview-container');
const chatMessages = document.getElementById('chat-messages');
const chatInput = document.getElementById('chat-input');
const statusIndicator = document.getElementById('status-indicator');
const btnEdit = document.getElementById('btn-edit');
const btnPreview = document.getElementById('btn-preview');

// --- Editor highlight: markdown + fenced code block sub-language ---
function highlightMarkdownEditor(text) {
    if (typeof hljs === 'undefined') return escapeHtmlEditor(text);

    // Language aliases
    const langAliases = {
        'bash': 'bash', 'sh': 'bash', 'zsh': 'bash', 'shell': 'bash',
        'js': 'javascript', 'ts': 'typescript',
        'py': 'python', 'rb': 'ruby', 'rs': 'rust',
        'yml': 'yaml', 'html': 'xml', 'svg': 'xml',
        'golang': 'go',
        'jsonc': 'json',
        'toml': 'ini',
    };

    // Split text into segments: outside fence | fence block
    // Regex: capture ``` fence start, content, fence end
    const fenceRe = /^(`{3,})([ \t]*)(\S*)([ \t]*)[\r\n]([\s\S]*?)^\1[ \t]*$/gm;
    let result = '';
    let lastIndex = 0;
    let match;

    while ((match = fenceRe.exec(text)) !== null) {
        const [fullMatch, ticks, , rawLang, , code] = match;
        const start = match.index;
        const end   = start + fullMatch.length;

        // Highlight the markdown section before this fence
        if (start > lastIndex) {
            const mdChunk = text.slice(lastIndex, start);
            result += hljs.highlight(mdChunk, { language: 'markdown', ignoreIllegals: true }).value;
        }

        // Highlight the fence header (```python)
        const langLabel = escapeHtmlEditor(rawLang);
        const ticksEsc  = escapeHtmlEditor(ticks);
        result += `<span class="hljs-meta">${ticksEsc}${langLabel}</span>
`;

        // Highlight the code content with its language
        const lang = langAliases[rawLang.toLowerCase()] || rawLang.toLowerCase();
        let codeHtml;
        if (lang && hljs.getLanguage(lang)) {
            codeHtml = hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
        } else if (lang) {
            // Try autodetect
            codeHtml = hljs.highlightAuto(code).value;
        } else {
            codeHtml = escapeHtmlEditor(code);
        }
        result += codeHtml;

        // Closing fence
        result += `<span class="hljs-meta">${ticksEsc}</span>`;
        lastIndex = end;
    }

    // Remaining markdown after last fence
    if (lastIndex < text.length) {
        const tail = text.slice(lastIndex);
        result += hljs.highlight(tail, { language: 'markdown', ignoreIllegals: true }).value;
    }

    return result;
}

function escapeHtmlEditor(text) {
    return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// --- Editor: textarea + highlight.js overlay ---
function initEditor() {
    const textarea = document.getElementById('editor');
    const hlCode   = document.getElementById('editor-highlight-code');
    if (!textarea || !hlCode) return;

    function syncHighlight() {
        // Append trailing newline so last line renders correctly
        const text = textarea.value.endsWith('\n') ? textarea.value : textarea.value + '\n';
        hlCode.innerHTML = highlightMarkdownEditor(text);
    }

    function syncScroll() {
        const pre = document.getElementById('editor-highlight');
        if (pre) {
            pre.scrollTop  = textarea.scrollTop;
            pre.scrollLeft = textarea.scrollLeft;
        }
    }

    textarea.addEventListener('input', () => {
        specContent = textarea.value;
        editorDirty = true;
        syncHighlight();
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            // Live preview update
            if (showPreview) renderPreview();
            // Sync to server
            if (editorDirty && isConnected) {
                ws.send(JSON.stringify({ type: 'spec_update', content: specContent, filename: currentFilename }));
                editorDirty = false;
            }
        }, 300);
    });

    textarea.addEventListener('scroll', () => {
        syncScroll(); // sync highlight overlay
        syncEditorToPreview(textarea);
    });

    // Tab key
    textarea.addEventListener('keydown', e => {
        if (e.key === 'Tab') {
            e.preventDefault();
            const s = textarea.selectionStart;
            const v = textarea.value;
            textarea.value = v.substring(0, s) + '    ' + v.substring(textarea.selectionEnd);
            textarea.selectionStart = textarea.selectionEnd = s + 4;
            specContent = textarea.value;
            syncHighlight();
        }
    });

    syncHighlight();
}

function setEditorValue(text) {
    const textarea = document.getElementById('editor');
    const hlCode   = document.getElementById('editor-highlight-code');
    if (!textarea) return;
    textarea.value = text;
    if (hlCode && typeof hljs !== 'undefined') {
        const t = text.endsWith('\n') ? text : text + '\n';
        hlCode.innerHTML = highlightMarkdownEditor(t);
    }
}


// --- marked.js configuration (v15+) ---
if (typeof marked !== 'undefined') {
    // Custom renderer: syntax-highlight code blocks with highlight.js
    const renderer = new marked.Renderer();
    renderer.code = function({ text, lang }) {
        // Mermaid: output a special div, rendered later
        if (lang && lang.toLowerCase() === 'mermaid') {
            const id = 'mermaid-' + Math.random().toString(36).slice(2);
            return `<div class="mermaid-block" id="${id}" data-src="${text.replace(/"/g,'&quot;')}"></div>`;
        }
        const raw = text.replace(/"/g, '&quot;'); // for data attribute
        if (typeof hljs !== 'undefined') {
            const language = lang && hljs.getLanguage(lang) ? lang : null;
            const highlighted = language
                ? hljs.highlight(text, { language }).value
                : hljs.highlightAuto(text).value;
            const langClass = language ? ` class="language-${language}"` : '';
            return `<pre class="hljs-pre" data-raw="${raw}"><code class="hljs${langClass}">${highlighted}</code></pre>`;
        }
        const safe = text.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        return `<pre class="hljs-pre" data-raw="${raw}"><code>${safe}</code></pre>`;
    };
    // hooks: unwrap <svg> mistakenly wrapped in <p>
    const hooks = {
        postprocess(html) {
            // <p><svg ...>...</svg></p>  →  <svg ...>...</svg>
            return html.replace(/<p>(\s*<svg[\s\S]*?<\/svg>\s*)<\/p>/gi, '$1');
        }
    };
    marked.use({ renderer, hooks, breaks: true, gfm: true });
}

// --- Nickname Modal ---
function submitNickname() {
    const input = document.getElementById('nickname-input');
    const tokenInput = document.getElementById('token-input');
    const name = (input.value || '').trim();
    if (!name) {
        input.focus();
        input.style.borderColor = 'var(--error)';
        setTimeout(() => input.style.borderColor = '', 800);
        return;
    }
    myNickname = name;
    myToken = tokenInput ? (tokenInput.value || '').trim() : '';
    // Persist to localStorage
    try {
        localStorage.setItem('rune_nickname', myNickname);
        if (myToken) localStorage.setItem('rune_token', myToken);
        else localStorage.removeItem('rune_token');
    } catch {}
    document.getElementById('nickname-modal').classList.add('hidden');
    loggedOut = false;
    connect();
}

function loadStoredCredentials() {
    try {
        const savedNick = localStorage.getItem('rune_nickname');
        const savedToken = localStorage.getItem('rune_token');
        if (savedNick) {
            const input = document.getElementById('nickname-input');
            if (input) input.value = savedNick;
        }
        if (savedToken) {
            const tokenInput = document.getElementById('token-input');
            if (tokenInput) tokenInput.value = savedToken;
        }
        // Auto-join if both saved
        if (savedNick) {
            submitNickname();
        }
    } catch {}
}

// Enter key on nickname input
document.getElementById('nickname-input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') submitNickname();
});

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
        // Send nickname as first message
        ws.send(JSON.stringify({ type: 'set_nickname', name: myNickname, token: myToken || undefined }));
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
        if (!loggedOut) {
            addSystemMessage('Disconnected. Reconnecting...');
            setTimeout(connect, 2000);
        }
    };

    ws.onerror = (e) => {
        console.error('WebSocket error:', e);
    };
}

function handleServerMessage(msg) {
    switch (msg.type) {
        case 'spec_full': {
            const isAgentUpdate = specVersion > 0 && msg.content !== specContent;
            if (msg.content !== specContent) {
                specContent = msg.content;
                setEditorValue(specContent);
                if (showPreview) renderPreview();
            }
            if (isAgentUpdate) flashSpecIndicator();
            specVersion++;
            break;
        }
        case 'spec_patch': {
            if (msg.content !== specContent) {
                specContent = msg.content;
                setEditorValue(specContent);
                if (showPreview) renderPreview();
                flashSpecIndicator();
            }
            break;
        }

        case 'chat_message':
            // Broadcast chat message from a user
            addChatMessage(msg.nickname, msg.content);
            break;

        case 'chat_token':
            appendToLastAssistant(msg.content);
            break;

        case 'chat_meta':
            attachMetaToLastAssistant(msg.model, msg.tokens_in, msg.tokens_out, msg.context_tokens, msg.context_window);
            break;

        case 'chat_done':
            finalizeAssistantMessage();
            removeAllApprovalButtons();
            break;

        case 'status':
            setStatus(msg.state);
            break;

        case 'file_list':
            fileList = msg.files || [];
            currentFilename = msg.active || 'spec.md';
            updateDocTitle(currentFilename);
            break;

        case 'file_content':
            currentFilename = msg.filename;
            specContent = msg.content;
            setEditorValue(msg.content);
            if (showPreview) renderPreview();
            updateDocTitle(msg.filename);
            break;

        case 'file_deleted':
            // file_list will follow, handled there
            break;

        case 'archive_done':
            hideArchiveDialog();
            // Clear chat UI
            document.getElementById('chat-messages').innerHTML = '';
            addSystemMessage(`📦 Archived ${msg.count} message(s) → ${msg.filename}`);
            break;

        case 'search_results':
            renderSearchResults(msg.query, msg.results || []);
            break;

        case 'auth_result':
            isAdmin = msg.is_admin;
            if (isAdmin) addSystemMessage('👑 You are connected as admin');
            break;

        case 'model_list':
            availableModels = msg.models || [];
            activeModel = msg.active || '';
            // Restore saved model preference (admin only)
            if (isAdmin) {
                const saved = localStorage.getItem('rune_model');
                if (saved && availableModels.includes(saved) && saved !== activeModel) {
                    switchModel(saved);
                }
            }
            updateModelIndicator();
            break;

        case 'model_changed':
            activeModel = msg.model || '';
            updateModelIndicator();
            addSystemMessage('🔄 Model switched to: ' + activeModel);
            break;

        case 'session_list':
            sessions = msg.sessions || [];
            renderSessionTree();
            // Show + button for admin
            const newBtn = document.getElementById('btn-new-session');
            if (newBtn && isAdmin) newBtn.classList.remove('hidden');
            break;

        case 'session_switched':
            currentSessionId = msg.session_id;
            document.getElementById('chat-messages').innerHTML = '';
            renderSessionTree();
            break;

        case 'dir_browse_result':
            renderDirBrowser(msg.path, msg.parent, msg.entries || []);
            break;

        case 'system':
            addSystemMessage(msg.content);
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
            addSystemMessage(`Error: ${msg.message}`);
            break;
    }
}

// --- Chat ---
function sendMessage() {
    const text = chatInput.value.trim();
    if (!text || !isConnected) return;

    // Send to server — do NOT optimistic render; wait for broadcast echo
    ws.send(JSON.stringify({ type: 'chat_send', content: text }));
    chatInput.value = '';
    chatInput.style.height = 'auto';
}

function fmtTime(unixSec) {
    const d = unixSec ? new Date(unixSec * 1000) : new Date();
    const mm  = String(d.getMonth() + 1).padStart(2, '0');
    const dd  = String(d.getDate()).padStart(2, '0');
    const hh  = String(d.getHours()).padStart(2, '0');
    const min = String(d.getMinutes()).padStart(2, '0');
    return `${mm}-${dd} ${hh}:${min}`;
}

function addChatMessage(nickname, content) {
    const isMe = nickname === myNickname;
    const div = document.createElement('div');
    div.className = `chat-msg ${isMe ? 'user' : 'other'}`;

    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.textContent = isMe ? `🧑 ${nickname} (you)` : `👤 ${nickname}`;
    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    timeSpan.textContent = fmtTime(null);
    sender.appendChild(nameSpan);
    sender.appendChild(timeSpan);

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

function replayHistory(messages) {
    if (!messages || messages.length === 0) return;
    addSystemMessage('── archived ──');
    for (const m of messages) {
        if (m.role === 'user') {
            // Render history user message with original timestamp
            const isMe = m.nickname === myNickname;
            const div = document.createElement('div');
            div.className = `chat-msg ${isMe ? 'user' : 'other'}`;
            const sender = document.createElement('div');
            sender.className = 'sender';
            const nameSpan = document.createElement('span');
            nameSpan.textContent = isMe ? `🧑 ${m.nickname} (you)` : `👤 ${m.nickname}`;
            const timeSpan = document.createElement('span');
            timeSpan.className = 'msg-time';
            timeSpan.textContent = fmtTime(m.created_at || null);
            sender.appendChild(nameSpan);
            sender.appendChild(timeSpan);
            const body = document.createElement('div');
            body.className = 'body';
            body.textContent = m.content;
            div.appendChild(sender);
            div.appendChild(body);
            chatMessages.appendChild(div);
        } else if (m.role === 'assistant') {
            // Render as completed assistant message
            const div = document.createElement('div');
            div.className = 'chat-msg assistant';
            const sender = document.createElement('div');
            sender.className = 'sender';
            const nameSpan = document.createElement('span');
            nameSpan.textContent = 'ᚱᚢᚾᛖ';
            sender.appendChild(nameSpan);
            // Attach model + token meta if available
            if (m.model || m.tokens_in || m.tokens_out) {
                const meta = document.createElement('span');
                meta.className = 'msg-meta';
                let parts = [];
                if (m.model) parts.push(m.model);
                if (m.tokens_in || m.tokens_out) parts.push(`↑${m.tokens_in||0} ↓${m.tokens_out||0}`);
                meta.textContent = parts.join(' · ');
                sender.appendChild(meta);
            }
            const timeSpan = document.createElement('span');
            timeSpan.className = 'msg-time';
            timeSpan.textContent = fmtTime(m.created_at || null);
            sender.appendChild(timeSpan);
            const body = document.createElement('div');
            body.className = 'body';
            if (typeof marked !== 'undefined') {
                body.innerHTML = marked.parse(m.content);
            } else {
                body.textContent = m.content;
            }
            div.appendChild(sender);
            div.appendChild(body);
            chatMessages.appendChild(div);
        } else if (m.role === 'system') {
            addSystemMessage(m.content);
        }
    }
    addSystemMessage('── current ──');
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function updateOnlineCount(count) {
    const el = document.getElementById('online-count');
    if (el) el.textContent = count;
}

let currentAssistantEl = null;
let currentAssistantDiv = null;
let currentAssistantText = '';

function appendToLastAssistant(token) {
    if (!currentAssistantEl) {
        const div = document.createElement('div');
        div.className = 'chat-msg assistant';

        const sender = document.createElement('div');
        sender.className = 'sender';
        const nameSpan = document.createElement('span');
        nameSpan.textContent = 'ᚱᚢᚾᛖ';
        const timeSpan = document.createElement('span');
        timeSpan.className = 'msg-time';
        timeSpan.textContent = fmtTime(null);
        sender.appendChild(nameSpan);
        sender.appendChild(timeSpan);

        const body = document.createElement('div');
        body.className = 'body';

        div.appendChild(sender);
        div.appendChild(body);
        chatMessages.appendChild(div);
        currentAssistantEl = body;
        currentAssistantDiv = div;
        currentAssistantText = '';
    }
    currentAssistantText += token;
    if (typeof marked !== 'undefined') {
        currentAssistantEl.innerHTML = marked.parse(currentAssistantText);
    } else {
        currentAssistantEl.textContent = currentAssistantText;
    }
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function finalizeAssistantMessage() {
    if (currentAssistantEl && typeof marked !== 'undefined') {
        currentAssistantEl.innerHTML = marked.parse(currentAssistantText);
    }
    currentAssistantEl = null;
    currentAssistantText = '';
    currentAssistantDiv = null;
}

function attachMetaToLastAssistant(model, tokIn, tokOut, ctxTokens, ctxWindow) {
    const target = currentAssistantDiv || chatMessages.querySelector('.chat-msg.assistant:last-child');
    if (!target) return;
    const sender = target.querySelector('.sender');
    if (!sender) return;
    // Remove old meta if any
    const oldMeta = sender.querySelector('.msg-meta');
    if (oldMeta) oldMeta.remove();
    if (!model && !tokIn && !tokOut) return;
    const meta = document.createElement('span');
    meta.className = 'msg-meta';
    let parts = [];
    if (model) parts.push(model);
    if (tokIn || tokOut) parts.push(`↑${tokIn||0} ↓${tokOut||0}`);
    meta.textContent = parts.join(' · ');
    // Insert before .msg-time if present
    const timeEl = sender.querySelector('.msg-time');
    if (timeEl) sender.insertBefore(meta, timeEl);
    else sender.appendChild(meta);
    // Update context overlay
    if (ctxWindow && ctxWindow > 0) updateContextOverlay(ctxTokens || 0, ctxWindow);
}

function updateContextOverlay(ctxTokens, ctxWindow) {
    const overlay = document.getElementById('context-overlay');
    const pctEl   = document.getElementById('context-pct');
    const cntEl   = document.getElementById('context-counts');
    if (!overlay || !pctEl || !cntEl) return;
    const pct = Math.round((ctxTokens / ctxWindow) * 100);
    pctEl.textContent = pct + '% context used';
    const fmt = n => n >= 1000 ? (n / 1000).toFixed(1) + 'k' : String(n);
    cntEl.textContent = fmt(ctxTokens) + ' / ' + fmt(ctxWindow);
    overlay.classList.remove('hidden', 'warn', 'danger');
    if (pct >= 80) overlay.classList.add('danger');
    else if (pct >= 60) overlay.classList.add('warn');
}

function showApprovalRequest(id, detail) {
    if (!isAdmin) return; // only admin sees approval requests
    const div = document.createElement('div');
    div.className = 'chat-msg assistant approval';
    div.innerHTML = `
        <div class="sender">🔒 Approval Required</div>
        <div class="body"><code>${escapeHtml(detail)}</code></div>
        <div id="approval-btns-${id}" style="margin-top:8px;display:flex;gap:8px">
            <button onclick="respondApproval('${id}',true)" class="btn-approve">✓ Allow</button>
            <button onclick="respondApproval('${id}',false)" class="btn-deny">✗ Deny</button>
        </div>
    `;
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function removeApprovalButtons(id) {
    const btns = document.getElementById('approval-btns-' + id);
    if (btns) btns.remove();
}

function removeAllApprovalButtons() {
    document.querySelectorAll('[id^="approval-btns-"]').forEach(el => el.remove());
}

function respondApproval(id, approved) {
    ws.send(JSON.stringify({ type: 'approval_response', id, approved }));
    addSystemMessage(approved ? `Approved: ${id}` : `Denied: ${id}`);
    removeApprovalButtons(id);
}

// --- Spec Editor ---
function applyPanelLayout() {
    const center     = document.getElementById('panel-center');
    const centerBody = document.getElementById('center-body');

    // Editor visibility
    if (showEdit) {
        editorContainer.classList.remove('hidden');
        btnEdit.classList.add('active');
    } else {
        editorContainer.classList.add('hidden');
        btnEdit.classList.remove('active');
    }

    // Preview visibility
    if (showPreview) {
        previewContainer.classList.remove('hidden');
        btnPreview.classList.add('active');
        renderPreview();
    } else {
        previewContainer.classList.add('hidden');
        btnPreview.classList.remove('active');
    }

    // Split layout: side-by-side when both on
    if (showEdit && showPreview) {
        centerBody.classList.add('split-view');
        editorContainer.style.width  = '';
        previewContainer.style.width = '';
    } else {
        centerBody.classList.remove('split-view');
        editorContainer.style.width  = '';
        previewContainer.style.width = '';
    }

    // Both off → hide center so chat expands to fill
    if (!showEdit && !showPreview) {
        center.classList.add('hidden');
    } else {
        center.classList.remove('hidden');
    }

    // When center is hidden, force chat panel open
    const panelRight = document.getElementById('panel-right');
    if (!showEdit && !showPreview) {
        if (panelRight && panelRight.classList.contains('collapsed')) {
            panelRight.classList.remove('collapsed');
            updateToggleIcon(panelRight, 'right');
            try {
                const saved = localStorage.getItem('rune_panel_right');
                panelRight.style.width = saved ? saved + 'px' : '';
            } catch {}
        }
        // Hide the right panel resize handle arrow (no collapse allowed)
        const rh = document.getElementById('resize-right');
        if (rh) rh.style.pointerEvents = 'none';
        if (rh) rh.querySelector('.toggle-icon') && (rh.querySelector('.toggle-icon').style.display = 'none');
    } else {
        const rh = document.getElementById('resize-right');
        if (rh) rh.style.pointerEvents = '';
        const icon = rh && rh.querySelector('.toggle-icon');
        if (icon) icon.style.display = '';
    }

    // Persist
    try {
        localStorage.setItem('rune_show_edit',    showEdit    ? '1' : '0');
        localStorage.setItem('rune_show_preview', showPreview ? '1' : '0');
    } catch {}
}

function toggleEdit() {
    showEdit = !showEdit;
    applyPanelLayout();
}

function togglePreview() {
    showPreview = !showPreview;
    applyPanelLayout();
}

// Legacy alias (used internally for keyboard shortcut etc.)
function setMode(mode) {
    if (mode === 'edit')    { showEdit = true;  showPreview = false; }
    else                    { showEdit = false; showPreview = true;  }
    applyPanelLayout();
}

// --- Edit ↔ Preview scroll sync ---
let _scrollSyncLock = false;

function syncEditorToPreview(textarea) {
    if (!showPreview || !showEdit) return; // only sync in split view
    if (_scrollSyncLock) return;
    const maxScroll = textarea.scrollHeight - textarea.clientHeight;
    if (maxScroll <= 0) return;
    const pct = textarea.scrollTop / maxScroll;
    const pc = previewContainer;
    const pcMax = pc.scrollHeight - pc.clientHeight;
    if (pcMax <= 0) return;
    _scrollSyncLock = true;
    pc.scrollTop = pct * pcMax;
    requestAnimationFrame(() => { _scrollSyncLock = false; });
}

function syncPreviewToEditor(textarea) {
    if (!showPreview || !showEdit) return;
    if (_scrollSyncLock) return;
    const pc = previewContainer;
    const pcMax = pc.scrollHeight - pc.clientHeight;
    if (pcMax <= 0) return;
    const pct = pc.scrollTop / pcMax;
    const maxScroll = textarea.scrollHeight - textarea.clientHeight;
    if (maxScroll <= 0) return;
    _scrollSyncLock = true;
    textarea.scrollTop = pct * maxScroll;
    requestAnimationFrame(() => { _scrollSyncLock = false; });
}

function initPreviewScrollSync() {
    const textarea = document.getElementById('editor');
    if (!textarea) return;
    previewContainer.addEventListener('scroll', () => {
        syncPreviewToEditor(textarea);
    });
}

function renderPreview() {
    if (typeof marked !== 'undefined') {
        preview.innerHTML = marked.parse(specContent);
        // Render mermaid blocks (with ready-wait for slow 3MB load)
        preview.querySelectorAll('.mermaid-block').forEach(el => {
            const src = el.dataset.src ? el.dataset.src.replace(/&quot;/g, '"') : '';
            if (!src) return;
            const doRender = (retries) => {
                if (window.mermaid && typeof window.mermaid.render === 'function') {
                    // Use unique ID each render to prevent mermaid cache stale results
                    const uid = 'mermaid-' + Date.now() + '-' + Math.random().toString(36).slice(2);
                    el.id = uid;
                    window.mermaid.render(uid + '-svg', src)
                        .then(({ svg }) => { el.innerHTML = svg; })
                        .catch(err => {
                            el.innerHTML = '<pre style="color:var(--error)">Mermaid error: ' + escapeHtml(err.message) + '</pre>';
                        });
                } else if (retries > 0) {
                    setTimeout(() => doRender(retries - 1), 200);
                } else {
                    el.innerHTML = '<pre style="color:var(--text-muted)">Mermaid not loaded</pre>';
                }
            };
            doRender(20); // wait up to 4s (20 × 200ms)
        });

        preview.querySelectorAll('pre.hljs-pre').forEach(pre => {
            const btn = document.createElement('button');
            btn.className = 'copy-btn';
            btn.textContent = '📋';
            btn.title = 'Copy';
            btn.onclick = (e) => {
                e.stopPropagation();
                // Decode data-raw: unescape &quot; &amp; &lt; &gt;
                let raw = '';
                if (pre.dataset.raw !== undefined) {
                    const tmp = document.createElement('textarea');
                    tmp.innerHTML = pre.dataset.raw;
                    raw = tmp.value;
                } else {
                    raw = pre.querySelector('code')?.textContent ?? '';
                }

                const doCopy = () => {
                    btn.textContent = '✓';
                    btn.style.opacity = '1';
                    setTimeout(() => {
                        btn.textContent = '📋';
                        btn.style.opacity = '';
                    }, 1500);
                };

                if (navigator.clipboard && window.isSecureContext) {
                    navigator.clipboard.writeText(raw).then(doCopy).catch(() => fallbackCopy(raw, doCopy));
                } else {
                    fallbackCopy(raw, doCopy);
                }
            };
            pre.style.position = 'relative';
            pre.appendChild(btn);
        });
    } else {
        preview.textContent = specContent;
    }
}

function flashSpecIndicator() {
    const toolbar = document.querySelector('.toolbar');
    toolbar.classList.add('spec-updated');
    setTimeout(() => toolbar.classList.remove('spec-updated'), 1200);
}

// --- Status ---
// --- Archive ---
// --- Model switcher ---
function updateModelIndicator() {
    const indicator = document.getElementById('model-indicator');
    const nameEl = document.getElementById('model-name');
    if (!indicator || !nameEl) return;
    if (!activeModel) { indicator.style.display = 'none'; return; }
    nameEl.textContent = activeModel;
    indicator.style.display = 'flex';
    // Admin can click the name to switch; show pointer cursor
    nameEl.style.cursor = (isAdmin && availableModels.length > 1) ? 'pointer' : 'default';
}

function showModelDialog() {
    if (!isAdmin || availableModels.length <= 1) return;
    const listEl = document.getElementById('model-list');
    if (!listEl) return;
    listEl.innerHTML = '';
    availableModels.forEach(m => {
        const btn = document.createElement('button');
        btn.className = 'model-option' + (m === activeModel ? ' active' : '');
        btn.textContent = m;
        btn.onclick = () => { switchModel(m); hideModelDialog(); };
        listEl.appendChild(btn);
    });
    document.getElementById('model-modal').classList.remove('hidden');
}

function hideModelDialog() {
    document.getElementById('model-modal').classList.add('hidden');
}

function switchModel(model) {
    if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'switch_model', model }));
        try { localStorage.setItem('rune_model', model); } catch {}
    }
}

function showArchiveDialog() {
    document.getElementById('archive-modal').classList.remove('hidden');
}
function hideArchiveDialog() {
    document.getElementById('archive-modal').classList.add('hidden');
}
function confirmArchive() {
    if (isConnected) ws.send(JSON.stringify({ type: 'archive_chat' }));
}

// --- Search ---
function showSearchDialog() {
    document.getElementById('search-modal').classList.remove('hidden');
    document.getElementById('search-input').focus();
}
function hideSearchDialog() {
    document.getElementById('search-modal').classList.add('hidden');
}
function doSearch() {
    const q = document.getElementById('search-input').value.trim();
    if (!q) return;
    document.getElementById('search-results').innerHTML = '<div class="search-loading">Searching…</div>';
    if (isConnected) ws.send(JSON.stringify({ type: 'search_chat', query: q }));
}
function renderSearchResults(query, results) {
    const el = document.getElementById('search-results');
    if (!results.length) {
        el.innerHTML = `<div class="search-empty">No results for "${escapeHtml(query)}"</div>`;
        return;
    }
    const html = results.map(r => {
        const ts = new Date(r.created_at * 1000).toLocaleString('zh-TW');
        const role = r.role === 'assistant' ? '🤖' : '🧑';
        const highlighted = escapeHtml(r.content).replace(
            new RegExp(escapeHtml(query).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'),
            m => `<mark>${m}</mark>`
        );
        return `<div class="search-item">
            <div class="search-meta">${role} <strong>${escapeHtml(r.nickname)}</strong> <span class="search-time">${ts}</span></div>
            <div class="search-content">${highlighted}</div>
        </div>`;
    }).join('');
    el.innerHTML = `<div class="search-count">${results.length} result(s)</div>` + html;
}
function escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

// --- Logout ---
function showLogoutDialog() {
    document.getElementById('logout-modal').classList.remove('hidden');
}

function hideLogoutDialog() {
    document.getElementById('logout-modal').classList.add('hidden');
}

function confirmLogout() {
    loggedOut = true;
    // Close WS
    if (ws) { try { ws.close(); } catch(e) {} ws = null; }
    // Clear all stored credentials
    localStorage.removeItem('rune_nickname');
    localStorage.removeItem('rune_token');
    localStorage.removeItem('rune_model');
    // Reset in-memory state
    myNickname = '';
    myToken    = '';
    // Hide dialog + app, show login modal
    hideLogoutDialog();
    document.getElementById('nickname-input').value = '';
    const tokenInput = document.getElementById('token-input');
    if (tokenInput) tokenInput.value = '';
    document.getElementById('nickname-modal').classList.remove('hidden');
}

// --- File management ---
function updateDocTitle(name) {
    const el = document.getElementById('doc-title');
    if (el && !el.isContentEditable) el.textContent = name;
}

function createFile() {
    const name = prompt('New filename (must end in .md):');
    if (!name) return;
    const clean = name.trim();
    if (!clean.endsWith('.md')) { alert('Filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(clean)) { alert('Invalid filename. Use only letters, numbers, _ - .'); return; }
    if (isConnected) ws.send(JSON.stringify({ type: 'file_create', name: clean }));
}

function deleteCurrentFile() {
    if (!confirm('Delete ' + currentFilename + '?')) return;
    if (isConnected) ws.send(JSON.stringify({ type: 'file_delete', name: currentFilename }));
}

function switchFile(name) {
    if (isConnected) ws.send(JSON.stringify({ type: 'file_switch', name }));
}

function renameCurrentFile(newName) {
    const clean = newName.trim();
    if (!clean || clean === currentFilename) return;
    if (!clean.endsWith('.md')) { alert('Filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(clean)) { alert('Invalid filename'); return; }
    if (isConnected) ws.send(JSON.stringify({ type: 'file_rename', old_name: currentFilename, new_name: clean }));
}

function initDocTitle() {
    const el = document.getElementById('doc-title');
    if (!el) return;
    el.addEventListener('dblclick', () => {
        el.contentEditable = 'true';
        el.focus();
        // Select all
        const range = document.createRange();
        range.selectNodeContents(el);
        window.getSelection().removeAllRanges();
        window.getSelection().addRange(range);
    });
    el.addEventListener('keydown', e => {
        if (e.key === 'Enter') { e.preventDefault(); el.blur(); }
        if (e.key === 'Escape') { el.textContent = currentFilename; el.blur(); }
    });
    el.addEventListener('blur', () => {
        el.contentEditable = 'false';
        const newName = el.textContent.trim();
        if (newName && newName !== currentFilename) {
            renameCurrentFile(newName);
        } else {
            el.textContent = currentFilename; // revert if unchanged
        }
    });
}

const STATUS_EMOJI = {
    idle:         '🟢',
    typing:       '⌨️',
    thinking:     '🤔',
    disconnected: '🔴',
};
function setStatus(state) {
    statusIndicator.className = `status ${state}`;
    statusIndicator.textContent = STATUS_EMOJI[state] || '⚪';
    statusIndicator.title = state;
}

// --- Panel Toggle ---
function togglePanel(side) {
    const panel = document.getElementById('panel-' + side);
    // When center is hidden (both Edit+Preview off), chat panel cannot collapse
    if (side === 'right') {
        const center = document.getElementById('panel-center');
        if (center && center.classList.contains('hidden')) return;
    }
    const wasCollapsed = panel.classList.contains('collapsed');
    panel.classList.toggle('collapsed');
    updateToggleIcon(panel, side);

    if (wasCollapsed) {
        // Expanding: restore saved width (or default 280)
        try {
            const saved = localStorage.getItem('rune_panel_' + side);
            panel.style.width = (saved ? saved + 'px' : '280px');
        } catch {
            panel.style.width = '280px';
        }
        
    } else {
        // Collapsing: save current width, then clear inline so CSS !important takes over
        try { localStorage.setItem('rune_panel_' + side, panel.offsetWidth); } catch {}
        panel.style.width = '';
    }
}

function updateToggleIcon(panel, side) {
    const icon = panel.querySelector('.toggle-icon');
    if (!icon) return;
    const collapsed = panel.classList.contains('collapsed');
    if (side === 'left')  icon.textContent = collapsed ? '›' : '‹';
    else                  icon.textContent = collapsed ? '‹' : '›';
}

// --- Utilities ---
function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

function fallbackCopy(text, onSuccess) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    try {
        document.execCommand('copy');
        if (onSuccess) onSuccess();
    } catch (err) {
        console.error('Copy failed:', err);
    }
    document.body.removeChild(ta);
}

// --- Keyboard shortcuts ---
chatInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
    }
});

document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key === 'E') {
        e.preventDefault();
        if (showEdit || showPreview) { showEdit = !showEdit; showPreview = !showPreview; } else { showEdit = true; } applyPanelLayout();
    }
});

document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'Enter') {
        e.preventDefault();
        chatInput.focus();
        sendMessage();
    }
});

// --- Panel Resize ---
function initPanelResize() {
    ['left', 'right'].forEach(side => {
        const panel = document.getElementById('panel-' + side);
        const saved = localStorage.getItem('rune_panel_' + side);
        if (saved && !panel.classList.contains('collapsed')) panel.style.width = saved + 'px';
        updateToggleIcon(panel, side);
    });
    setupResizeHandle('resize-left',  'panel-left',  'left');
    setupResizeHandle('resize-right', 'panel-right', 'right');
}

function setupResizeHandle(handleId, panelId, side) {
    const handle = document.getElementById(handleId);
    const panel  = document.getElementById(panelId);
    if (!handle || !panel) return;

    let startX, startW, moved = false;

    handle.addEventListener('mousedown', e => {
        e.preventDefault();
        startX = e.clientX;
        startW = panel.offsetWidth;
        moved  = false;
        handle.classList.add('dragging');
        document.body.style.userSelect = 'none';

        // Only set col-resize cursor when not collapsed
        if (!panel.classList.contains('collapsed')) {
            document.body.style.cursor = 'col-resize';
        }

        function onMove(e) {
            if (panel.classList.contains('collapsed')) return;
            const dist = Math.abs(e.clientX - startX);
            if (dist < 4) return; // dead zone — too small to be a drag
            moved = true;
            const delta = side === 'left' ? e.clientX - startX : startX - e.clientX;
            const minW = parseInt(getComputedStyle(panel).minWidth) || 160;
            const maxW = parseInt(getComputedStyle(panel).maxWidth) || 600;
            const newW = Math.max(minW, Math.min(maxW, startW + delta));
            panel.style.width = newW + 'px';
        }

        function onUp() {
            handle.classList.remove('dragging');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            document.removeEventListener('mousemove', onMove);
            document.removeEventListener('mouseup', onUp);

            if (!moved) {
                // Pure click → toggle collapse
                togglePanel(side);
            } else {
                // Drag ended → persist width
                try { localStorage.setItem('rune_panel_' + side, panel.offsetWidth); } catch {}
                
            }
        }

        document.addEventListener('mousemove', onMove);
        document.addEventListener('mouseup', onUp);
    });
}

// --- Session Management ---

function renderSessionTree() {
    const tree = document.getElementById('session-tree');
    if (!tree) return;
    tree.innerHTML = '';
    sessions.forEach(s => {
        const item = document.createElement('div');
        item.className = 'session-item';
        item.dataset.sessionId = s.id;

        const header = document.createElement('div');
        header.className = 'session-header' + (s.id === currentSessionId ? ' active' : '');

        const toggle = document.createElement('span');
        toggle.className = 'session-toggle';
        toggle.textContent = s.id === currentSessionId ? '▼' : '▶';
        toggle.onclick = (e) => { e.stopPropagation(); toggleSessionFiles(s.id); };

        const name = document.createElement('span');
        name.className = 'session-name';
        name.textContent = s.name;
        name.title = s.workspace;
        name.onclick = () => switchSession(s.id);

        header.appendChild(toggle);
        header.appendChild(name);

        if (isAdmin) {
            const gear = document.createElement('span');
            gear.className = 'session-gear';
            gear.textContent = '⚙';
            gear.onclick = (e) => { e.stopPropagation(); showSessionSettings(s.id); };
            header.appendChild(gear);
        }

        item.appendChild(header);

        // File list
        const filesDiv = document.createElement('div');
        filesDiv.className = 'session-files' + (s.id !== currentSessionId ? ' collapsed' : '');
        filesDiv.id = 'session-files-' + s.id;
        (s.files || []).forEach(fname => {
            const fileEl = document.createElement('div');
            fileEl.className = 'session-file';
            fileEl.textContent = fname;
            fileEl.title = fname;
            fileEl.onclick = () => {
                if (s.id !== currentSessionId) switchSession(s.id);
                // Switch file
                ws.send(JSON.stringify({ type: 'file_switch', name: fname }));
                // Highlight
                tree.querySelectorAll('.session-file').forEach(f => f.classList.remove('active'));
                fileEl.classList.add('active');
            };
            filesDiv.appendChild(fileEl);
        });
        item.appendChild(filesDiv);
        tree.appendChild(item);
    });
}

function toggleSessionFiles(sessionId) {
    const filesDiv = document.getElementById('session-files-' + sessionId);
    if (!filesDiv) return;
    filesDiv.classList.toggle('collapsed');
    // Update toggle icon
    const item = filesDiv.parentElement;
    const toggle = item.querySelector('.session-toggle');
    if (toggle) toggle.textContent = filesDiv.classList.contains('collapsed') ? '▶' : '▼';
}

function switchSession(sessionId) {
    if (sessionId === currentSessionId) return;
    currentSessionId = sessionId;
    ws.send(JSON.stringify({ type: 'session_switch', session_id: sessionId }));
    renderSessionTree();
}

// --- New Session Dialog ---

function showNewSessionDialog() {
    document.getElementById('new-session-modal').classList.remove('hidden');
    document.getElementById('new-session-name').value = '';
    const wsInput = document.getElementById('new-session-workspace');
    // Default to first session's workspace or empty
    const defaultWs = sessions.length > 0 ? sessions[0].workspace : '';
    wsInput.value = defaultWs;
    document.getElementById('new-session-name').focus();
}

function hideNewSessionDialog() {
    document.getElementById('new-session-modal').classList.add('hidden');
}

function createSession() {
    const name = document.getElementById('new-session-name').value.trim();
    const workspace = document.getElementById('new-session-workspace').value.trim();
    if (!name) return;
    ws.send(JSON.stringify({ type: 'session_create', name, workspace: workspace || undefined }));
    hideNewSessionDialog();
}

// --- Session Settings Dialog ---

function showSessionSettings(sessionId) {
    const s = sessions.find(x => x.id === sessionId);
    if (!s) return;
    settingsSessionId = sessionId;
    document.getElementById('session-settings-title').textContent = 'Session: ' + s.name;
    document.getElementById('session-settings-name').value = s.name;
    document.getElementById('session-settings-workspace').value = s.workspace;
    // Hide delete button for default session
    const delBtn = document.getElementById('btn-delete-session');
    if (delBtn) delBtn.style.display = sessionId === 'default' ? 'none' : '';
    document.getElementById('session-settings-modal').classList.remove('hidden');
}

function hideSessionSettings() {
    document.getElementById('session-settings-modal').classList.add('hidden');
    settingsSessionId = null;
}

function saveSessionSettings() {
    if (!settingsSessionId) return;
    const name = document.getElementById('session-settings-name').value.trim();
    const workspace = document.getElementById('session-settings-workspace').value.trim();
    const s = sessions.find(x => x.id === settingsSessionId);
    if (s && name && name !== s.name) {
        ws.send(JSON.stringify({ type: 'session_rename', session_id: settingsSessionId, name }));
    }
    if (s && workspace && workspace !== s.workspace) {
        ws.send(JSON.stringify({ type: 'session_set_workspace', session_id: settingsSessionId, workspace }));
    }
    hideSessionSettings();
}

function deleteCurrentSession() {
    if (!settingsSessionId || settingsSessionId === 'default') return;
    if (!confirm('Delete this session? Chat history will be preserved but session metadata will be removed.')) return;
    ws.send(JSON.stringify({ type: 'session_delete', session_id: settingsSessionId }));
    hideSessionSettings();
    if (currentSessionId === settingsSessionId) {
        switchSession('default');
    }
}

// --- Directory Browser ---

function openDirBrowser(targetInputId) {
    dirBrowserTargetInput = document.getElementById(targetInputId);
    const startPath = dirBrowserTargetInput ? (dirBrowserTargetInput.value || '/') : '/';
    document.getElementById('dir-browser-modal').classList.remove('hidden');
    navigateDir(startPath || '/');
}

function hideDirBrowser() {
    document.getElementById('dir-browser-modal').classList.add('hidden');
    dirBrowserTargetInput = null;
}

function navigateDir(path) {
    document.getElementById('dir-browser-path').value = path;
    ws.send(JSON.stringify({ type: 'dir_browse', path }));
}

function renderDirBrowser(path, parent, entries) {
    document.getElementById('dir-browser-path').value = path;
    const list = document.getElementById('dir-browser-list');
    list.innerHTML = '';
    // Parent directory entry
    if (parent) {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        el.innerHTML = '<span class="dir-entry-icon">⬆</span><span class="dir-entry-name">..</span>';
        el.onclick = () => navigateDir(parent);
        list.appendChild(el);
    }
    entries.forEach(e => {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        el.innerHTML = `<span class="dir-entry-icon">📁</span><span class="dir-entry-name">${escapeHtml(e.name)}</span>`;
        el.onclick = () => navigateDir(path + (path.endsWith('/') ? '' : '/') + e.name);
        list.appendChild(el);
    });
}

function selectDir() {
    const path = document.getElementById('dir-browser-path').value;
    if (dirBrowserTargetInput) {
        dirBrowserTargetInput.value = path;
    }
    hideDirBrowser();
}

// --- Init ---
initEditor();
initPreviewScrollSync();
initDocTitle();
initPanelResize();
// Restore edit/preview state
try {
    const se = localStorage.getItem('rune_show_edit');
    const sp = localStorage.getItem('rune_show_preview');
    if (se !== null) showEdit    = se === '1';
    if (sp !== null) showPreview = sp === '1';
} catch {}
applyPanelLayout();
loadStoredCredentials();
