// Rune Public Preview — Playwright Tests
// Tests: scrollability, shared style.css, syntax highlight, KaTeX, dark/light theme
const { chromium } = require('/tmp/node_modules/playwright');
const { execSync } = require('child_process');

const BASE = 'http://localhost:9527';
const ADMIN_TOKEN = 'admin';

let pass = 0;
let fail = 0;

function ok(name) { console.log(`  ✓ ${name}`); pass++; }
function ko(name, detail) { console.error(`  ✗ ${name}: ${detail}`); fail++; }

async function withPage(browser, fn, opts = {}) {
  const ctx = await browser.newContext(opts);
  const page = await ctx.newPage();
  try { await fn(page); } finally { await ctx.close(); }
}

function apiPost(path, body) {
  const json = JSON.stringify(body).replace(/'/g, "'\\''");
  return execSync(
    `curl -sf -X POST '${BASE}${path}' -H 'Authorization: Bearer ${ADMIN_TOKEN}' -H 'Content-Type: application/json' -d '${json}'`,
    { encoding: 'utf8' }
  );
}

// ── Setup: create a public note + file via API ────────────────────────────

function setup() {
  // Create note
  try { apiPost('/api/note/create', { id: 'playwright-pub', name: 'Playwright Public Test' }); } catch(e) {}
  // Make note public
  apiPost('/api/note/visibility', { note_id: 'playwright-pub', public: true });

  const lines = Array.from({length: 60}, (_, i) =>
    `Line ${i + 1}: Lorem ipsum dolor sit amet, consectetur adipiscing elit.`
  ).join('\n\n');

  const content = `# Hello Public

This is a paragraph with **bold** and *italic* text.

## Code Block

\`\`\`javascript
const x = 42;
function greet(name) {
  return \`Hello, \${name}!\`;
}
\`\`\`

## Inline Code

Use \`console.log()\` to debug.

## Math

Inline: $E = mc^2$

Display:
$$\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}$$

## Long content for scroll test

${lines}
`;

  // Write content to temp file to avoid shell quoting issues
  const fs = require('fs');
  const payload = JSON.stringify({ note_id: 'playwright-pub', name: 'test.md', content });
  fs.writeFileSync('/tmp/rune_test_payload.json', payload);
  execSync(
    `curl -sf -X POST '${BASE}/api/file/create' -H 'Authorization: Bearer ${ADMIN_TOKEN}' -H 'Content-Type: application/json' -d @/tmp/rune_test_payload.json`,
    { encoding: 'utf8' }
  );

  // Make file public
  apiPost('/api/file/visibility', { note_id: 'playwright-pub', filename: 'test.md', public: true });

  console.log('  setup: note + file created and made public');
}

// ── Tests ─────────────────────────────────────────────────────────────────

async function testScrollable(browser) {
  await withPage(browser, async (page) => {
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const bodyOverflow = await page.evaluate(() => getComputedStyle(document.body).overflow);
    const bodyHeight = await page.evaluate(() => getComputedStyle(document.body).height);
    const scrollHeight = await page.evaluate(() => document.documentElement.scrollHeight);
    const clientHeight = await page.evaluate(() => document.documentElement.clientHeight);

    if (bodyOverflow === 'hidden') {
      ko('body is not scrollable', `overflow=${bodyOverflow}`);
    } else {
      ok(`body overflow is not hidden (overflow=${bodyOverflow})`);
    }

    if (scrollHeight > clientHeight) {
      ok(`page is taller than viewport (scrollHeight=${scrollHeight} > clientHeight=${clientHeight})`);
    } else {
      ko('page should be scrollable', `scrollHeight=${scrollHeight} clientHeight=${clientHeight}`);
    }

    await page.evaluate(() => window.scrollTo(0, 500));
    const scrollY = await page.evaluate(() => window.scrollY);
    if (scrollY > 0) {
      ok(`scroll works (scrollY=${scrollY})`);
    } else {
      ko('scroll did not move', `scrollY=${scrollY}`);
    }
  });
}

async function testSharedStyleCss(browser) {
  await withPage(browser, async (page) => {
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const stylesheets = await page.evaluate(() =>
      Array.from(document.styleSheets).map(s => s.href).filter(Boolean)
    );
    const hasStyleCss = stylesheets.some(s => s.includes('style.css'));
    if (hasStyleCss) {
      ok('style.css is loaded');
    } else {
      ko('style.css not loaded', `sheets: ${stylesheets.join(', ')}`);
    }

    const previewEl = await page.$('#preview');
    if (previewEl) {
      ok('#preview element exists');
    } else {
      ko('#preview element missing', '');
    }

    const h1Color = await page.evaluate(() => {
      const h1 = document.querySelector('#preview h1');
      return h1 ? getComputedStyle(h1).color : null;
    });
    if (h1Color && h1Color !== 'rgb(0, 0, 0)') {
      ok(`h1 has custom accent colour (${h1Color})`);
    } else {
      ko('h1 colour not applied', `color=${h1Color}`);
    }
  });
}

async function testSyntaxHighlight(browser) {
  await withPage(browser, async (page) => {
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const hljsCount = await page.evaluate(() =>
      document.querySelectorAll('#preview code.hljs').length
    );
    if (hljsCount > 0) {
      ok(`hljs rendered ${hljsCount} code block(s)`);
    } else {
      ko('no hljs code blocks found', '');
    }

    const codeBg = await page.evaluate(() => {
      const pre = document.querySelector('#preview pre.hljs-pre');
      return pre ? getComputedStyle(pre).backgroundColor : null;
    });
    if (codeBg && codeBg !== 'rgba(0, 0, 0, 0)' && codeBg !== 'rgb(255, 255, 255)') {
      ok(`code block has distinct background (${codeBg})`);
    } else {
      ko('code block background not set correctly', `bg=${codeBg}`);
    }
  });
}

async function testKaTeX(browser) {
  await withPage(browser, async (page) => {
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const katexCount = await page.evaluate(() =>
      document.querySelectorAll('#preview .katex').length
    );
    if (katexCount >= 2) {
      ok(`KaTeX rendered ${katexCount} math element(s) (inline + display)`);
    } else {
      ko('KaTeX not rendered', `found ${katexCount} .katex elements`);
    }
  });
}

async function testLightMode(browser) {
  await withPage(browser, async (page) => {
    await page.emulateMedia({ colorScheme: 'light' });
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const bodyBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    // Light bg-primary is #f6f8fa = rgb(246, 248, 250)
    if (bodyBg === 'rgb(246, 248, 250)') {
      ok(`light mode body bg correct (${bodyBg})`);
    } else {
      ko('light mode body bg wrong', `got ${bodyBg}`);
    }

    const codeBg = await page.evaluate(() => {
      const pre = document.querySelector('#preview pre.hljs-pre');
      return pre ? getComputedStyle(pre).backgroundColor : null;
    });
    // #e8ecf0 = rgb(232, 236, 240)
    if (codeBg === 'rgb(232, 236, 240)') {
      ok(`light mode code block bg correct (${codeBg})`);
    } else {
      ko('light mode code block bg wrong', `got ${codeBg}`);
    }

    // Scrollable in light mode too
    const overflow = await page.evaluate(() => getComputedStyle(document.body).overflow);
    if (overflow !== 'hidden') {
      ok(`light mode: body scrollable (overflow=${overflow})`);
    } else {
      ko('light mode: body still overflow hidden', '');
    }
  });
}

async function testDarkMode(browser) {
  await withPage(browser, async (page) => {
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.goto(`${BASE}/public/playwright-pub/test`, { waitUntil: 'networkidle' });
    await page.waitForSelector('#preview', { state: 'visible', timeout: 10000 });

    const bodyBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    // Dark bg-primary is #1e1e2e = rgb(30, 30, 46)
    if (bodyBg === 'rgb(30, 30, 46)') {
      ok(`dark mode body bg correct (${bodyBg})`);
    } else {
      ko('dark mode body bg wrong', `got ${bodyBg}`);
    }

    const overflow = await page.evaluate(() => getComputedStyle(document.body).overflow);
    if (overflow !== 'hidden') {
      ok(`dark mode: body scrollable (overflow=${overflow})`);
    } else {
      ko('dark mode: body still overflow hidden', '');
    }
  });
}

// ── Cleanup ───────────────────────────────────────────────────────────────

function cleanup() {
  try {
    apiPost('/api/note/delete', { note_id: 'playwright-pub' });
  } catch(e) {}
  console.log('  cleanup: done');
}

// ── Main ──────────────────────────────────────────────────────────────────

(async () => {
  console.log('\nRune Public Preview — Playwright Tests');
  console.log('======================================');

  console.log('\n[setup]');
  setup();

  const browser = await chromium.launch({ headless: true });

  try {
    console.log('\n[1] Scrollability');
    await testScrollable(browser);

    console.log('\n[2] Shared style.css');
    await testSharedStyleCss(browser);

    console.log('\n[3] Syntax highlight (hljs)');
    await testSyntaxHighlight(browser);

    console.log('\n[4] KaTeX math rendering');
    await testKaTeX(browser);

    console.log('\n[5] Light mode');
    await testLightMode(browser);

    console.log('\n[6] Dark mode');
    await testDarkMode(browser);

  } finally {
    await browser.close();
    console.log('\n[cleanup]');
    cleanup();
  }

  console.log(`\n══════════════════════════════════════`);
  console.log(`Results: ${pass} passed, ${fail} failed`);
  if (fail > 0) process.exit(1);
})();
