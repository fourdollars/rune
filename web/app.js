// Rune WebUI — app.js
// Three-panel layout: Notes | Editor | Chat

'use strict';

// --- State ---
let showEdit    = true;
let showPreview = true;
let _editorStateRestored = false;
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
let isGuest = false;
let availableModels = [];
let currentThinking = 'off';

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
// Initial state: disconnected (before SSE connects)
if (statusIndicator) { statusIndicator.className = 'status disconnected'; statusIndicator.textContent = '🔴'; }
const mobileStatusInit = document.getElementById('mobile-status');
if (mobileStatusInit) { mobileStatusInit.className = 'status disconnected'; mobileStatusInit.textContent = '🔴'; }
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
    // Also update mobile editor if present
    if (isMobile) setMobileEditorContent(text);
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
async function fetchNoteListAndConnect() {
    // Try saved note first
    const saved = localStorage.getItem('rune_note');
    if (saved) { connect(saved); return; }

    // No saved note — fetch list via REST and connect to first available
    try {
        const res = await fetch('/api' + '/notes', {
            headers: myToken ? { 'Authorization': 'Bearer ' + myToken } : {}
        });
        const data = await res.json();
        if (data.ok && data.notes && data.notes.length > 0) {
            const firstNote = data.notes[0].id;
            connect(firstNote);
        }
    } catch {}
}

