// Rune WebUI — app.js
// Three-panel layout: Notes | Editor | Chat

'use strict';

// --- URL Routing helpers ---
// Parse /notes/{note}/{file} from pathname; strip trailing .md
function parseNotesUrl() {
    const path = window.location.pathname;
    // Strip .md suffix and redirect to clean URL
    if (path.endsWith('.md')) {
        const clean = path.slice(0, -3);
        history.replaceState(null, '', clean);
        return parseNotesUrlFromPath(clean);
    }
    return parseNotesUrlFromPath(path);
}
function parseNotesUrlFromPath(path) {
    const m = path.match(/^\/notes\/([^\/]+)\/([^\/]+)$/);
    if (m) return { noteId: decodeURIComponent(m[1]), file: decodeURIComponent(m[2]) };
    const m2 = path.match(/^\/notes\/([^\/]+)\/?$/);
    if (m2) return { noteId: decodeURIComponent(m2[1]), file: null };
    return { noteId: null, file: null };
}
function updateBrowserUrl(noteId, filename) {
    if (!noteId) return;
    const slug = filename ? filename.replace(/\.md$/, '') : null;
    const url = slug
        ? '/notes/' + encodeURIComponent(noteId) + '/' + encodeURIComponent(slug)
        : '/notes/' + encodeURIComponent(noteId) + '/';
    if (window.location.pathname !== url) {
        history.pushState({ noteId, filename }, '', url);
    }
}

// Pending note/file from URL (set before auth, consumed after login)
let _pendingNoteId = null;
let _pendingFile   = null;

(function initRouting() {
    const parsed = parseNotesUrl();
    if (parsed.noteId) {
        _pendingNoteId = parsed.noteId;
        _pendingFile   = parsed.file;
    }
    // Handle browser back/forward
    window.addEventListener('popstate', (e) => {
        if (e.state && e.state.noteId) {
            if (e.state.noteId !== currentNoteId) {
                switchNote(e.state.noteId, e.state.filename || null);
            } else if (e.state.filename && e.state.filename !== currentFilename) {
                switchFile(e.state.filename);
            }
        }
    });
})();

// --- State ---
let showEdit    = true;
let showPreview = true;
let syncScrollEnabled = true;
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
let isAdmin = false;
let isGuest = false;
let availableModels = [];
let currentThinking = 'off';

// --- Note state ---
let notes = [];
let currentNoteId = '';
let dirBrowserTargetInput = null;
let settingsNoteId = null;
let selectedNoteIcon = null;

let activeModel = '';
let lastContextTokens = null;

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
        // Wrap code body in a block-level span for background styling
        result += `<span class="editor-code-block">${codeHtml}</span>`;

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

let editorInstance = null;

function editorTheme() {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'rune-dark' : 'rune-light';
}

function initEditor() {
    const wrapper = document.getElementById('editor-wrapper');
    if (!wrapper) return;

    editorInstance = CodeMirror(wrapper, {
        mode: 'markdown',
        lineNumbers: true,
        lineWrapping: true,
        theme: editorTheme(),
        value: specContent || '',
        tabSize: 4,
        indentUnit: 4,
        viewportMargin: 100,
        extraKeys: {
            "Ctrl-B": () => insertFormat('bold'),
            "Cmd-B": () => insertFormat('bold'),
            "Ctrl-I": () => insertFormat('italic'),
            "Cmd-I": () => insertFormat('italic'),
            "Ctrl-H": () => insertFormat('header'),
            "Cmd-H": () => insertFormat('header'),
            "Ctrl-K": () => insertFormat('link'),
            "Cmd-K": () => insertFormat('link'),
        }
    });

    // Update theme when OS color scheme changes
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onSchemeChange = () => { if (editorInstance) editorInstance.setOption('theme', editorTheme()); };
    if (mq.addEventListener) mq.addEventListener('change', onSchemeChange);
    else if (mq.addListener) mq.addListener(onSchemeChange); // Safari <14 fallback

    editorInstance.on('change', () => {
        specContent = editorInstance.getValue();
        editorDirty = true;
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            if (showPreview) renderPreview();
            if (editorDirty && currentNoteId) {
                api('file/update', { note_id: currentNoteId, filename: currentFilename, content: specContent });
                editorDirty = false;
            }
        }, 300);
    });

    editorInstance.on('scroll', () => {
        if (typeof handleEditorScroll === 'function') handleEditorScroll();
    });
}

