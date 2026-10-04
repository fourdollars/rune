const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert');

console.log('=== Comprehensive Vendor Assets Integration Test ===');

class Element {
  constructor(tag) {
    this.tagName = tag;
    this.style = {};
    this.attrs = {};
    this.children = [];
  }
  setAttribute(k, v) { this.attrs[k] = v; }
  getAttribute(k) { return this.attrs[k]; }
  removeAttribute(k) { delete this.attrs[k]; }
  appendChild(c) { this.children.push(c); return c; }
  append(c) { this.children.push(c); return c; }
  remove() {}
}

const sandbox = {
  console,
  setTimeout,
  clearTimeout,
  addEventListener: () => {},
  removeEventListener: () => {},
  window: null,
  document: {
    compatMode: 'CSS1Compat',
    createElement: (tag) => new Element(tag),
    createElementNS: (ns, tag) => new Element(tag),
    getElementById: () => null,
    querySelectorAll: () => [],
    querySelector: () => null,
    body: new Element('body')
  },
  navigator: { userAgent: 'Node' }
};
sandbox.window = sandbox;
sandbox.globalThis = sandbox;
const ctx = vm.createContext(sandbox);

// 1. CodeMirror
const cmCode = fs.readFileSync(path.join(__dirname, '../web/codemirror.min.js'), 'utf8');
const cmModesCode = fs.readFileSync(path.join(__dirname, '../web/codemirror-modes.min.js'), 'utf8');
const cmMarkdownCode = fs.readFileSync(path.join(__dirname, '../web/codemirror-markdown.min.js'), 'utf8');

vm.runInContext(cmCode, ctx);
assert.strictEqual(typeof ctx.CodeMirror, 'function', 'CodeMirror constructor should be defined');
console.log('✓ CodeMirror loaded, version:', ctx.CodeMirror.version);
assert(ctx.CodeMirror.version.startsWith('5.65.'), 'Version should be 5.65.x');

vm.runInContext(cmModesCode, ctx);
assert.strictEqual(typeof ctx.CodeMirror.defineSimpleMode, 'function', 'defineSimpleMode addon should exist');
assert(ctx.CodeMirror.modes.toml, 'toml mode should be defined');
assert(ctx.CodeMirror.modes.htmlmixed, 'htmlmixed mode should be defined');
assert(ctx.CodeMirror.modes.javascript, 'javascript mode should be defined');
assert(ctx.CodeMirror.modes.python, 'python mode should be defined');
console.log('✓ CodeMirror modes loaded (toml, htmlmixed, javascript, python, etc.)');

vm.runInContext(cmMarkdownCode, ctx);
assert(ctx.CodeMirror.modes.markdown, 'markdown mode should be defined');
console.log('✓ CodeMirror markdown mode loaded');

// 2. Highlight.js
const hljsCode = fs.readFileSync(path.join(__dirname, '../web/highlight.min.js'), 'utf8');
vm.runInContext(hljsCode, ctx);
assert.strictEqual(typeof ctx.hljs, 'object', 'hljs should be defined');
const hlRes = ctx.hljs.highlight('const x = 42;', { language: 'javascript' });
assert(hlRes.value.includes('hljs-number'), 'hljs should highlight numbers in JS');
console.log('✓ Highlight.js loaded and highlighted sample code');

// 3. KaTeX
const katexCode = fs.readFileSync(path.join(__dirname, '../web/katex.min.js'), 'utf8');
vm.runInContext(katexCode, ctx);
assert.strictEqual(typeof ctx.katex, 'object', 'katex should be defined');
const mathHtml = ctx.katex.renderToString('E = mc^2');
assert(mathHtml.includes('katex-html'), 'katex should render HTML output');
console.log('✓ KaTeX loaded and rendered formula');

// 4. KaTeX auto-render
const autoRenderCode = fs.readFileSync(path.join(__dirname, '../web/katex-auto-render.min.js'), 'utf8');
vm.runInContext(autoRenderCode, ctx);
assert.strictEqual(typeof ctx.renderMathInElement, 'function', 'renderMathInElement should be defined');
console.log('✓ KaTeX auto-render loaded');

// 5. Marked
const markedCode = fs.readFileSync(path.join(__dirname, '../web/marked.min.js'), 'utf8');
vm.runInContext(markedCode, ctx);
assert.strictEqual(typeof ctx.marked, 'object', 'marked should be defined');
const parsed = ctx.marked.parse('# Hello World\n\nThis is **bold**.');
assert(parsed.includes('Hello World') && parsed.includes('<strong>bold</strong>'), 'marked should parse markdown correctly');
console.log('✓ Marked loaded and parsed markdown');

// 6. Mermaid
const mermaidCode = fs.readFileSync(path.join(__dirname, '../web/mermaid.min.js'), 'utf8');
vm.runInContext(mermaidCode, ctx);
assert.strictEqual(typeof ctx.mermaid, 'object', 'mermaid should be defined');
assert.strictEqual(typeof ctx.mermaid.render, 'function', 'mermaid.render should be a function');
ctx.mermaid.initialize({ startOnLoad: false });
console.log('✓ Mermaid loaded and initialized');

console.log('\n🎉 ALL EMBEDDED ASSETS PASSED INTEGRATION VERIFICATION!');
