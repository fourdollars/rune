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

// 3. Replicate marked.js configuration from web/app.js
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

marked.use({ renderer, breaks: true, gfm: true });

// 4. Replicate assignLines logic from web/app.js
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
    assert.match(html, /<h1 data-line="0">Title<\/h1>/);
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

console.log("All unit tests passed successfully! 🎉");
