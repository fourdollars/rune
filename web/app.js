// Rune WebUI — app.js
// Three-panel layout: Sessions | spec.md | Chat

'use strict';

// --- State ---
let ws = null;
let showEdit    = true;
let showPreview = false;
let specContent = '';
let isConnected = false;
let editorDirty = false;
let debounceTimer = null;
let specVersion = 0;
let myNickname = '';
let myToken = '';
let isAdmin = false;

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
                ws.send(JSON.stringify({ type: 'spec_update', content: specContent }));
                editorDirty = false;
            }
        }, 300);
    });

    textarea.addEventListener('scroll', syncScroll);

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
        addSystemMessage('Disconnected. Reconnecting...');
        setTimeout(connect, 2000);
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

        case 'chat_done':
            finalizeAssistantMessage();
            break;

        case 'status':
            setStatus(msg.state);
            break;

        case 'auth_result':
            isAdmin = msg.is_admin;
            if (isAdmin) addSystemMessage('👑 You are connected as admin');
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

function addChatMessage(nickname, content) {
    const isMe = nickname === myNickname;
    const div = document.createElement('div');
    div.className = `chat-msg ${isMe ? 'user' : 'other'}`;

    const sender = document.createElement('div');
    sender.className = 'sender';
    sender.textContent = isMe ? `🧑 ${nickname} (you)` : `👤 ${nickname}`;

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
    addSystemMessage('── 對話記錄 ──');
    for (const m of messages) {
        if (m.role === 'user') {
            addChatMessage(m.nickname, m.content);
        } else if (m.role === 'assistant') {
            // Render as completed assistant message
            const div = document.createElement('div');
            div.className = 'chat-msg assistant';
            const sender = document.createElement('div');
            sender.className = 'sender';
            sender.textContent = 'ᚱᚢᚾᛖ';
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
    addSystemMessage('── 目前對話 ──');
    chatMessages.scrollTop = chatMessages.scrollHeight;
}

function updateOnlineCount(count) {
    const el = document.getElementById('online-count');
    if (el) el.textContent = count;
}

let currentAssistantEl = null;
let currentAssistantText = '';

function appendToLastAssistant(token) {
    if (!currentAssistantEl) {
        const div = document.createElement('div');
        div.className = 'chat-msg assistant';

        const sender = document.createElement('div');
        sender.className = 'sender';
        sender.textContent = 'ᚱᚢᚾᛖ';

        const body = document.createElement('div');
        body.className = 'body';

        div.appendChild(sender);
        div.appendChild(body);
        chatMessages.appendChild(div);
        currentAssistantEl = body;
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
}

function showApprovalRequest(id, detail) {
    if (!isAdmin) return; // only admin sees approval requests
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
function setStatus(state) {
    statusIndicator.className = `status ${state}`;
    statusIndicator.textContent = `● ${state}`;
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

// --- Init ---
initEditor();
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
