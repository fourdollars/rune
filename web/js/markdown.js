import './state.js';
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

    // --- Math extensions: intercept $$ and $ before marked mangles the content ---
    // Block math: $$...$$  (must be registered before inline to take priority)
    const blockMathExtension = {
        name: 'blockMath',
        level: 'block',
        start(src) { return src.indexOf('$$'); },
        tokenizer(src) {
            const match = src.match(/^\$\$([\s\S]+?)\$\$/);
            if (match) {
                return { type: 'blockMath', raw: match[0], text: match[1].trim() };
            }
        },
        renderer(token) {
            if (typeof katex !== 'undefined') {
                try {
                    return '<div class="math-block">' + katex.renderToString(token.text, { displayMode: true, throwOnError: false }) + '</div>';
                } catch (e) {
                    return '<div class="math-block math-error">' + escapeHtml(token.text) + '</div>';
                }
            }
            return '<div class="math-block">$$' + escapeHtml(token.text) + '$$</div>';
        }
    };

    // Inline math: $...$
    const inlineMathExtension = {
        name: 'inlineMath',
        level: 'inline',
        start(src) { return src.indexOf('$'); },
        tokenizer(src) {
            // Avoid matching $$ (already handled by block extension)
            const match = src.match(/^\$(?!\$)((?:[^$\\]|\\[\s\S])+?)\$/);
            if (match) {
                return { type: 'inlineMath', raw: match[0], text: match[1] };
            }
        },
        renderer(token) {
            if (typeof katex !== 'undefined') {
                try {
                    return '<span class="math-inline">' + katex.renderToString(token.text, { displayMode: false, throwOnError: false }) + '</span>';
                } catch (e) {
                    return '<span class="math-inline math-error">$' + escapeHtml(token.text) + '$</span>';
                }
            }
            return '<span class="math-inline">$' + escapeHtml(token.text) + '$</span>';
        }
    };

    marked.use({ renderer, hooks, breaks: true, gfm: true, extensions: [blockMathExtension, inlineMathExtension] });
}
