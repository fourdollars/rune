const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

// Test Suite: Markdown Line-Tagging Renderers
console.log("=== Markdown Line-Tagging Renderers Test Suite ===");

// 1. Load marked.min.js into a VM context
const markedCode = fs.readFileSync(path.join(__dirname, '../web/marked.min.js'), 'utf8');
const context = { console };
vm.runInNewContext(markedCode, context);
const marked = context.marked;

if (typeof marked === 'undefined') {
    console.error("Failed to load marked.js");
    process.exit(1);
}
console.log("✓ Loaded marked.js successfully");

// 2. Mock hljs and renderMathInElement (as in browser)
context.hljs = {
    getLanguage: (lang) => lang === 'javascript' ? {} : null,
    highlight: (text, { language }) => ({ value: `highlighted:${text}` }),
    highlightAuto: (text) => ({ value: `auto:${text}` })
};

// 3. Replicate marked.js configuration from web/js/markdown.js
const getLineAttr = (token) => {
    return (token && typeof token.startLine === 'number') ? ` data-line="${token.startLine}"` : '';
};

function createSlugger() {
    const occurrences = new Map();
    return function slugify(text) {
        const rawSlug = (text || '')
            .toLowerCase()
            .trim()
            .replace(/<[^>]*>/g, '')
            .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '')
            .replace(/\s/g, '-');
        const baseSlug = rawSlug || 'heading';
        const count = occurrences.get(baseSlug) || 0;
        occurrences.set(baseSlug, count + 1);
        if (count === 0) return baseSlug;
        return `${baseSlug}-${count}`;
    };
}

let slugify = createSlugger();
const renderer = new marked.Renderer();

renderer.heading = function(token) {
    const { depth, text, tokens } = token;
    const lineAttr = getLineAttr(token);
    const id = slugify(text);
    const idAttr = id ? ` id="${id}"` : '';
    const body = this.parser ? this.parser.parseInline(tokens) : text;
    return `<h${depth}${idAttr}${lineAttr}>${body}</h${depth}>\n`;
};

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
    if (typeof context.hljs !== 'undefined') {
        const language = lang && context.hljs.getLanguage(lang) ? lang : null;
        const highlighted = language
            ? context.hljs.highlight(text, { language }).value
            : context.hljs.highlightAuto(text).value;
        const langClass = language ? ` class="language-${language}"` : '';
        return `<pre class="hljs-pre"${lineAttr} data-raw="${raw}"><code class="hljs${langClass}">${highlighted}</code></pre>`;
    }
    const safe = text.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
    return `<pre class="hljs-pre"${lineAttr} data-raw="${raw}"><code>${safe}</code></pre>`;
};

const hooks = {
    preprocess(markdown) {
        slugify = createSlugger();
        return markdown;
    }
};

const tokenizer = {
    del(src) {
        const match = /^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/.exec(src);
        if (match) {
            return {
                type: 'del',
                raw: match[0],
                text: match[2],
                tokens: this.lexer.inlineTokens(match[2]),
            };
        }
    },
};