function insertFormat(type) {
    if (!editorInstance) return;
    editorInstance.focus();

    const doc = editorInstance.getDoc();
    const selection = doc.getSelection();
    const cursor = doc.getCursor();

    switch (type) {
        case 'bold':
            doc.replaceSelection('**' + (selection || 'text') + '**');
            if (!selection) {
                const start = doc.getCursor('start');
                doc.setSelection({ line: start.line, ch: start.ch - 6 }, { line: start.line, ch: start.ch - 2 });
            }
            break;
        case 'italic':
            doc.replaceSelection('*' + (selection || 'text') + '*');
            if (!selection) {
                const start = doc.getCursor('start');
                doc.setSelection({ line: start.line, ch: start.ch - 5 }, { line: start.line, ch: start.ch - 1 });
            }
            break;
        case 'header':
            const lineNo = cursor.line;
            const lineText = doc.getLine(lineNo);
            const headerMatch = lineText.match(/^(#{1,4})\s(.*)$/);
            
            if (headerMatch) {
                const currentLevel = headerMatch[1].length;
                const content = headerMatch[2];
                if (currentLevel < 4) {
                    const newPrefix = '#'.repeat(currentLevel + 1) + ' ';
                    doc.replaceRange(newPrefix + content, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
                } else {
                    doc.replaceRange(content, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
                }
            } else {
                doc.replaceRange('# ' + lineText, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
            }
            break;
        case 'link':
            const linkUrl = selection.match(/^https?:\/\//) ? selection : 'https://example.com';
            const linkText = selection.match(/^https?:\/\//) ? 'Link' : (selection || 'Link text');
            doc.replaceSelection(`[${linkText}](${linkUrl})`);
            break;
        case 'image':
            const imgUrl = selection.match(/^https?:\/\//) ? selection : 'https://example.com/image.png';
            const imgAlt = selection.match(/^https?:\/\//) ? 'Alt' : (selection || 'Alt text');
            doc.replaceSelection(`![${imgAlt}](${imgUrl})`);
            break;
        case 'code':
            doc.replaceSelection('```\n' + (selection || 'code') + '\n```');
            break;
        case 'ul':
            transformLines(line => line.startsWith('- ') ? line.substring(2) : '- ' + line);
            break;
        case 'ol':
            transformLines((line, i) => {
                const match = line.match(/^(\d+)\.\s(.*)$/);
                return match ? match[2] : (i + 1) + '. ' + line;
            });
            break;
        case 'task':
            transformLines(line => {
                const match = line.match(/^- \[[ xX]\] (.*)$/);
                return match ? match[1] : '- [ ] ' + line;
            });
            break;
        case 'table':
            doc.replaceSelection(
                '\n| Column 1 | Column 2 |\n' +
                '| -------- | -------- |\n' +
                '| Cell 1   | Cell 2   |\n'
            );
            break;
    }

    function transformLines(fn) {
        editorInstance.operation(() => {
            const start = doc.getCursor('start').line;
            const end = doc.getCursor('end').line;
            for (let i = start; i <= end; i++) {
                const line = doc.getLine(i);
                doc.replaceRange(fn(line, i - start), { line: i, ch: 0 }, { line: i, ch: line.length });
            }
        });
    }
}

function setEditorValue(text) {
    specContent = text;
    if (editorInstance) {
        if (editorInstance.getValue() !== text) {
            const cursor = editorInstance.getCursor();
            editorInstance.setValue(text);
            editorInstance.setCursor(cursor);
        }
    }
}


// --- marked.js configuration (v15+) ---
if (typeof marked !== 'undefined') {
    const getLineAttr = (token) => {
        return (token && typeof token.startLine === 'number') ? ` data-line="${token.startLine}"` : '';
    };

    const renderer = new marked.Renderer();

    // Helper to wrap renderer methods to inject data-line attributes
    const wrap = (methodName) => {
        const original = renderer[methodName];
        renderer[methodName] = function(token) {
            const html = original.call(this, token);
            const lineAttr = getLineAttr(token);
            if (lineAttr && html) {
                return html.replace(/^<([a-z0-9]+)/i, `<$1${lineAttr}`);
            }
            return html;
        };
    };

    wrap('paragraph');
    wrap('heading');
    wrap('blockquote');
    wrap('list');
    wrap('listitem');

    renderer.code = function(token) {
        const { text, lang } = token;
        const lineAttr = getLineAttr(token);
        if (lang && lang.toLowerCase() === 'mermaid') {
            const id = 'mermaid-' + Math.random().toString(36).slice(2);
            return `<div class="mermaid-block"${lineAttr} id="${id}" data-src="${text.replace(/"/g,'&quot;')}"></div>`;
        }
        const raw = text.replace(/"/g, '&quot;');
        if (typeof hljs !== 'undefined') {
            const language = lang && hljs.getLanguage(lang) ? lang : null;
            const highlighted = language
                ? hljs.highlight(text, { language }).value
                : hljs.highlightAuto(text).value;
            const langClass = language ? ` class="language-${language}"` : '';
            return `<pre class="hljs-pre"${lineAttr} data-raw="${raw}"><code class="hljs${langClass}">${highlighted}</code></pre>`;
        }
        const safe = text.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        return `<pre class="hljs-pre"${lineAttr} data-raw="${raw}"><code>${safe}</code></pre>`;
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

// Enter key on nickname input (no-op: nickname modal removed, login handled by GitHub OAuth)
// document.getElementById('nickname-input') may not exist in new UI

// --- Connection ---
async function fetchNoteListAndConnect() {
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
    params.set('note_id', targetNote);

    evtSource = new EventSource('/api/events?' + params.toString(), { withCredentials: true });
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
    try {
        const resp = await fetch('/api/' + endpoint, {
            method: 'POST',
            headers,
            credentials: 'include',
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
            addSystemMessage('📦 Archived ' + (msg.count || 0) + ' message(s) → ' + msg.filename);
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
        if (showEdit || showPreview) {
            panelCenter.classList.remove('hidden');
        }
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
                renderChatMath(body);
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

    // Restore context overlay from the last assistant message that has context_tokens
    const lastWithCtx = [...messages].reverse().find(
        m => m.role === 'assistant' && m.context_tokens != null && m.model
    );
    if (lastWithCtx) {
        const modelEntry = availableModels.find(m => m.id === lastWithCtx.model);
        if (modelEntry && modelEntry.context_window) {
            updateContextOverlay(lastWithCtx.context_tokens, modelEntry.context_window);
        }
    }
}

function renderChatMath(el) {
    if (typeof renderMathInElement !== 'undefined') {
        renderMathInElement(el, {
            delimiters: [
                {left: '$$', right: '$$', display: true},
                {left: '$', right: '$', display: false},
                {left: '\\(', right: '\\)', display: false},
                {left: '\\[', right: '\\]', display: true}
            ],
            throwOnError: false
        });
    }
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
        renderChatMath(currentAssistantEl);
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
    lastContextTokens = ctxTokens;
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

    // Show split-title-bar whenever any panel is visible (editor or preview)
    const splitTitle = document.getElementById('split-title-bar');
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
function toggleSyncScroll() {
    syncScrollEnabled = !syncScrollEnabled;
    const btn = document.getElementById('btn-sync-scroll');
    if (btn) {
        if (syncScrollEnabled) btn.classList.add('active');
        else btn.classList.remove('active');
    }
    try {
        localStorage.setItem('rune_sync_scroll', syncScrollEnabled ? '1' : '0');
    } catch {}
}

let activeScrollSource = null;
let scrollTimeout = null;

function handleEditorScroll() {
    if (!syncScrollEnabled || !showPreview || !showEdit || !editorInstance) return;
    if (activeScrollSource === 'preview') return;
    
    activeScrollSource = 'editor';
    clearTimeout(scrollTimeout);

    const scrollInfo = editorInstance.getScrollInfo();
    const topLine = editorInstance.lineAtHeight(scrollInfo.top, 'local');

    const elements = Array.from(previewContainer.querySelectorAll('[data-line]'));
    if (elements.length === 0) {
        activeScrollSource = null;
        return;
    }

    let low = 0;
    let high = elements.length - 1;
    let targetIdx = 0;

    while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        const line = parseInt(elements[mid].dataset.line, 10);
        if (line <= topLine) {
            targetIdx = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }

    const elLow = elements[targetIdx];
    const elHigh = elements[targetIdx + 1];
    
    const lineLow = parseInt(elLow.dataset.line, 10);
    const offsetLow = elLow.offsetTop;
    let targetScrollTop = offsetLow;

    if (elHigh) {
        const lineHigh = parseInt(elHigh.dataset.line, 10);
        const offsetHigh = elHigh.offsetTop;
        const ratio = (topLine - lineLow) / (lineHigh - lineLow || 1);
        targetScrollTop = offsetLow + ratio * (offsetHigh - offsetLow);
    }

    previewContainer.scrollTop = targetScrollTop;

    scrollTimeout = setTimeout(() => { activeScrollSource = null; }, 100);
}

function handlePreviewScroll() {
    if (!syncScrollEnabled || !showPreview || !showEdit || !editorInstance) return;
    if (activeScrollSource === 'editor') return;
    
    activeScrollSource = 'preview';
    clearTimeout(scrollTimeout);

    const scrollTop = previewContainer.scrollTop;
    const elements = Array.from(previewContainer.querySelectorAll('[data-line]'));
    if (elements.length === 0) {
        activeScrollSource = null;
        return;
    }

    let low = 0;
    let high = elements.length - 1;
    let targetIdx = 0;

    while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        const offset = elements[mid].offsetTop;
        if (offset <= scrollTop) {
            targetIdx = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }

    const elLow = elements[targetIdx];
    const elHigh = elements[targetIdx + 1];
    
    const lineLow = parseInt(elLow.dataset.line, 10);
    const offsetLow = elLow.offsetTop;
    let targetEditorLine = lineLow;

    if (elHigh) {
        const lineHigh = parseInt(elHigh.dataset.line, 10);
        const offsetHigh = elHigh.offsetTop;
        const ratio = (scrollTop - offsetLow) / (offsetHigh - offsetLow || 1);
        targetEditorLine = lineLow + ratio * (lineHigh - lineLow);
    }
    targetEditorLine = Math.round(targetEditorLine);
    targetEditorLine = Math.max(0, Math.min(editorInstance.lineCount() - 1, targetEditorLine));

    const editorScrollTop = editorInstance.heightAtLine(targetEditorLine, 'local');
    editorInstance.scrollTo(null, editorScrollTop);

    scrollTimeout = setTimeout(() => { activeScrollSource = null; }, 100);
}

function initPreviewScrollSync() {
    previewContainer.addEventListener('scroll', handlePreviewScroll);
}

function renderPreview() {
    if (typeof marked !== 'undefined') {
        const assignLines = (tokens, startLine = 0) => {
            let currentLine = startLine;
            for (const token of tokens) {
                token.startLine = currentLine;
                if (token.items && token.items.length > 0) {
                    let itemLine = currentLine;
                    for (const item of token.items) {
                        item.startLine = itemLine;
                        if (item.tokens) {
                            assignLines(item.tokens, itemLine);
                        }
                        const itemNewlines = (item.raw.match(/\n/g) || []).length;
                        itemLine += itemNewlines;
                    }
                }
                if (token.tokens && token.tokens.length > 0) {
                    assignLines(token.tokens, currentLine);
                }
                const newlines = (token.raw.match(/\n/g) || []).length;
                currentLine += newlines;
            }
        };

        const tokens = marked.lexer(specContent);
        assignLines(tokens, 0);
        preview.innerHTML = marked.parser(tokens);
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

    // Build options: prepend "off" only when "none" is not already in the list
    selects.forEach(select => {
        select.innerHTML = '';
        if (!efforts.includes('none')) {
            const offOpt = document.createElement('option');
            offOpt.value = 'off';
            offOpt.textContent = 'off';
            select.appendChild(offOpt);
        }

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

    // Set provider in title
    const titleEl = document.getElementById('model-modal-title');
    if (titleEl) {
        const firstModel = availableModels.find(m => m.provider);
        const providerName = firstModel ? firstModel.provider : '';
        if (providerName) {
            let friendlyProvider = providerName;
            const lower = providerName.toLowerCase();
            if (lower === 'gemini') {
                friendlyProvider = 'Google Gemini';
            } else if (lower === 'github-copilot') {
                friendlyProvider = 'GitHub Copilot';
            } else if (lower === 'openrouter') {
                friendlyProvider = 'OpenRouter';
            } else if (lower === 'openai') {
                friendlyProvider = 'OpenAI';
            } else if (lower === 'openai-compatible') {
                friendlyProvider = 'OpenAI compatible';
            } else {
                friendlyProvider = providerName.charAt(0).toUpperCase() + providerName.slice(1);
            }
            titleEl.textContent = `Switch Model (${friendlyProvider})`;
        } else {
            titleEl.textContent = 'Switch Model';
        }
    }

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

    const searchInput = document.getElementById('model-search-input');
    if (searchInput) {
        searchInput.value = '';
        searchInput.oninput = (e) => {
            const query = e.target.value.toLowerCase().trim();
            const buttons = listEl.querySelectorAll('.model-option');
            buttons.forEach(btn => {
                const nameSpan = btn.querySelector('.model-option-name');
                if (nameSpan) {
                    const name = nameSpan.textContent.toLowerCase();
                    if (name.includes(query)) {
                        btn.style.display = 'flex';
                    } else {
                        btn.style.display = 'none';
                    }
                }
            });
        };
    }

    document.getElementById('model-modal').classList.remove('hidden');
    if (searchInput) {
        setTimeout(() => searchInput.focus(), 50);
    }
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
    // Close SSE
    if (evtSource) { evtSource.close(); evtSource = null; }
    // Clear session from localStorage
    localStorage.removeItem('rune_session_id');
    localStorage.removeItem('rune_nickname');
    // Server clears HttpOnly cookie via /auth/logout redirect
    window.location.href = '/auth/logout';
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
        a.href = '/public/' + encodeURIComponent(currentNoteId) + '/';
        a.target = '_blank'; a.rel = 'noopener';
        a.className = 'title-public-link';
        a.textContent = label;
        return a;
    }
    function fileLink(label) {
        const slug = (label || '').replace(/\.md$/, '');
        if (!filePublic) return document.createTextNode(label);
        const a = document.createElement('a');
        a.href = '/public/' + encodeURIComponent(currentNoteId) + '/' + encodeURIComponent(slug);
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
    updateBrowserUrl(currentNoteId, currentFilename);
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

        const notePublic = !!s.public;
        const folderIcon = document.createElement('span');
        folderIcon.className = 'icon' + (isAdmin ? ' clickable' : '') + (notePublic ? '' : ' private');
        if (s.icon) {
            folderIcon.textContent = s.icon;
        } else {
            folderIcon.textContent = notePublic ? ((s.id === currentNoteId) ? '📂' : '📁') : '🔐';
        }
        folderIcon.title = notePublic
            ? (isAdmin ? 'Public note (click to make private)' : 'Public note')
            : (isAdmin ? 'Private note (click to make public)' : 'Private note');
        if (isAdmin) {
            folderIcon.onclick = (e) => {
                e.stopPropagation();
                const nextPublic = !notePublic;
                s.public = nextPublic;
                renderNoteList();
                api('note/visibility', { note_id: s.id, public: nextPublic });
            };
        }

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

            const fileVisibility = s.fileVisibility || {};
            const filePublic = !!fileVisibility[fname];

            const fileIcon = document.createElement('span');
            fileIcon.className = 'icon' + (isAdmin ? ' clickable' : '') + (filePublic ? '' : ' private');
            fileIcon.textContent = filePublic ? (fname.endsWith('.md') ? '📝' : '📄') : '🔒';
            fileIcon.title = filePublic
                ? (isAdmin ? 'Public file (click to make private)' : 'Public file')
                : (isAdmin ? 'Private file (click to make public)' : 'Private file');
            if (isAdmin) {
                fileIcon.onclick = (e) => {
                    e.stopPropagation();
                    const nextPublic = !filePublic;
                    if (!s.fileVisibility) s.fileVisibility = {};
                    s.fileVisibility[fname] = nextPublic;
                    renderNoteList();
                    api('file/visibility', { note_id: s.id, filename: fname, public: nextPublic });
                };
            }

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
    updateBrowserUrl(currentNoteId, currentFilename);
}

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

const EMOJI_CATEGORIES = {
    'smileys': {
        icon: '😀',
        title: 'Smileys',
        list: [
            { char: '😀', tags: 'smiley smile happy grin face' },
            { char: '😃', tags: 'smiley smile happy grin face' },
            { char: '😄', tags: 'smiley smile happy grin face' },
            { char: '😁', tags: 'smiley smile happy grin face' },
            { char: '😆', tags: 'smiley smile happy grin face' },
            { char: '😅', tags: 'smiley smile happy grin sweat face' },
            { char: '🤣', tags: 'smiley laugh rofl face' },
            { char: '😂', tags: 'smiley laugh tear face' },
            { char: '🙂', tags: 'smiley smile face' },
            { char: '🙃', tags: 'smiley upside down face' },
            { char: '😉', tags: 'smiley wink face' },
            { char: '😊', tags: 'smiley smile blush face' },
            { char: '😇', tags: 'smiley angel halo face' },
            { char: '🥰', tags: 'smiley love hearts blush face' },
            { char: '😍', tags: 'smiley love hearts eyes face' },
            { char: '🤩', tags: 'smiley star eyes face' },
            { char: '😘', tags: 'smiley love kiss face' },
            { char: '😋', tags: 'smiley yum delicious face' },
            { char: '😛', tags: 'smiley tongue face' },
            { char: '😜', tags: 'smiley tongue wink face' },
            { char: '🤪', tags: 'smiley crazy tongue face' },
            { char: '🤑', tags: 'smiley money mouth face' },
            { char: '😎', tags: 'smiley cool sunglasses face' },
            { char: '🤓', tags: 'smiley nerd glasses face' },
            { char: '🧐', tags: 'smiley monocle face' },
            { char: '🤔', tags: 'smiley think face' },
            { char: '😐', tags: 'smiley neutral face' },
            { char: '😑', tags: 'smiley expressionless face' },
            { char: '😏', tags: 'smiley smirk face' },
            { char: '😒', tags: 'smiley unamused face' },
            { char: '😬', tags: 'smiley grimace face' },
            { char: '🤥', tags: 'smiley lie liar face' },
            { char: '😌', tags: 'smiley relieved face' },
            { char: '😔', tags: 'smiley pensive face' },
            { char: '😪', tags: 'smiley sleepy tear face' },
            { char: '😴', tags: 'smiley sleep face' },
            { char: '😷', tags: 'smiley mask sick face' },
            { char: '🤢', tags: 'smiley nauseous green face' },
            { char: '🤮', tags: 'smiley vomit face' },
            { char: '🥵', tags: 'smiley hot red sweat face' },
            { char: '🥶', tags: 'smiley cold blue ice face' },
            { char: '😵', tags: 'smiley dizzy face' },
            { char: '🤯', tags: 'smiley mind blown explode head face' },
            { char: '🥳', tags: 'smiley party celebrate face' },
            { char: '💀', tags: 'skull dead bones' },
            { char: '💩', tags: 'poop dump' },
            { char: '🔥', tags: 'fire hot lit burn' },
            { char: '✨', tags: 'sparkles gold shine magic' },
            { char: '🌟', tags: 'star gold glow' },
            { char: '⭐', tags: 'star gold' },
            { char: '❤️', tags: 'love heart red' },
            { char: '💖', tags: 'love heart sparkles' },
            { char: '💔', tags: 'heart broken' }
        ]
    },
    'animals': {
        icon: '🐶',
        title: 'Nature',
        list: [
            { char: '🐶', tags: 'dog puppy animal pet' },
            { char: '🐱', tags: 'cat kitty animal pet' },
            { char: '🐭', tags: 'mouse animal' },
            { char: '🐰', tags: 'rabbit bunny animal' },
            { char: '🦊', tags: 'fox animal' },
            { char: '🐻', tags: 'bear animal' },
            { char: '🐼', tags: 'panda animal' },
            { char: '🐨', tags: 'koala animal' },
            { char: '🐯', tags: 'tiger animal' },
            { char: '🦁', tags: 'lion animal' },
            { char: '🐮', tags: 'cow animal' },
            { char: '🐷', tags: 'pig animal' },
            { char: '🐸', tags: 'frog animal' },
            { char: '🐵', tags: 'monkey animal' },
            { char: '🐒', tags: 'monkey animal' },
            { char: '🐧', tags: 'penguin animal' },
            { char: '🐦', tags: 'bird animal' },
            { char: '🦆', tags: 'duck animal' },
            { char: '🦅', tags: 'eagle animal' },
            { char: '🦉', tags: 'owl animal' },
            { char: '🐺', tags: 'wolf animal' },
            { char: '🦄', tags: 'unicorn animal magic' },
            { char: '🐝', tags: 'bee insect' },
            { char: '🐛', tags: 'bug caterpillar insect' },
            { char: '🦋', tags: 'butterfly insect' },
            { char: '🕷️', tags: 'spider insect' },
            { char: '🐢', tags: 'turtle animal' },
            { char: '🐍', tags: 'snake animal' },
            { char: '🐙', tags: 'octopus ocean sea' },
            { char: '🐬', tags: 'dolphin ocean sea' },
            { char: '🐳', tags: 'whale ocean sea' },
            { char: '🦈', tags: 'shark ocean sea' },
            { char: '🌲', tags: 'tree forest pine green' },
            { char: '🌵', tags: 'cactus desert green' },
            { char: '🍀', tags: 'clover leaf green luck' },
            { char: '🍁', tags: 'maple leaf red fall' },
            { char: '🌸', tags: 'flower blossom pink spring' },
            { char: '🌹', tags: 'rose flower red love' },
            { char: '🌻', tags: 'sunflower flower yellow' }
        ]
    },
    'food': {
        icon: '🍔',
        title: 'Food & Drink',
        list: [
            { char: '🍏', tags: 'apple green fruit food' },
            { char: '🍎', tags: 'apple red fruit food' },
            { char: '🍊', tags: 'orange fruit food' },
            { char: '🍌', tags: 'banana fruit food' },
            { char: '🍉', tags: 'watermelon fruit food' },
            { char: '🍇', tags: 'grape fruit food' },
            { char: '🍓', tags: 'strawberry fruit food' },
            { char: '🍒', tags: 'cherry fruit food' },
            { char: '🍑', tags: 'peach fruit food' },
            { char: '🍍', tags: 'pineapple fruit food' },
            { char: '🥥', tags: 'coconut fruit food' },
            { char: '🥝', tags: 'kiwi fruit food' },
            { char: '🍅', tags: 'tomato vegetable food' },
            { char: '🍆', tags: 'eggplant aubergine vegetable food' },
            { char: '🥑', tags: 'avocado vegetable food' },
            { char: '🌽', tags: 'corn vegetable food' },
            { char: '🥕', tags: 'carrot vegetable food' },
            { char: '🥔', tags: 'potato vegetable food' },
            { char: '🥐', tags: 'croissant bread bakery food' },
            { char: '🍞', tags: 'bread toast bakery food' },
            { char: '🥓', tags: 'bacon meat food' },
            { char: '🥩', tags: 'steak meat food' },
            { char: '🍔', tags: 'hamburger burger fastfood food' },
            { char: '🍕', tags: 'pizza fastfood food' },
            { char: '🌭', tags: 'hotdog fastfood food' },
            { char: '🍟', tags: 'fries fastfood food' },
            { char: '🥚', tags: 'egg food' },
            { char: '🍿', tags: 'popcorn movie snack food' },
            { char: '🥟', tags: 'dumpling dimsum food' },
            { char: '🍣', tags: 'sushi japanese food' },
            { char: '🍷', tags: 'wine glass drink alcohol' },
            { char: '🍺', tags: 'beer mug drink alcohol' },
            { char: '🍻', tags: 'beers cheers drink alcohol' },
            { char: '☕', tags: 'coffee cafe mug drink hot' },
            { char: '🍵', tags: 'tea green cup drink hot' }
        ]
    },
    'activities': {
        icon: '⚽',
        title: 'Activities',
        list: [
            { char: '⚽', tags: 'soccer football ball sports' },
            { char: '🏀', tags: 'basketball ball sports' },
            { char: '🏈', tags: 'football ball sports' },
            { char: '⚾', tags: 'baseball ball sports' },
            { char: '🥎', tags: 'softball ball sports' },
            { char: '🎾', tags: 'tennis ball sports' },
            { char: '🏐', tags: 'volleyball ball sports' },
            { char: '🎱', tags: 'billiards pool ball sports' },
            { char: '🏓', tags: 'pingpong table tennis ball sports' },
            { char: '🏸', tags: 'badminton sports' },
            { char: '🏹', tags: 'archery bow arrow sports' },
            { char: '🎣', tags: 'fishing rod fish sports' },
            { char: '🎯', tags: 'dart target bullseye' },
            { char: '🪁', tags: 'kite fly' },
            { char: '🎮', tags: 'controller game video sports console' },
            { char: '🕹️', tags: 'joystick game video console' },
            { char: '🎰', tags: 'slot machine game casino' },
            { char: '🎲', tags: 'die dice game board' },
            { char: '🧩', tags: 'puzzle piece game' },
            { char: '🎨', tags: 'paint palette art design' },
            { char: '🎬', tags: 'clapperboard movie cinema' },
            { char: '🎤', tags: 'microphone singing music' },
            { char: '🎧', tags: 'headphones music audio' },
            { char: '🎹', tags: 'piano keyboard music instrument' },
            { char: '🥁', tags: 'drum music instrument' },
            { char: '🎸', tags: 'guitar music instrument' },
            { char: '🎻', tags: 'violin music instrument' }
        ]
    },
    'travel': {
        icon: '🚗',
        title: 'Travel',
        list: [
            { char: '🚗', tags: 'car red automobile travel transport' },
            { char: '🚕', tags: 'taxi cab yellow travel transport' },
            { char: '🚓', tags: 'police car travel transport' },
            { char: '🚒', tags: 'fire engine truck travel transport' },
            { char: '🚐', tags: 'van travel transport' },
            { char: '🚚', tags: 'truck transport' },
            { char: '🚜', tags: 'tractor transport farm' },
            { char: '🛵', tags: 'scooter transport' },
            { char: '🏍️', tags: 'motorcycle bike transport' },
            { char: '🚨', tags: 'police siren emergency light' },
            { char: '🚲', tags: 'bicycle bike transport sports' },
            { char: '⛽', tags: 'gas fuel station' },
            { char: '⚓', tags: 'anchor ship boat navy' },
            { char: '⛵', tags: 'sailboat ship boat travel' },
            { char: '🛶', tags: 'canoe kayak boat travel' },
            { char: '✈️', tags: 'airplane plane flight travel' },
            { char: '🚀', tags: 'rocket space launch work speed' },
            { char: '🚁', tags: 'helicopter travel flight' },
            { char: '🏕️', tags: 'camping outdoor travel' },
            { char: '🏠', tags: 'house home building' },
            { char: '🏡', tags: 'house garden home building' },
            { char: '🏢', tags: 'office business building' },
            { char: '🏥', tags: 'hospital medical building' },
            { char: '🏫', tags: 'school education building' },
            { char: '🏭', tags: 'factory building industry' },
            { char: '🏰', tags: 'castle fortress building' },
            { char: '⛩️', tags: 'shrine torii gate Japanese' },
            { char: '⛲', tags: 'fountain park' },
            { char: '🌌', tags: 'milkyway galaxy space night' },
            { char: '🌉', tags: 'bridge night' }
        ]
    },
    'objects': {
        icon: '💡',
        title: 'Objects',
        list: [
            { char: '⌚', tags: 'watch time clock' },
            { char: '📱', tags: 'phone mobile smartphone cell' },
            { char: '💻', tags: 'laptop computer tech' },
            { char: '⌨️', tags: 'keyboard tech computer' },
            { char: '🖥️', tags: 'monitor screen computer' },
            { char: '🖨️', tags: 'printer office paper' },
            { char: '📸', tags: 'camera photo picture' },
            { char: '🎥', tags: 'camera movie video' },
            { char: '☎️', tags: 'telephone phone landline' },
            { char: '📺', tags: 'tv television screen' },
            { char: '📻', tags: 'radio music audio' },
            { char: '🎙️', tags: 'microphone studio recording' },
            { char: '🧭', tags: 'compass direction travel' },
            { char: '⏰', tags: 'alarm clock time' },
            { char: '⏳', tags: 'hourglass time sand' },
            { char: '🔋', tags: 'battery power energy charge' },
            { char: '💡', tags: 'lightbulb idea light bulb' },
            { char: '🔦', tags: 'flashlight torch light' },
            { char: '🕯️', tags: 'candle light wax' },
            { char: '🧯', tags: 'extinguisher safety fire' },
            { char: '💵', tags: 'dollar cash money green' },
            { char: '🪙', tags: 'coin gold money cash' },
            { char: '💰', tags: 'money bag gold cash' },
            { char: '💳', tags: 'credit card money bank' },
            { char: '💎', tags: 'gem diamond jewel' },
            { char: '⚖️', tags: 'scales justice balance' },
            { char: '🔨', tags: 'hammer tool construction' },
            { char: '🔧', tags: 'wrench spanner tool fix' },
            { char: '🔩', tags: 'bolt nut screw tool hardware' },
            { char: '⚙️', tags: 'gear settings tool mechanics' },
            { char: '🔐', tags: 'lock key secure private' },
            { char: '🔒', tags: 'lock secure private' },
            { char: '🔓', tags: 'lock open insecure' },
            { char: '🔑', tags: 'key lock open' },
            { char: '📚', tags: 'books study read education' },
            { char: '📝', tags: 'pencil paper write memo note' },
            { char: '📌', tags: 'pushpin pin map post' },
            { char: '✉️', tags: 'envelope mail letter' },
            { char: '🔔', tags: 'bell notification alert' }
        ]
    },
    'flags': {
        icon: '🚩',
        title: 'Flags',
        list: [
            { char: '🏁', tags: 'flag checkered finish race' },
            { char: '🚩', tags: 'flag red post' },
            { char: '🎌', tags: 'flags crossed Japan festival' },
            { char: '🏴', tags: 'flag black' },
            { char: '🏳️', tags: 'flag white peace surrender' },
            { char: '🏳️‍🌈', tags: 'flag rainbow pride lgbt' },
            { char: '🏳️‍⚧️', tags: 'flag transgender pride lgbt' },
            { char: '🏴‍☠️', tags: 'flag pirate skull crossbones' }
        ]
    }
};

let emojiPickerInitialized = false;

function initEmojiPicker() {
    const tabsContainer = document.getElementById('emoji-picker-tabs');
    const categoriesContainer = document.getElementById('emoji-categories-container');
    if (!tabsContainer || !categoriesContainer) return;

    tabsContainer.innerHTML = '';
    categoriesContainer.innerHTML = '';

    Object.keys(EMOJI_CATEGORIES).forEach(key => {
        const category = EMOJI_CATEGORIES[key];
        
        // Render Tab button
        const tabBtn = document.createElement('button');
        tabBtn.type = 'button';
        tabBtn.className = 'emoji-tab-btn';
        tabBtn.textContent = category.icon;
        tabBtn.title = category.title;
        tabBtn.onclick = (e) => {
            e.stopPropagation();
            const targetHeader = document.getElementById(`category-sec-${key}`);
            if (targetHeader) {
                targetHeader.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }
        };
        tabsContainer.appendChild(tabBtn);

        // Render Category Section
        const section = document.createElement('div');
        section.className = 'emoji-category-section';
        section.id = `category-sec-${key}`;

        const title = document.createElement('div');
        title.className = 'emoji-category-title';
        title.textContent = category.title;
        section.appendChild(title);

        const grid = document.createElement('div');
        grid.className = 'emoji-grid';

        category.list.forEach(emoji => {
            const btn = document.createElement('button');
            btn.type = 'button';
            btn.className = 'emoji-btn';
            btn.textContent = emoji.char;
            btn.title = emoji.tags;
            btn.onclick = (e) => {
                e.stopPropagation();
                selectEmoji(emoji.char);
            };
            grid.appendChild(btn);
        });

        section.appendChild(grid);
        categoriesContainer.appendChild(section);
    });

    if (emojiPickerInitialized) return;
    emojiPickerInitialized = true;

    // Search Event Listener
    const searchInput = document.getElementById('emoji-search-input');
    if (searchInput) {
        searchInput.oninput = () => filterEmojis(searchInput.value.trim().toLowerCase());
    }

    // Reset Button Listener
    const clearBtn = document.getElementById('emoji-clear-btn');
    if (clearBtn) {
        clearBtn.onclick = (e) => {
            e.stopPropagation();
            selectEmoji(null);
        };
    }

    // Popover click behavior
    const trigger = document.getElementById('emoji-picker-trigger');
    const popover = document.getElementById('emoji-picker-popover');
    if (trigger && popover) {
        trigger.onclick = (e) => {
            e.stopPropagation();
            popover.classList.toggle('hidden');
            if (!popover.classList.contains('hidden') && searchInput) {
                searchInput.value = '';
                filterEmojis('');
                searchInput.focus();
            }
        };
    }

    // Click outside dismissal
    document.addEventListener('click', (e) => {
        if (popover && !popover.classList.contains('hidden')) {
            const wrapper = document.querySelector('.emoji-picker-wrapper');
            if (wrapper && !wrapper.contains(e.target)) {
                popover.classList.add('hidden');
            }
        }
    });
}

function selectEmoji(emoji) {
    selectedNoteIcon = emoji;
    const trigger = document.getElementById('emoji-picker-trigger');
    if (trigger) trigger.textContent = emoji || '📂';
    
    // Update active state class in popover grid
    document.querySelectorAll('.emoji-btn').forEach(btn => {
        if (btn.textContent === emoji) {
            btn.classList.add('active');
        } else {
            btn.classList.remove('active');
        }
    });

    const popover = document.getElementById('emoji-picker-popover');
    if (popover) popover.classList.add('hidden');
}

function filterEmojis(query) {
    const container = document.getElementById('emoji-categories-container');
    const tabs = document.getElementById('emoji-picker-tabs');
    if (!container) return;

    if (!query) {
        // Show tabs and categories normally
        if (tabs) tabs.style.display = 'flex';
        initEmojiPicker(); // Re-render standard category state
        document.querySelectorAll('.emoji-btn').forEach(btn => {
            if (btn.textContent === selectedNoteIcon) {
                btn.classList.add('active');
            } else {
                btn.classList.remove('active');
            }
        });
        return;
    }

    // Hide category tabs during search
    if (tabs) tabs.style.display = 'none';
    container.innerHTML = '';

    // Render flat grid of search results
    const resultsSection = document.createElement('div');
    resultsSection.className = 'emoji-category-section';

    const title = document.createElement('div');
    title.className = 'emoji-category-title';
    title.textContent = 'Search Results';
    resultsSection.appendChild(title);

    const grid = document.createElement('div');
    grid.className = 'emoji-grid';

    let count = 0;
    Object.keys(EMOJI_CATEGORIES).forEach(key => {
        const category = EMOJI_CATEGORIES[key];
        category.list.forEach(emoji => {
            if (emoji.tags.toLowerCase().includes(query)) {
                count++;
                const btn = document.createElement('button');
                btn.type = 'button';
                btn.className = 'emoji-btn';
                if (emoji.char === selectedNoteIcon) btn.classList.add('active');
                btn.textContent = emoji.char;
                btn.title = emoji.tags;
                btn.onclick = (e) => {
                    e.stopPropagation();
                    selectEmoji(emoji.char);
                };
                grid.appendChild(btn);
            }
        });
    });

    if (count === 0) {
        const noResults = document.createElement('div');
        noResults.style.fontSize = '13px';
        noResults.style.color = 'var(--text-secondary)';
        noResults.style.padding = '10px 0';
        noResults.textContent = 'No matching emojis';
        grid.appendChild(noResults);
    }

    resultsSection.appendChild(grid);
    container.appendChild(resultsSection);
}

// --- Note Settings Dialog ---

function showNoteSettings(sessionId) {
    const s = notes.find(x => x.id === sessionId);
    if (!s) return;
    settingsNoteId = sessionId;
    document.getElementById('note-settings-title').textContent = 'Note: ' + s.name;
    document.getElementById('note-settings-name').value = s.name;
    
    selectedNoteIcon = s.icon || null;
    const trigger = document.getElementById('emoji-picker-trigger');
    if (trigger) trigger.textContent = selectedNoteIcon || '📂';
    // Highlight currently selected button:
    document.querySelectorAll('.emoji-btn').forEach(btn => {
        if (selectedNoteIcon && btn.textContent === selectedNoteIcon) {
            btn.classList.add('active');
        } else {
            btn.classList.remove('active');
        }
    });
    const popover = document.getElementById('emoji-picker-popover');
    if (popover) popover.classList.add('hidden');

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
    if (s && name) {
        if (name !== s.name || selectedNoteIcon !== (s.icon || null)) {
            api('note/rename', { note_id: settingsNoteId, name, icon: selectedNoteIcon });
        }
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

// --- CodeMirror mode aliases and mermaid simple mode ---
// The markdown mode resolves fence language names via CodeMirror.findModeByName(),
// which searches CodeMirror.modeInfo. The CDN minified builds don't populate that
// array, so we push aliases here before initEditor() runs.
function registerCodeMirrorModes() {
    if (typeof CodeMirror === 'undefined') return;

    // Helper: push an entry only if the name isn't already registered
    const info = (CodeMirror.modeInfo = CodeMirror.modeInfo || []);
    function reg(name, mime, mode) {
        const lc = name.toLowerCase();
        if (!info.some(e => e.name.toLowerCase() === lc)) {
            info.push({ name, mime, mode: mode || name });
        }
    }

    // C / C++ / Java / C# / Kotlin  (all handled by clike)
    reg('C',          'text/x-csrc',       'clike');
    reg('C++',        'text/x-c++src',     'clike');
    reg('cpp',        'text/x-c++src',     'clike');
    reg('Java',       'text/x-java',       'clike');
    reg('C#',         'text/x-csharp',     'clike');
    reg('csharp',     'text/x-csharp',     'clike');
    reg('Kotlin',     'text/x-kotlin',     'clike');
    reg('kotlin',     'text/x-kotlin',     'clike');
    reg('Scala',      'text/x-scala',      'clike');
    reg('scala',      'text/x-scala',      'clike');
    // JSON / TypeScript / plain JS aliases
    reg('JSON',       'application/json',  'javascript');
    reg('json',       'application/json',  'javascript');
    reg('jsonc',      'application/json',  'javascript');
    reg('TypeScript', 'application/typescript', 'javascript');
    reg('typescript', 'application/typescript', 'javascript');
    reg('ts',         'application/typescript', 'javascript');
    reg('js',         'text/javascript',   'javascript');
    // Shell aliases
    reg('bash',       'text/x-sh',         'shell');
    reg('sh',         'text/x-sh',         'shell');
    reg('zsh',        'text/x-sh',         'shell');
    // HTML
    reg('html',       'text/html',         'htmlmixed');
    reg('htm',        'text/html',         'htmlmixed');
    // TOML
    reg('TOML',       'text/x-toml',       'toml');
    reg('toml',       'text/x-toml',       'toml');
    // Mermaid — simple mode for diagram DSL
    if (CodeMirror.defineSimpleMode) {
        CodeMirror.defineSimpleMode('mermaid', {
            start: [
                { regex: /%%.*$/,       token: 'comment' },
                { regex: /\b(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram|stateDiagram-v2|gantt|pie|gitGraph|erDiagram|journey|mindmap|timeline|quadrantChart|block-beta|requirementDiagram)\b/, token: 'keyword' },
                { regex: /\b(LR|RL|TB|TD|BT|note|loop|alt|opt|else|par|critical|break|rect|activate|deactivate|participant|actor|as|end|over|of|link|click|style|classDef|class)\b/, token: 'builtin' },
                { regex: /"(?:[^"\\]|\\.)*"/, token: 'string' },
                { regex: /\[[^\]]*\]/,        token: 'string' },
                { regex: /-->|-->>|->>|-->|->|--\|>|===|==>|-\.->/,  token: 'operator' },
                { regex: /#[a-fA-F0-9]{3,6}/, token: 'number' },
                { regex: /\d+(\.\d+)?/,        token: 'number' },
            ],
            meta: { lineComment: '%%' },
        });
        reg('mermaid', 'text/x-mermaid', 'mermaid');
    }
}

// --- Init ---
registerCodeMirrorModes();
initEditor();
initPreviewScrollSync();
initPanelResize();
initEmojiPicker();
// Restore edit/preview state
try {
    const se = localStorage.getItem('rune_show_edit');
    const sp = localStorage.getItem('rune_show_preview');
    if (se !== null) showEdit    = se === '1';
    if (sp !== null) showPreview = sp === '1';
} catch {}
try {
    const val = localStorage.getItem('rune_sync_scroll');
    if (val !== null) {
        syncScrollEnabled = val === '1';
    }
} catch {}
const btnSync = document.getElementById('btn-sync-scroll');
if (btnSync) {
    if (syncScrollEnabled) btnSync.classList.add('active');
    else btnSync.classList.remove('active');
}
applyPanelLayout();
// Session init: verify session via /api/me, then connect, or redirect to login
function getSessionId() {
    const ls = localStorage.getItem('rune_session_id');
    if (ls) return ls;
    const match = document.cookie.match(/(?:^|;\s*)rune_session_id=([^;]+)/);
    if (match) {
        try { localStorage.setItem('rune_session_id', match[1]); } catch {}
        return match[1];
    }
    return null;
}

(async function initSession() {
    const sessionId = getSessionId();
    if (!sessionId) {
        window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
        return;
    }
    try {
        const resp = await fetch('/api/me', { credentials: 'include' });
        const data = resp.ok ? await resp.json() : { ok: false };
        if (data.ok) {
            myNickname = data.login || '';
            isAdmin = data.role === 'admin';
            isGuest = data.role === 'guest';
            // If URL contains a specific note/file, use it as the initial target
            if (_pendingNoteId) {
                localStorage.setItem('rune_note', _pendingNoteId);
                if (_pendingFile) localStorage.setItem('rune_file', _pendingFile + '.md');
                _pendingNoteId = null;
                _pendingFile   = null;
            }
            connect();
        } else {
            localStorage.removeItem('rune_session_id');
            window.location.href = '/?next=' + encodeURIComponent(window.location.pathname);
        }
    } catch {
        // Network error — SSE will handle auth
        connect();
    }
})();





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
    if (editorInstance && !editorInstance.getValue() && specContent) {
        editorInstance.setValue(specContent);
    }

    updateMobileFilename();
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
        const notePublic = !!s.public;
        const noteIconStr = s.icon ? s.icon : (notePublic ? (s.id === currentNoteId ? '📂' : '📁') : '🔐');
        noteHeader.innerHTML = '<span class="mobile-note-icon">' + noteIconStr + '</span><span class="mobile-note-name">' + (s.name || s.id) + '</span>';
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
            const fileVisibility = s.fileVisibility || {};
            const filePublic = !!fileVisibility[fname];
            const fileIconStr = filePublic ? (fname.endsWith('.md') ? '📝' : '📄') : '🔒';
            fileRow.innerHTML = '<span class="mobile-file-icon">' + fileIconStr + '</span><span class="mobile-file-name">' + fname + '</span>';
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
