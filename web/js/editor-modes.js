import './state.js';
// --- CodeMirror mode aliases and mermaid simple mode ---
// The markdown mode resolves fence language names via CodeMirror.findModeByName(),
// which searches CodeMirror.modeInfo. The CDN minified builds don't populate that
// array, so we push aliases here before initEditor() runs.
globalThis.registerCodeMirrorModes = function registerCodeMirrorModes() {
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