const escapeHtml = (str) => {
    return (str || '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
};

const blockMathExtension = {
    name: 'blockMath',
    level: 'block',
    start(src) { return src.indexOf('$$'); },
    tokenizer(src) {
        const match = src.match(/^\$\$([\s\S]+?)\$\$/);
        if (match) return { type: 'blockMath', raw: match[0], text: match[1].trim() };
    },
    renderer(token) {
        return '<div class="math-block">$$' + escapeHtml(token.text) + '$$</div>';
    }
};

const inlineMathExtension = {
    name: 'inlineMath',
    level: 'inline',
    start(src) { return src.indexOf('$'); },
    tokenizer(src) {
        const match = src.match(/^\$(?!\$)((?:[^$\\]|\\[\s\S])+?)\$/);
        if (match) return { type: 'inlineMath', raw: match[0], text: match[1] };
    },
    renderer(token) {
        return '<span class="math-inline">$' + escapeHtml(token.text) + '$</span>';
    }
};

marked.use({ renderer, tokenizer, hooks, breaks: true, gfm: true, extensions: [blockMathExtension, inlineMathExtension] });

// 4. Replicate assignLines logic from web/js/preview.js
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

function parse(markdown) {
    const tokens = marked.lexer(markdown);
    assignLines(tokens, 0);
    return marked.parser(tokens);
}

// --- Test Cases ---

// Test 1: Simple Paragraph
{
    const html = parse("Hello World");
    assert.match(html, /<p data-line="0">Hello World<\/p>/);
    console.log("✓ Test 1: Simple Paragraph passed");
}

// Test 2: Formatting Preservation (Bold & Italic)
{
    const html = parse("This is **bold** and *italic*.");
    assert.match(html, /<p data-line="0">This is <strong>bold<\/strong> and <em>italic<\/em>\.<\/p>/);
    console.log("✓ Test 2: Formatting Preservation passed");
}

// Test 3: Multiple Blocks with correct Line Indices
{
    const markdown = `# Title

First paragraph.

Second paragraph.`;
    const html = parse(markdown);
    assert.match(html, /<h1 id="title" data-line="0">Title<\/h1>/);
    assert.match(html, /<p data-line="2">First paragraph\.<\/p>/);
    assert.match(html, /<p data-line="4">Second paragraph\.<\/p>/);
    console.log("✓ Test 3: Multiple Blocks line numbers passed");
}

// Test 4: Nested Lists and List Items
{
    const markdown = `- Item 1
- Item 2
  - Subitem 2.1`;
    const html = parse(markdown);
    assert.match(html, /<ul data-line="0">/);
    assert.match(html, /<li data-line="0">Item 1<\/li>/);
    assert.match(html, /<li data-line="1">Item 2/);
    assert.match(html, /<ul data-line="2">/);
    assert.match(html, /<li data-line="2">Subitem 2.1<\/li>/);
    console.log("✓ Test 4: Nested Lists and List Items passed");
}

// Test 5: Blockquotes
{
    const markdown = `> Quote line 1
> Quote line 2`;
    const html = parse(markdown);
    assert.match(html, /<blockquote data-line="0">/);
    assert.match(html, /<p data-line="0">Quote line 1<br>Quote line 2<\/p>/);
    console.log("✓ Test 5: Blockquotes passed");
}

// Test 6: Code Blocks and Syntax Highlight
{
    const markdown = `\`\`\`javascript
const x = 42;
\`\`\``;
    const html = parse(markdown);
    assert.match(html, /<pre class="hljs-pre" data-line="0" data-raw="const x = 42;?">/);
    assert.match(html, /<code class="hljs class="language-javascript"">highlighted:const x = 42;?<\/code>/);
    console.log("✓ Test 6: Code Blocks passed");
}

// Test 7: Mermaid Code Blocks
{
    const markdown = `\`\`\`mermaid
graph TD;
    A-->B;
\`\`\``;
    const html = parse(markdown);
    assert.match(html, /<div class="mermaid-block" data-line="0" id="mermaid-[a-z0-9]+" data-src="graph TD;?\n?\s*A-->B;?"><\/div>/);
    console.log("✓ Test 7: Mermaid Code Blocks passed");
}

// Test 8: Strikethrough requires ~~ and preserves single-tilde ranges and approximations
{
    const markdown = "> The network latency is ~50ms (~100MB/s bandwidth). Operating temperature is 20~35C with step A1 ~ A3.\n\nHere is ~~valid strikethrough text~~.";
    const html = parse(markdown);
    assert.doesNotMatch(html, /<del>50ms/);
    assert.doesNotMatch(html, /<del>[^<]*20<\/del>/);
    assert.match(html, /~50ms \(~100MB\/s bandwidth\)/);
    assert.match(html, /20~35C/);
    assert.match(html, /A1 ~ A3/);
    assert.match(html, /<del>valid strikethrough text<\/del>/);
    console.log("✓ Test 8: Strikethrough requires ~~ and preserves single tilde passed");
}

// Test 9: Heading slugification and TOC anchor link targets (Recipe Guide)
{
    const markdown = `# 經典義式肉醬千層麵 (Classic Beef Lasagna Recipe)
## 1. Ingredients & Prep (食材與前置準備)
### 製作步驟
## 2. Cooking & Assembly (烹調與組裝)
### 製作步驟
`;
    const html = parse(markdown);
    assert.match(html, /<h1 id="經典義式肉醬千層麵-classic-beef-lasagna-recipe" data-line="0">經典義式肉醬千層麵 \(Classic Beef Lasagna Recipe\)<\/h1>/);
    assert.match(html, /<h2 id="1-ingredients--prep-食材與前置準備" data-line="1">1\. Ingredients &amp; Prep \(食材與前置準備\)<\/h2>/);
    assert.match(html, /<h3 id="製作步驟" data-line="2">製作步驟<\/h3>/);
    assert.match(html, /<h2 id="2-cooking--assembly-烹調與組裝" data-line="3">2\. Cooking &amp; Assembly \(烹調與組裝\)<\/h2>/);
    assert.match(html, /<h3 id="製作步驟-1" data-line="4">製作步驟<\/h3>/);
    console.log("✓ Test 9: Heading slugification and TOC anchor link targets (Recipe Guide) passed");
}

// Test 10: Inline and Block Math formulas including \sqrt{2}
{
    const markdown = "Here is an inline formula: $\\sqrt{2}$ and $E = mc^2$.\n\n$$\\sum_{i=1}^n i = \\frac{n(n+1)}{2}$$";
    const html = parse(markdown);
    assert.ok(html.includes('<span class="math-inline">$\\sqrt{2}$</span>'));
    assert.ok(html.includes('<span class="math-inline">$E = mc^2$</span>'));
    assert.ok(html.includes('<div class="math-block">$$\\sum_{i=1}^n i = \\frac{n(n+1)}{2}$$</div>'));
    console.log("✓ Test 10: Inline and Block Math formulas passed");
}

console.log("All unit tests passed successfully! 🎉");