function connect(noteId) {
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
    if (myToken) params.set('token', myToken);
    params.set('note_id', targetNote);

    evtSource = new EventSource('/api/events?' + params.toString());
    let authFailed = false;

    evtSource.onopen = () => {
        isConnected = true;
        setStatus('idle');
        addSystemMessage('Connected');
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
        addSystemMessage('Disconnected. Reconnecting...');
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
                    addSystemMessage('\u26d4 This note is private. Switching to an accessible note...');
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
            // Only accept file_content SSE for current note + current file
            // (these come from file mutations like AI edits, not from file/switch)
            if (msg.note_id && msg.note_id !== currentNoteId) break;
            if (msg.filename !== currentFilename) break;
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
            addSystemMessage('📦 Archived ' + (msg.count || 0) + ' message(s) → ' + msg.filename);
            break;
        case 'search_results':
            renderSearchResults(msg.query, msg.results || []);
            break;
        case 'auth_result':
            isAdmin = msg.is_admin;
            isGuest = !!msg.is_guest;
            // Rainbow title for admin
            const runeTitle = document.getElementById('rune-title');
            if (runeTitle && isAdmin) {
                runeTitle.classList.add('rune-title-rainbow');
            }
            // Mobile title rainbow
            const mobileRuneTitle = document.getElementById('mobile-rune-title');
            if (mobileRuneTitle && isAdmin) {
                mobileRuneTitle.classList.add('rune-title-rainbow');
            }
            // Show mobile new-note button for admin
            const mobileDrawerActions = document.getElementById('mobile-drawer-actions');
            if (mobileDrawerActions && isAdmin) mobileDrawerActions.style.display = '';
            if (isAdmin) addSystemMessage('👑 You are connected as admin');
            if (isGuest) {
                addSystemMessage('👁 Read-only guest mode');
                // Hide chat input, new-note button, and edit button
                const chatInput = document.getElementById('chat-input');
                if (chatInput) chatInput.closest('.chat-input-area').style.display = 'none';
                const newNoteBtn = document.getElementById('btn-new-note');
                if (newNoteBtn) newNoteBtn.style.display = 'none';
                const editBtn = document.getElementById('btn-edit');
                if (editBtn) editBtn.style.display = 'none';
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
            addSystemMessage('🔄 Model switched to: ' + activeModel + ' ' + currentThinking);
            break;
        case 'thinking_changed':
            currentThinking = msg.thinking || 'off';
            updateThinkingSelect();
            addSystemMessage("🔄 Model switched to: " + activeModel + " " + currentThinking);
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

    if (!currentNoteId && notes.length === 0 && !isMobile) {
        // Truly no notes: expand note panel fullscreen (desktop only)
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


// --- Generic Dialog (replaces native prompt/confirm) ---
function showDialog({ title, message, input, inputValue, placeholder, danger, okLabel }) {
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
            nameSpan.textContent = 'ᚱ';
            sender.appendChild(nameSpan);
            // Model in header
            if (m.model) {
                const meta = document.createElement('span');
                meta.className = 'msg-meta';
                meta.textContent = (m.thinking && m.thinking !== 'off') ? `${m.model} ${m.thinking}` : m.model;
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
            // Run stats at message tail
            const totalTok = (m.tokens_in||0) + (m.tokens_out||0);
            if (m.steps || totalTok || m.tool_calls) {
                const stats = document.createElement('div');
                stats.className = 'run-stats';
                stats.textContent = `⚡ ${m.steps||0} steps | ${totalTok} tokens | ${m.tool_calls||0} tool calls`;
                body.appendChild(stats);
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
        nameSpan.textContent = 'ᚱ';
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

function attachMetaToLastAssistant(model, tokIn, tokOut, ctxTokens, ctxWindow, steps, toolCalls, thinking) {
    const target = currentAssistantDiv || chatMessages.querySelector('.chat-msg.assistant:last-child');
    if (!target) return;
    const sender = target.querySelector('.sender');
    if (!sender) return;
    // Remove old meta if any
    const oldMeta = sender.querySelector('.msg-meta');
    if (oldMeta) oldMeta.remove();
    // Model stays in header
    if (model) {
        const meta = document.createElement('span');
        meta.className = 'msg-meta';
        meta.textContent = (thinking && thinking !== 'off') ? `${model} ${thinking}` : model;
        const timeEl = sender.querySelector('.msg-time');
        if (timeEl) sender.insertBefore(meta, timeEl);
        else sender.appendChild(meta);
    }
    // Run stats go at the tail of the message body
    const totalTok = (tokIn||0) + (tokOut||0);
    if (steps || totalTok || toolCalls) {
        const body = target.querySelector('.body');
        if (body) {
            // Remove old stats footer if any
            const oldStats = body.querySelector('.run-stats');
            if (oldStats) oldStats.remove();
            const stats = document.createElement('div');
            stats.className = 'run-stats';
            stats.textContent = `⚡ ${steps||0} steps | ${totalTok} tokens | ${toolCalls||0} tool calls`;
            body.appendChild(stats);
            // Auto-scroll to show the stats line
            chatMessages.scrollTop = chatMessages.scrollHeight;
        }
    }
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
        // Has files: show buttons; restore from localStorage only on first call
        btnEdit.classList.remove('hidden');
        btnPreview.classList.remove('hidden');
        if (!_editorStateRestored) {
            _editorStateRestored = true;
            try {
                const se = localStorage.getItem('rune_show_edit');
                const sp = localStorage.getItem('rune_show_preview');
                showEdit    = se !== null ? se === '1' : true;
                showPreview = sp !== null ? sp === '1' : true;
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

    // Show split-title whenever any panel is visible (editor or preview)
    const splitTitle = document.getElementById('split-title');
    if (splitTitle) {
        splitTitle.style.display = (showEdit || showPreview) ? 'flex' : 'none';
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

    // Persist (only after initial state has been restored from localStorage)
    if (_editorStateRestored) {
        try {
            localStorage.setItem('rune_show_edit',    showEdit    ? '1' : '0');
            localStorage.setItem('rune_show_preview', showPreview ? '1' : '0');
        } catch {}
    }
    // Update split-title bar and mobile filename
    updateDocTitle(currentFilename);
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
        // Render LaTeX math expressions with KaTeX
        if (typeof renderMathInElement !== 'undefined') {
            renderMathInElement(preview, {
                delimiters: [
                    {left: '$$', right: '$$', display: true},
                    {left: '$', right: '$', display: false},
                    {left: '\\(', right: '\\)', display: false},
                    {left: '\\[', right: '\\]', display: true}
                ],
                throwOnError: false
            });
        }
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
    // Sync mobile model name
    const mobileModelEl = document.getElementById("mobile-model-name");
    if (mobileModelEl) mobileModelEl.textContent = activeModel;
    indicator.style.display = 'flex';
    // Admin can click the name to switch; show pointer cursor
    nameEl.style.cursor = (isAdmin && availableModels.length > 1) ? 'pointer' : 'default';
}

function updateThinkingSelect() {
    const selects = [
        document.getElementById('thinking-select'),
        document.getElementById('mobile-thinking-select')
    ].filter(Boolean);
    if (selects.length === 0) return;

    // Find current model's reasoning_efforts
    const currentModelObj = availableModels.find(m => (m.id || m) === activeModel);
    const efforts = (currentModelObj && currentModelObj.reasoning_efforts) || [];

    if (!isAdmin || efforts.length === 0) {
        selects.forEach(s => s.style.display = 'none');
        return;
    }

    // Build options: always include "off" plus the model's supported efforts
    selects.forEach(select => {
        select.innerHTML = '';
        const offOpt = document.createElement('option');
        offOpt.value = 'off';
        offOpt.textContent = 'off';
        select.appendChild(offOpt);

        efforts.forEach(level => {
            const opt = document.createElement('option');
            opt.value = level;
            opt.textContent = level;
            select.appendChild(opt);
        });

        select.value = currentThinking || 'off';
        select.style.display = '';
    });
}

function switchThinking(level) {
    if (isConnected) {
        api('model/thinking', { note_id: currentNoteId, thinking: level });
    }
}

function showModelDialog() {
    if (!isAdmin || availableModels.length <= 1) return;
    const listEl = document.getElementById('model-list');
    if (!listEl) return;
    listEl.innerHTML = '';
    availableModels.forEach(m => {
        const btn = document.createElement('button');
        const modelId = m.id || m;
        btn.className = 'model-option' + (modelId === activeModel ? ' active' : '');
        
        // Model name
        const nameSpan = document.createElement('span');
        nameSpan.className = 'model-option-name';
        nameSpan.textContent = modelId;
        btn.appendChild(nameSpan);
        
        // Metadata badges
        const badgeContainer = document.createElement('span');
        badgeContainer.className = 'model-badges';
        
        if (m.reasoning_efforts && m.reasoning_efforts.length > 0) {
            const reasonBadge = document.createElement('span');
            reasonBadge.className = 'model-reasoning-badge';
            reasonBadge.textContent = m.reasoning_efforts.join(' | ');
            badgeContainer.appendChild(reasonBadge);
        }
        
        if (m.context_window) {
            const ctxBadge = document.createElement('span');
            ctxBadge.className = 'model-ctx-badge';
            ctxBadge.textContent = formatContextWindow(m.context_window);
            badgeContainer.appendChild(ctxBadge);
        }
        
        btn.appendChild(badgeContainer);
        btn.onclick = () => { switchModel(modelId); hideModelDialog(); };
        listEl.appendChild(btn);
    });
    document.getElementById('model-modal').classList.remove('hidden');
}

function formatContextWindow(tokens) {
    if (tokens >= 1000000) return (tokens / 1000000).toFixed(0) + 'M';
    if (tokens >= 1000) return (tokens / 1000).toFixed(0) + 'K';
    return tokens.toString();
}


function hideModelDialog() {
    document.getElementById('model-modal').classList.add('hidden');
}

function switchModel(model) {
    if (isConnected) {
        api('model/switch', { model, note_id: currentNoteId });
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
    const s = notes.find(x => x.id === currentNoteId);
    const noteName = s ? s.name : '';
    const file = (fileList && fileList.length > 0) ? currentFilename : null;

    // Check public visibility for link generation
    const notePublic = s && !!s.public;
    const filePublic = notePublic && file && s.fileVisibility && !!s.fileVisibility[file];

    function noteLink(label) {
        if (!notePublic) return document.createTextNode(label);
        const a = document.createElement('a');
        a.href = '/notes/' + encodeURIComponent(currentNoteId) + '/';
        a.target = '_blank'; a.rel = 'noopener';
        a.className = 'title-public-link';
        a.textContent = label;
        return a;
    }
    function fileLink(label) {
        const slug = (label || '').replace(/\.md$/, '');
        if (!filePublic) return document.createTextNode(label);
        const a = document.createElement('a');
        a.href = '/notes/' + encodeURIComponent(currentNoteId) + '/' + encodeURIComponent(slug);
        a.target = '_blank'; a.rel = 'noopener';
        a.className = 'title-public-link';
        a.textContent = label;
        return a;
    }

    // Build innerHTML fragment for an element
    function buildTitleNodes(el) {
        el.innerHTML = '';
        if (noteName && file) {
            el.appendChild(noteLink(noteName));
            const sep = document.createElement('span');
            sep.innerHTML = ' – ';
            el.appendChild(sep);
            el.appendChild(fileLink(file));
        } else if (noteName) {
            el.appendChild(noteLink(noteName));
        } else if (file) {
            el.appendChild(fileLink(file));
        }
    }

    // Mobile header
    const mfn = document.getElementById('mobile-filename');
    if (mfn) buildTitleNodes(mfn);

    // Desktop split-view title bar
    const splitTitle = document.getElementById('split-title');
    if (splitTitle) buildTitleNodes(splitTitle);
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

async function createFile() {
    const name = await showDialog({ title: 'New File', message: 'Filename must end in .md', input: true, placeholder: 'example.md' });
    if (!name) return;
    if (!name.endsWith('.md')) { addSystemMessage('Error: filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(name)) { addSystemMessage('Error: invalid filename'); return; }
    api('file/create', { note_id: currentNoteId, name });
}

async function deleteCurrentFile() {
    const ok = await showDialog({ title: 'Delete File', message: 'Delete ' + currentFilename + '?', danger: true });
    if (!ok) return;
    api('file/delete', { note_id: currentNoteId, name: currentFilename });
}

async function switchFile(name) {
    const data = await api('file/switch', { note_id: currentNoteId, name });
    if (!data || !data.ok) return;
    currentFilename = data.filename || name;
    specContent = data.content || '';
    setEditorValue(specContent);
    if (showPreview) renderPreview();
    updateDocTitle(currentFilename);
    try { localStorage.setItem('rune_file', currentFilename); } catch {}
}

function renameCurrentFile(newName) {
    const clean = newName.trim();
    if (!clean || clean === currentFilename) return;
    if (!clean.endsWith('.md')) { addSystemMessage('Error: filename must end in .md'); return; }
    if (!/^[a-zA-Z0-9_\-\.]+\.md$/.test(clean)) { addSystemMessage('Error: invalid filename'); return; }
    api('file/rename', { note_id: currentNoteId, old_name: currentFilename, new_name: clean });
}


const STATUS_EMOJI = {
    idle:         '🟢',
    typing:       '⌨️',
    thinking:     '🤔',
    tool:         '🔧',
    disconnected: '🔴',
};
function setStatus(state) {
    statusIndicator.className = `status ${state}`;
    statusIndicator.textContent = STATUS_EMOJI[state] || '⚪';
    statusIndicator.title = state;
    // Sync mobile status
    const mobileStatus = document.getElementById('mobile-status');
    if (mobileStatus) {
        mobileStatus.className = `status ${state}`;
        mobileStatus.textContent = STATUS_EMOJI[state] || '⚪';
    }
}

function setToolStatus(toolName) {
    statusIndicator.className = 'status tool';
    statusIndicator.textContent = '🔧';
    statusIndicator.title = `tool: ${toolName}`;
    const mobileStatus = document.getElementById('mobile-status');
    if (mobileStatus) {
        mobileStatus.className = 'status tool';
        mobileStatus.textContent = '🔧';
    }
}

function clearToolStatus() {
    // Revert to thinking (tool ended, waiting for next LLM response)
    setStatus('thinking');
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

        // Visibility icon for note
        {
            const notePublic = !!s.public;
            const visIcon = document.createElement('span');
            visIcon.className = 'visibility-icon' + (isAdmin ? '' : ' readonly');
            visIcon.title = notePublic ? 'Public (click to make private)' : 'Private (click to make public)';
            visIcon.textContent = notePublic ? '👁' : '🙈';
            if (isAdmin) {
                visIcon.onclick = (e) => {
                    e.stopPropagation();
                    api('note/visibility', { note_id: s.id, public: !notePublic });
                };
            }
            folderRow.appendChild(visIcon);
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
                renameBtn.onclick = async (e) => {
                    e.stopPropagation();
                    const newName = await showDialog({ title: 'Rename File', input: true, inputValue: fname, placeholder: 'new-name.md' });
                    if (newName && newName !== fname) {
                        api('file/rename', { note_id: s.id, old_name: fname, new_name: newName });
                    }
                };
                fileActions.appendChild(renameBtn);

                const delBtn = document.createElement('button');
                delBtn.textContent = '🗑';
                delBtn.title = 'Delete';
                delBtn.onclick = async (e) => {
                    e.stopPropagation();
                    const ok = await showDialog({ title: 'Delete File', message: 'Delete "' + fname + '"?', danger: true });
                    if (ok) api('file/delete', { note_id: s.id, name: fname });
                };
                fileActions.appendChild(delBtn);

                fileRow.appendChild(fileActions);
            }

            // Visibility icon for file
            {
                const fileVisibility = s.fileVisibility || {};
                const filePublic = !!fileVisibility[fname];
                const visIcon = document.createElement('span');
                visIcon.className = 'visibility-icon' + (isAdmin ? '' : ' readonly');
                visIcon.title = filePublic ? 'Public (click to make private)' : 'Private (click to make public)';
                visIcon.textContent = filePublic ? '👁' : '🙈';
                if (isAdmin) {
                    visIcon.onclick = (e) => {
                        e.stopPropagation();
                        api('file/visibility', { note_id: s.id, filename: fname, public: !filePublic });
                    };
                }
                fileRow.appendChild(visIcon);
            }

            fileRow.onclick = () => {
                if (s.id !== currentNoteId) {
                    // Switching notes: pass fname so switchNote opens the right file directly
                    switchNote(s.id, fname);
                } else {
                    // Already on this note: just switch file
                    switchFile(fname);
                }
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





async function switchNote(sessionId, forceFile = null) {
    if (sessionId === currentNoteId) return;
    currentNoteId = sessionId;
    localStorage.setItem('rune_note', sessionId);
    renderNoteList();
    updateChatInputState();
    updatePageTitle();

    // Close existing SSE immediately (stop receiving events from old room)
    if (evtSource) { evtSource.close(); evtSource = null; }

    const data = await api('note/switch', { note_id: sessionId });
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
            const fileData = await api('file/switch', { note_id: sessionId, name: targetFile });
            specContent = (fileData && fileData.content) || '';
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

async function deleteCurrentNote() {
    if (!settingsNoteId) return;
    const ok = await showDialog({ title: 'Delete Note', message: 'Delete this note? Chat history will be preserved.', danger: true, okLabel: 'Delete Note' });
    if (!ok) return;
    const deletedId = settingsNoteId;
    api('note/delete', { note_id: deletedId });
    hideNoteSettings();
    if (currentNoteId === deletedId) {
        // Close current SSE to prevent reconnect loop to deleted note
        if (evtSource) { evtSource.close(); evtSource = null; }
        isConnected = false;
        currentNoteId = '';
        localStorage.removeItem('rune_note');
        updateChatInputState();
        // Switch to another available note
        fetchNoteListAndConnect();
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



// ═══════════════════════════════════════════════════════════════════════════
// MOBILE EDITOR — contenteditable + highlight.js
// ═══════════════════════════════════════════════════════════════════════════

let mobileEditorInitialized = false;

function initMobileEditor() {
    const mobileEd = document.getElementById('mobile-editor');
    if (!mobileEd || mobileEditorInitialized) return;
    mobileEditorInitialized = true;

    // Set initial content
    if (specContent) {
        mobileEditorHighlight(mobileEd, specContent);
    }

    // Input handler — debounced highlight + save
    let mobileDebounce = null;
    mobileEd.addEventListener('input', () => {
        const text = getMobileEditorText(mobileEd);
        specContent = text;
        editorDirty = true;

        // Sync to hidden textarea (for desktop compatibility)
        const textarea = document.getElementById('editor');
        if (textarea) textarea.value = text;

        clearTimeout(mobileDebounce);
        mobileDebounce = setTimeout(() => {
            // Re-highlight (preserving cursor)
            mobileEditorHighlight(mobileEd, text);
            // Live preview
            if (showPreview) renderPreview();
            // Save to server
            if (editorDirty && currentNoteId) {
                api('file/update', { note_id: currentNoteId, filename: currentFilename, content: specContent });
                editorDirty = false;
            }
        }, 400);
    });

    // Paste: plain text only
    mobileEd.addEventListener('paste', (e) => {
        e.preventDefault();
        const text = e.clipboardData.getData('text/plain');
        document.execCommand('insertText', false, text);
    });

    // Tab key support
    mobileEd.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') {
            e.preventDefault();
            document.execCommand('insertText', false, '    ');
        }
    });
}

function getMobileEditorText(el) {
    // Extract plain text from contenteditable
    // Use innerText which respects line breaks
    return el.innerText || '';
}

function setMobileEditorContent(text) {
    const mobileEd = document.getElementById('mobile-editor');
    if (!mobileEd) return;
    mobileEditorHighlight(mobileEd, text);
}

function mobileEditorHighlight(el, text) {
    // Save cursor position
    const sel = window.getSelection();
    let cursorOffset = 0;
    if (sel.rangeCount > 0 && el.contains(sel.anchorNode)) {
        cursorOffset = getTextOffset(el, sel.anchorNode, sel.anchorOffset);
    }

    // Render highlighted HTML
    if (typeof highlightMarkdownEditor === 'function' && text.length > 0) {
        el.innerHTML = highlightMarkdownEditor(text.endsWith('\n') ? text : text + '\n');
    } else {
        el.textContent = text;
    }

    // Restore cursor
    if (document.activeElement === el || el.contains(document.activeElement)) {
        restoreCursor(el, cursorOffset);
    }
}

function getTextOffset(root, node, offset) {
    // Calculate total text offset from start of root to (node, offset)
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
    let total = 0;
    let current;
    while ((current = walker.nextNode())) {
        if (current === node) {
            return total + offset;
        }
        total += current.textContent.length;
    }
    return total + offset;
}

function restoreCursor(el, offset) {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, null, false);
    let remaining = offset;
    let current;
    while ((current = walker.nextNode())) {
        if (remaining <= current.textContent.length) {
            const range = document.createRange();
            range.setStart(current, remaining);
            range.collapse(true);
            const sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            return;
        }
        remaining -= current.textContent.length;
    }
    // If we couldn't place cursor, put it at end
    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
}

// ═══════════════════════════════════════════════════════════════════════════
// MOBILE UI — Accordion + Drawer
// ═══════════════════════════════════════════════════════════════════════════

const mobileQuery = window.matchMedia('(max-width: 768px)');
let isMobile = mobileQuery.matches;
let mobileInitDone = false;

function setupMobileUI() {
    if (!isMobile) return;
    mobileInitDone = true;
// Fix mobile viewport height (iOS Safari/Android Chrome address bar)    function setMobileVh() {        const vh = window.innerHeight;        document.documentElement.style.setProperty("--mobile-vh", vh + "px");    }    setMobileVh();    window.addEventListener("resize", setMobileVh);    if (window.visualViewport) window.visualViewport.addEventListener("resize", setMobileVh);

    // Restore accordion states from localStorage
    ['preview', 'editor', 'chat'].forEach(section => {
        const key = 'rune_mobile_' + section;
        const saved = localStorage.getItem(key);
        const el = document.getElementById('mobile-section-' + section);
        if (!el) return;
        const defaultExpanded = (section !== 'editor');
        const expanded = saved !== null ? saved === '1' : defaultExpanded;
        if (expanded) {
            el.classList.add('expanded');
        } else {
            el.classList.remove('expanded');
        }
    });

    // Move chat DOM into mobile chat section
    moveChatToMobile();

    // Force preview to render if expanded
    const previewSection = document.getElementById('mobile-section-preview');
    if (previewSection && previewSection.classList.contains('expanded')) {
        const pc = document.getElementById('preview-container');
        if (pc) pc.classList.remove('hidden');
        renderPreview();
    }

    // Force editor content sync
    const editor = document.getElementById('editor');
    if (editor && !editor.value && specContent) {
        editor.value = specContent;
    }

    updateMobileFilename();
    initMobileEditor();
}

function teardownMobileUI() {
    if (!mobileInitDone) return;
    moveChatToDesktop();
    mobileInitDone = false;
}

function toggleMobileSection(section) {
    const el = document.getElementById('mobile-section-' + section);
    if (!el) return;
    el.classList.toggle('expanded');
    const expanded = el.classList.contains('expanded');
    localStorage.setItem('rune_mobile_' + section, expanded ? '1' : '0');

    if (section === 'preview' && expanded) {
        const pc = document.getElementById('preview-container');
        if (pc) pc.classList.remove('hidden');
        renderPreview();
    }
}

function openNotesDrawer() {
    const drawer = document.getElementById('mobile-drawer');
    if (!drawer) return;
    drawer.classList.remove('hidden');
    renderMobileNoteTree();
}

function closeNotesDrawer() {
    const drawer = document.getElementById('mobile-drawer');
    if (drawer) drawer.classList.add('hidden');
}

function renderMobileNoteTree() {
    const dest = document.getElementById('mobile-drawer-body');
    if (!dest) return;
    dest.innerHTML = '';

    notes.forEach(s => {
        const noteSection = document.createElement('div');
        noteSection.className = 'mobile-note-section';

        // Note header (folder)
        const noteHeader = document.createElement('div');
        noteHeader.className = 'mobile-note-header' + (s.id === currentNoteId ? ' active' : '');
        noteHeader.innerHTML = '<span class="mobile-note-icon">' + (s.id === currentNoteId ? '📂' : '📁') + '</span><span class="mobile-note-name">' + (s.name || s.id) + '</span>';
        noteHeader.onclick = () => {
            if (s.id !== currentNoteId) {
                switchNote(s.id);
                closeNotesDrawer();
            }
        };
        noteSection.appendChild(noteHeader);

        // Files list
        const files = s.files || [];
        files.forEach(fname => {
            const fileRow = document.createElement('div');
            fileRow.className = 'mobile-file-row' + (s.id === currentNoteId && fname === currentFilename ? ' active' : '');
            fileRow.innerHTML = '<span class="mobile-file-icon">📄</span><span class="mobile-file-name">' + fname + '</span>';
            fileRow.onclick = () => {
                if (s.id !== currentNoteId) {
                    switchNote(s.id, fname);
                } else {
                    switchFile(fname);
                }
                closeNotesDrawer();
            };
            noteSection.appendChild(fileRow);
        });

        dest.appendChild(noteSection);
    });
}

function moveChatToMobile() {
    const sectionBody = document.querySelector('#mobile-section-chat .mobile-section-body');
    if (!sectionBody) return;

    const messages = document.getElementById('chat-messages');
    const inputArea = document.querySelector('#panel-right .chat-input-area');
    if (messages) sectionBody.appendChild(messages);
    if (inputArea) sectionBody.appendChild(inputArea);
}

function moveChatToDesktop() {
    const panelRight = document.getElementById('panel-right');
    if (!panelRight) return;
    const chatBody = panelRight.querySelector('.chat-body');
    const panelContent = panelRight.querySelector('.panel-content');

    const messages = document.getElementById('chat-messages');
    const inputArea = document.querySelector('.chat-input-area');

    if (chatBody && messages) chatBody.appendChild(messages);
    if (panelContent && inputArea) panelContent.appendChild(inputArea);
}

function updateMobileFilename() {
    const el = document.getElementById('mobile-filename');
    if (el) el.textContent = currentFilename || '';
}

// Listen for viewport changes
mobileQuery.addEventListener('change', (e) => {
    isMobile = e.matches;
    if (isMobile) {
        setupMobileUI();
    } else {
        teardownMobileUI();
    }
});

// Init mobile immediately (script runs at end of body, DOM is ready)
if (isMobile) setupMobileUI();
