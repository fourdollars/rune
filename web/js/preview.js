import './state.js';
globalThis.renderPreview = function renderPreview() {
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
                            const error = document.createElement('pre');
                            error.style.color = 'var(--error)';
                            error.textContent = 'Mermaid error: ' + err.message;
                            el.replaceChildren(error);
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
            btn.type = 'button';
            btn.className = 'copy-btn';
            btn.appendChild(runeIcon('copy'));
            btn.title = 'Copy code';
            btn.setAttribute('aria-label', 'Copy code');
            btn.dataset.action = 'copy-code';
            pre.style.position = 'relative';
            pre.appendChild(btn);
        });
    } else {
        preview.textContent = specContent;
    }
}

globalThis.markdownFragment = function markdownFragment(source) {
    const template = document.createElement('template');
    template.innerHTML = marked.parse(source);
    return template.content;
};

globalThis.copyCodeBlock = function copyCodeBlock(button) {
    const pre = button.closest('pre');
    const raw = pre.dataset.raw !== undefined ? decodeHtml(pre.dataset.raw) : pre.querySelector('code')?.textContent ?? '';
    const done = () => {
        setRuneIcon(button, 'check');
        button.style.opacity = '1';
        setTimeout(() => { setRuneIcon(button, 'copy'); button.style.opacity = ''; }, 1500);
    };
    if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(raw).then(done).catch(() => fallbackCopy(raw, done));
    else fallbackCopy(raw, done);
};
