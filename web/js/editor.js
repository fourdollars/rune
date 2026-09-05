import './state.js';
// --- DOM refs ---
globalThis.preview = document.getElementById('preview');
globalThis.editorContainer = document.getElementById('editor-container');
globalThis.previewContainer = document.getElementById('preview-container');
globalThis.chatMessages = document.getElementById('chat-messages');
globalThis.chatInput = document.getElementById('chat-input');
globalThis.statusIndicator = document.getElementById('status-indicator');
// Initial state: disconnected (before SSE connects)
if (statusIndicator) statusIndicator.className = 'status disconnected';
const mobileStatusInit = document.getElementById('mobile-status');
if (mobileStatusInit) mobileStatusInit.className = 'status disconnected';
globalThis.btnEdit = document.getElementById('btn-edit');
globalThis.btnPreview = document.getElementById('btn-preview');
globalThis.btnChat = document.getElementById('btn-chat');

// --- Editor highlight: markdown + fenced code block sub-language ---
globalThis.highlightMarkdownEditor = function highlightMarkdownEditor(text) {
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
};

globalThis.escapeHtmlEditor = function escapeHtmlEditor(text) {
    return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}


globalThis.editorTheme = function editorTheme() {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'rune-dark' : 'rune-light';
};

globalThis.initEditor = function initEditor() {
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
            if (editorDirty && currentNoteId && currentFilename) {
                api('notes/' + encodeURIComponent(currentNoteId) + '/files/' + encodeURIComponent(currentFilename), { content: specContent }, 'PUT');
                editorDirty = false;
            }
        }, 300);
    });

    editorInstance.on('scroll', () => {
        if (typeof handleEditorScroll === 'function') handleEditorScroll();
    });
};

globalThis.setEditorValue = function setEditorValue(text) {
    specContent = text;
    if (editorInstance) {
        if (editorInstance.getValue() !== text) {
            const cursor = editorInstance.getCursor();
            editorInstance.setValue(text);
            editorInstance.setCursor(cursor);
        }
    }
}
