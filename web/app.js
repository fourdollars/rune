// Rune WebUI — app.js
// Three-panel layout: Notes | Editor | Chat

'use strict';

// --- State ---
let showEdit    = true;
let showPreview = false;
let panelStateRestored = false;  // restore edit/preview from localStorage only once
let currentFilename = '';
let fileList = [];
let specContent = '';
let isConnected = false;
let evtSource = null;
let loggedOut   = false;
let editorDirty = false;
let debounceTimer = null;
let specVersion = 0;
let myNickname = '';
let myToken = '';
let isAdmin = false;
let availableModels = [];

// --- Note state ---
let notes = [];
let currentNoteId = '';
let dirBrowserTargetInput = null;
let settingsNoteId = null;

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
            if (editorDirty && currentNoteId) {
                api('file/update', { note_id: currentNoteId, filename: currentFilename, content: specContent });
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

// --- Connection ---
function connect() {
    if (evtSource) { evtSource.close(); evtSource = null; }

    const params = new URLSearchParams();
    if (myNickname) params.set('nickname', myNickname);
    if (myToken) params.set('token', myToken);

    evtSource = new EventSource('/api/events?' + params.toString());
    let authFailed = false;

    evtSource.onopen = () => {
        isConnected = true;
        addSystemMessage('Connected');
    };

    evtSource.onerror = (e) => {
        isConnected = false;
        if (authFailed) {
            // Don't reconnect on auth failure — show login
            evtSource.close();
            evtSource = null;
            return;
        }
        console.error('SSE error:', e);
        addSystemMessage('Disconnected. Reconnecting...');
    };

    // Listen for all event types
    const eventTypes = [
        'auth_result', 'model_list', 'note_list', 'note_switched',
        'history', 'file_list', 'file_content', 'file_deleted',
        'chat_token', 'chat_done', 'chat_meta', 'chat_message',
        'status', 'system', 'users_update', 'error',
        'model_changed', 'approval_request', 'archive_done',
        'search_results', 'dir_browse_result'
    ];

    eventTypes.forEach(type => {
        evtSource.addEventListener(type, (e) => {
            try {
                const msg = JSON.parse(e.data);
                // Handle auth failure: close SSE, show login modal
                if (msg.type === 'error' && msg.message && msg.message.includes('Authentication failed')) {
                    authFailed = true;
                    evtSource.close();
                    evtSource = null;
                    isConnected = false;
                    addSystemMessage('Authentication failed. Please check your token.');
                    document.getElementById('nickname-modal').classList.remove('hidden');
                    return;
                }
                handleMessage(msg);
            } catch(err) {
                console.error('Parse error:', err, e.data);
            }
        });
    });
}

// Helper for POST requests
async function api(endpoint, body) {
    const headers = { 'Content-Type': 'application/json' };
    if (myToken) headers['Authorization'] = 'Bearer ' + myToken;
    try {
        const resp = await fetch('/api/' + endpoint, {
            method: 'POST',
            headers,
            body: JSON.stringify(body),
        });
        const data = await resp.json();
        if (!data.ok && data.error) {
            addSystemMessage('Error: ' + data.error);
        }
        return data;
    } catch(e) {
        console.error('API error:', e);
        addSystemMessage('Error: ' + e.message);
        return { ok: false, error: e.message };
    }
}

function handleMessage(msg) {
    switch (msg.type) {
        case 'file_content':
            currentFilename = msg.filename;
            specContent = msg.content;
            setEditorValue(msg.content);
            if (showPreview) renderPreview();
            updateDocTitle(msg.filename);
            break;
        case 'chat_message':
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
            // On first file_list after page load, try to restore saved file
            if (!panelStateRestored && !currentFilename) {
                try {
                    const savedFile = localStorage.getItem('rune_file');
                    if (savedFile && fileList.includes(savedFile)) currentFilename = savedFile;
                } catch {}
            }
            // If current file still exists, re-fetch its content (may have been edited by agent)
            if (currentFilename && fileList.includes(currentFilename)) {
                api('file/switch', { note_id: currentNoteId, name: currentFilename });
            } else if (!currentFilename && fileList.length > 0) {
                // No file selected yet, pick first
                currentFilename = fileList[0];
                api('file/switch', { note_id: currentNoteId, name: currentFilename });
            } else if (currentFilename && !fileList.includes(currentFilename)) {
                // Current file was deleted
                currentFilename = fileList.length > 0 ? fileList[0] : '';
                if (currentFilename) {
                    api('file/switch', { note_id: currentNoteId, name: currentFilename });
                }
            }
            updateDocTitle(currentFilename);
            try { localStorage.setItem('rune_file', currentFilename); } catch {}
            updateEditorVisibility(fileList.length);
            break;
        case 'file_deleted':
            break;
        case 'archive_done':
            hideArchiveDialog();
            document.getElementById('chat-messages').innerHTML = '';
            addSystemMessage('📦 Archived ' + (msg.count || 0) + ' message(s) → ' + msg.filename);
            break;
        case 'search_results':
            renderSearchResults(msg.query, msg.results || []);
            break;
        case 'auth_result':
            isAdmin = msg.is_admin;
            // Rainbow title for admin
            const runeTitle = document.getElementById('rune-title');
            if (runeTitle && isAdmin) {
                runeTitle.classList.add('rune-title-rainbow');
            }
            if (isAdmin) addSystemMessage('👑 You are connected as admin');
            break;
        case 'model_list':
            availableModels = msg.models || [];
            activeModel = msg.active || '';
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
        case 'note_list':
            notes = msg.notes || [];
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
            updatePageTitle();
            const newBtn = document.getElementById('btn-new-note');
            if (newBtn && isAdmin) newBtn.classList.remove('hidden');
            break;
        case 'note_switched':
            currentNoteId = msg.note_id;
            updateChatInputState();
            document.getElementById('chat-messages').innerHTML = '';
            renderNoteList();
            updatePageTitle();
            const overlay2 = document.getElementById('context-overlay');
            if (overlay2) overlay2.classList.add('hidden');
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
            addSystemMessage('Error: ' + msg.message);
            break;
    }
}

// --- Chat ---
function sendMessage() {
    const text = chatInput.value.trim();
    if (!text || !isConnected || !currentNoteId) return;

    // Send to server — do NOT optimistic render; wait for broadcast echo
    api('chat', { note_id: currentNoteId, content: text, nickname: myNickname });
    chatInput.value = '';
    chatInput.style.height = 'auto';
}

function updateChatInputState() {
    if (!currentNoteId) {
        chatInput.disabled = true;
        chatInput.placeholder = 'Create a session first...';
    } else {
        chatInput.disabled = false;
        chatInput.placeholder = 'Type a message...';
    }
    applyNoNoteLayout();
}

function applyNoNoteLayout() {
    const panelLeft = document.getElementById('panel-left');
    const panelCenter = document.getElementById('panel-center');
    const panelRight = document.getElementById('panel-right');

    if (!currentNoteId) {
        // No active note: hide Edit/Preview buttons
        updateEditorVisibility(0);
    }

    if (!currentNoteId && notes.length === 0) {
        // Truly no notes: expand note panel fullscreen
        panelCenter.classList.add('hidden');
        panelRight.classList.add('hidden');
        panelLeft.classList.remove('collapsed');
        panelLeft.classList.add('fullscreen');
    } else {
        // Note exists or active: restore normal layout
        panelLeft.classList.remove('fullscreen');
        panelCenter.classList.remove('hidden');
        panelRight.classList.remove('hidden');
    }
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
    api('approval', { id, approved });
    addSystemMessage(approved ? `Approved: ${id}` : `Denied: ${id}`);
    removeApprovalButtons(id);
}

// --- Spec Editor ---
function updateEditorVisibility(fileCount) {
    const btnEdit = document.getElementById('btn-edit');
    const btnPreview = document.getElementById('btn-preview');
    if (fileCount === 0) {
        // No markdown files: hide buttons and collapse editor
        btnEdit.classList.add('hidden');
        btnPreview.classList.add('hidden');
        showEdit = false;
        showPreview = false;
    } else {
        // Has files: show buttons
        btnEdit.classList.remove('hidden');
        btnPreview.classList.remove('hidden');
        // Restore from localStorage only on first load; afterwards honour current state
        if (!panelStateRestored) {
            panelStateRestored = true;
            try {
                const se = localStorage.getItem('rune_show_edit');
                const sp = localStorage.getItem('rune_show_preview');
                showEdit    = se !== null ? se === '1' : true;
                showPreview = sp !== null ? sp === '1' : false;
            } catch {}
        }
    }
    applyPanelLayout();
}

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
    if (isConnected) {
        api('model/switch', { model });
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
    api('chat/archive', { note_id: currentNoteId });
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
    api('chat/search', { note_id: currentNoteId, query: q });
}
function renderSearchResults(query, results) {
    const el = document.getElementById('search-results');
    if (!results.length) {
        el.innerHTML = `<div class="search-empty">No results for "${escapeHtml(query)}"</div>`;
        return;
    }
    const html = results.map((r, i) => {
        const ts = new Date(r.created_at * 1000).toLocaleString('zh-TW');
        const role = r.role === 'assistant' ? '🤖' : '🧑';
        const highlighted = escapeHtml(r.content).replace(
            new RegExp(escapeHtml(query).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'),
            m => `<mark>${m}</mark>`
        );
        return `<div class="search-item">
            <div class="search-meta">${role} <strong>${escapeHtml(r.nickname)}</strong> <span class="search-time">${ts}</span><button class="search-copy-btn" data-idx="${i}" title="Copy">📋</button></div>
            <div class="search-content">${highlighted}</div>
        </div>`;
    }).join('');
    el.innerHTML = `<div class="search-count">${results.length} result(s)</div>` + html;
    // Bind copy buttons
    el.querySelectorAll('.search-copy-btn').forEach(btn => {
        btn.onclick = () => {
            const idx = parseInt(btn.dataset.idx);
            const text = results[idx].content;
            navigator.clipboard.writeText(text).then(() => {
                btn.textContent = '✓';
                setTimeout(() => btn.textContent = '📋', 1500);
            });
        };
    });
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
    // EventSource handles reconnection automatically
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
    updatePageTitle();
}

function updatePageTitle() {
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
}

function createFile() {
    const name = prompt('New filename (must end in .md):');
    if (!name) return;
    const clean = name.trim();
    if (!clean.endsWith('.md')) { alert('Filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(clean)) { alert('Invalid filename. Use only letters, numbers, _ - .'); return; }
    api('file/create', { note_id: currentNoteId, name: clean });
}

function deleteCurrentFile() {
    if (!confirm('Delete ' + currentFilename + '?')) return;
    api('file/delete', { note_id: currentNoteId, name: currentFilename });
}

function switchFile(name) {
    api('file/switch', { note_id: currentNoteId, name });
}

function renameCurrentFile(newName) {
    const clean = newName.trim();
    if (!clean || clean === currentFilename) return;
    if (!clean.endsWith('.md')) { alert('Filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(clean)) { alert('Invalid filename'); return; }
    api('file/rename', { note_id: currentNoteId, old_name: currentFilename, new_name: clean });
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
    // Persist collapsed state
    try { localStorage.setItem('rune_panel_' + side + '_collapsed', panel.classList.contains('collapsed') ? '1' : '0'); } catch {}
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
        // Restore collapsed state
        const wasCollapsed = localStorage.getItem('rune_panel_' + side + '_collapsed');
        if (wasCollapsed === '1' && !panel.classList.contains('collapsed')) {
            panel.classList.add('collapsed');
        } else if (wasCollapsed === '0' && panel.classList.contains('collapsed')) {
            panel.classList.remove('collapsed');
        }
        // Restore width
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

// --- Note Management ---

function renderNoteList() {
    const tree = document.getElementById('note-tree');
    if (!tree) return;
    tree.innerHTML = '';
    if (notes.length === 0) {
        tree.innerHTML = '<div class="explorer-empty">No notes yet.<br>Click <b>+</b> to create one.</div>';
        return;
    }
    notes.forEach(s => {
        const section = document.createElement('div');
        section.className = 'explorer-section';

        // Folder row
        const folderRow = document.createElement('div');
        folderRow.className = 'explorer-row' + (s.id === currentNoteId && !(s.files||[]).length ? ' active' : '');
        folderRow.onclick = () => {
            const children = section.querySelector('.explorer-children');
            const chev = folderRow.querySelector('.chevron');
            if (children.classList.contains('collapsed')) {
                children.classList.remove('collapsed');
                chev.classList.add('open');
            } else {
                children.classList.add('collapsed');
                chev.classList.remove('open');
            }
            switchNote(s.id);
        };

        const chevron = document.createElement('span');
        chevron.className = 'chevron open';
        chevron.textContent = '›';

        const folderIcon = document.createElement('span');
        folderIcon.className = 'icon';
        folderIcon.textContent = (s.id === currentNoteId) ? '📂' : '📁';

        const folderLabel = document.createElement('span');
        folderLabel.className = 'label';
        folderLabel.textContent = s.name;
        folderLabel.style.fontWeight = '500';

        folderRow.appendChild(chevron);
        folderRow.appendChild(folderIcon);
        folderRow.appendChild(folderLabel);

        if (isAdmin) {
            const actions = document.createElement('span');
            actions.className = 'actions';

            const addBtn = document.createElement('button');
            addBtn.textContent = '📄+';
            addBtn.title = 'New file';
            addBtn.onclick = (e) => {
                e.stopPropagation();
                if (s.id !== currentNoteId) switchNote(s.id);
                createFile();
            };
            actions.appendChild(addBtn);

            const gearBtn = document.createElement('button');
            gearBtn.textContent = '⚙';
            gearBtn.title = 'Settings';
            gearBtn.onclick = (e) => { e.stopPropagation(); showNoteSettings(s.id); };
            actions.appendChild(gearBtn);

            folderRow.appendChild(actions);
        }

        section.appendChild(folderRow);

        // Children (files)
        const children = document.createElement('div');
        children.className = 'explorer-children';
        (s.files || []).forEach(fname => {
            const fileRow = document.createElement('div');
            fileRow.className = 'explorer-row';
            fileRow.style.paddingLeft = '20px';

            const fileIcon = document.createElement('span');
            fileIcon.className = 'icon';
            fileIcon.textContent = fname.endsWith('.md') ? '📝' : '📄';

            const fileLabel = document.createElement('span');
            fileLabel.className = 'label';
            fileLabel.textContent = fname;

            fileRow.appendChild(fileIcon);
            fileRow.appendChild(fileLabel);

            if (isAdmin) {
                const fileActions = document.createElement('span');
                fileActions.className = 'actions';

                const renameBtn = document.createElement('button');
                renameBtn.textContent = '✏️';
                renameBtn.title = 'Rename';
                renameBtn.onclick = (e) => {
                    e.stopPropagation();
                    const newName = prompt('Rename file:', fname);
                    if (newName && newName.trim() && newName.trim() !== fname) {
                        api('file/rename', { note_id: s.id, old_name: fname, new_name: newName.trim() });
                    }
                };
                fileActions.appendChild(renameBtn);

                const delBtn = document.createElement('button');
                delBtn.textContent = '🗑';
                delBtn.title = 'Delete';
                delBtn.onclick = (e) => {
                    e.stopPropagation();
                    if (confirm('Delete "' + fname + '"?')) {
                        api('file/delete', { note_id: s.id, name: fname });
                    }
                };
                fileActions.appendChild(delBtn);

                fileRow.appendChild(fileActions);
            }

            fileRow.onclick = () => {
                if (s.id !== currentNoteId) switchNote(s.id);
                api('file/switch', { note_id: s.id, name: fname });
                tree.querySelectorAll('.explorer-row').forEach(r => r.classList.remove('active'));
                fileRow.classList.add('active');
                if (!showPreview) {
                    showPreview = true;
                    applyPanelLayout();
                }
            };
            children.appendChild(fileRow);
        });
        section.appendChild(children);
        tree.appendChild(section);
    });
}





async function switchNote(sessionId) {
    if (sessionId === currentNoteId) return;
    currentNoteId = sessionId;
    localStorage.setItem('rune_note', sessionId);
    renderNoteList();
    updateChatInputState();
    updatePageTitle();

    const data = await api('note/switch', { note_id: sessionId });
    if (!data || !data.ok) return;

    // Replay history from response
    document.getElementById('chat-messages').innerHTML = '';
    if (data.history && data.history.length) {
        replayHistory(data.history);
    }

    // Update file list
    fileList = data.files || [];
    updateEditorVisibility(fileList.length);

    // Load first file content
    if (data.current_file && data.file_content !== undefined) {
        currentFilename = data.current_file;
        specContent = data.file_content || '';
        updateDocTitle(currentFilename);
        renderPreview();
        setEditorValue(specContent);
    } else {
        currentFilename = '';
        specContent = '';
        updateDocTitle('');
    }
}

// --- New Note Dialog ---

function showNewNoteDialog() {
    document.getElementById('new-note-modal').classList.remove('hidden');
    document.getElementById('new-note-name').value = '';
    document.getElementById('new-note-name').focus();
}

function hideNewNoteDialog() {
    document.getElementById('new-note-modal').classList.add('hidden');
}

function createNote() {
    const name = document.getElementById('new-note-name').value.trim();
    if (!name) return;
    api("note/create", { name }).then(() => switchNote(name));
    hideNewNoteDialog();
}

// --- Note Settings Dialog ---

function showNoteSettings(sessionId) {
    const s = notes.find(x => x.id === sessionId);
    if (!s) return;
    settingsNoteId = sessionId;
    document.getElementById('note-settings-title').textContent = 'Note: ' + s.name;
    document.getElementById('note-settings-name').value = s.name;
    // Hide delete button for default session
    const delBtn = document.getElementById('btn-delete-note');
    if (delBtn) delBtn.style.display = sessionId === 'default' ? 'none' : '';
    document.getElementById('note-settings-modal').classList.remove('hidden');
}

function hideNoteSettings() {
    document.getElementById('note-settings-modal').classList.add('hidden');
    settingsNoteId = null;
}

function saveNoteSettings() {
    if (!settingsNoteId) return;
    const name = document.getElementById('note-settings-name').value.trim();
    const s = notes.find(x => x.id === settingsNoteId);
    if (s && name && name !== s.name) {
        api('note/rename', { note_id: settingsNoteId, name });
    }
    hideNoteSettings();
}

function deleteCurrentNote() {
    if (!settingsNoteId) return;
    if (!confirm('Delete this session? Chat history will be preserved but session metadata will be removed.')) return;
    api('note/delete', { note_id: settingsNoteId });
    hideNoteSettings();
    if (currentNoteId === settingsNoteId) {
        currentNoteId = '';
        updateChatInputState();
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
    api('dir/browse', { path }).then(r => { if (r.ok && r.data) handleMessage(r.data); });
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
