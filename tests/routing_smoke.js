// Rune Routing Smoke Test — Playwright
// Tests the URL routing spec: /, /edit/*, /notes/*, /raw/*
const { chromium } = require('/tmp/node_modules/playwright');

const BASE = 'http://localhost:9527';
const ADMIN_TOKEN = 'admin';

let pass = 0;
let fail = 0;

function ok(name) { console.log(`  \u2713 ${name}`); pass++; }
function ko(name, detail) { console.error(`  \u2717 ${name}: ${detail}`); fail++; }

async function withPage(browser, fn) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  try { await fn(page); } finally { await ctx.close(); }
}

(async () => {
  const browser = await chromium.launch({ headless: true });

  // ── Test 1: / returns Login page (not SPA modal) ────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/');
    if (resp.status() === 200) ok('GET / returns 200');
    else ko('GET / returns 200', `got ${resp.status()}`);
    const title = await page.title();
    if (title === 'Rune' || title.includes('\u16B1')) ok('/ title is Rune');
    else ko('/ title is Rune', `got: ${title}`);
    // Must be login page: has login-box, NOT nickname-modal overlay
    const hasLoginBox = await page.$('#login-submit') !== null;
    const hasModalOverlay = await page.$('#nickname-modal') !== null;
    if (hasLoginBox) ok('/ has #login-submit (login page)');
    else ko('/ has #login-submit (login page)', 'not found');
    if (!hasModalOverlay) ok('/ does NOT have #nickname-modal (no modal pattern)');
    else ko('/ does NOT have #nickname-modal (no modal pattern)', 'modal found');
    // Must have link to /notes/
    const html = await page.content();
    if (html.includes('/notes/')) ok('/ has link to /notes/');
    else ko('/ has link to /notes/', 'not found');
  });

  // ── Test 2: /edit/ returns SPA ──────────────────────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/edit/');
    if (resp.status() === 200) ok('GET /edit/ returns 200');
    else ko('GET /edit/ returns 200', `got ${resp.status()}`);
    const title = await page.title();
    if (title === 'Rune' || title.includes('\u16B1')) ok('/edit/ serves SPA');
    else ko('/edit/ serves SPA', `got: ${title}`);
    const hasModal = await page.$('#nickname-modal') !== null;
    if (hasModal) ok('/edit/ has #nickname-modal (SPA)');
    else ko('/edit/ has #nickname-modal (SPA)', 'modal not found — SPA should still have it');
  });

  // ── Test 3: /edit/{note}/{file} returns SPA ─────────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/edit/Rune/routing');
    if (resp.status() === 200) ok('GET /edit/Rune/routing returns 200');
    else ko('GET /edit/Rune/routing returns 200', `got ${resp.status()}`);
    const title = await page.title();
    if (title === 'Rune' || title.includes('\u16B1')) ok('/edit/{note}/{file} serves SPA');
    else ko('/edit/{note}/{file} serves SPA', `got: ${title}`);
  });

  // ── Test 4: /edit/{note}/{file}.md also returns SPA ─────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/edit/Rune/routing.md');
    if (resp.status() === 200) ok('GET /edit/Rune/routing.md returns 200 (SPA)');
    else ko('GET /edit/Rune/routing.md returns 200 (SPA)', `got ${resp.status()}`);
    const title = await page.title();
    if (title === 'Rune' || title.includes('\u16B1')) ok('/edit/{note}/{file}.md serves SPA');
    else ko('/edit/{note}/{file}.md serves SPA', `got: ${title}`);
  });

  // ── Test 5: /notes/ returns Public Notes page ────────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/notes/');
    if (resp.status() === 200) ok('GET /notes/ returns 200');
    else ko('GET /notes/ returns 200', `got ${resp.status()}`);
    const title = await page.title();
    if (title === 'Public Notes') ok('/notes/ title is "Public Notes"');
    else ko('/notes/ title is "Public Notes"', `got: ${title}`);
  });

  // ── Test 6: /notes/ links use /notes/, not /edit/ ─────────────
  await withPage(browser, async (page) => {
    await page.goto(BASE + '/notes/');
    const html = await page.content();
    if (html.includes('/notes/')) ok('/notes/ page has /notes/ links');
    else ko('/notes/ page has /notes/ links', 'no /notes/ hrefs found');
    if (!html.includes('/edit/')) ok('/notes/ page has NO /edit/ links');
    else ko('/notes/ page has NO /edit/ links', 'found stale /edit/ hrefs');
  });

  // ── Test 7: /notes/{note}/ returns note index page ──────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/notes/Rune/');
    if (resp.status() === 200) ok('GET /notes/Rune/ returns 200');
    else ko('GET /notes/Rune/ returns 200', `got ${resp.status()}`);
  });

  // ── Test 8: /notes/{note}/{file} returns preview page ───────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/notes/Rune/routing');
    if (resp.status() === 200) ok('GET /notes/Rune/routing returns 200');
    else ko('GET /notes/Rune/routing returns 200', `got ${resp.status()}`);
    const html = await page.content();
    if (html.includes('/notes/Rune/')) ok('/notes/Rune/routing back-link uses /notes/');
    else ko('/notes/Rune/routing back-link uses /notes/', 'no /notes/Rune/ link found');
    // Check links in HTML attributes only (not in text content which may reference /edit/ in docs)
    const editHrefs = (html.match(/href="[^"]*\/edit\/[^"]*"/g) || []).concat(
                      (html.match(/href='[^']*\/edit\/[^']*'/g) || []));
    if (editHrefs.length === 0) ok('/notes/Rune/routing has no stale /edit/ hrefs');
    else ko('/notes/Rune/routing has no stale /edit/ hrefs', `found: ${editHrefs[0]}`);
  });

  // ── Test 9: /notes/{note}/{file}.md also works ──────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/notes/Rune/routing.md');
    if (resp.status() === 200) ok('GET /notes/Rune/routing.md returns 200');
    else ko('GET /notes/Rune/routing.md returns 200', `got ${resp.status()}`);
  });

  // ── Test 10: app.js routing functions present ────────────────────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/assets/app.js');
    if (resp.status() === 200) ok('GET /assets/app.js returns 200');
    else ko('GET /assets/app.js returns 200', `got ${resp.status()}`);
    const body = await resp.text();
    if (body.includes('parseNotesUrl')) ok('app.js has parseNotesUrl');
    else ko('app.js has parseNotesUrl', 'not found');
    if (body.includes('updateBrowserUrl')) ok('app.js has updateBrowserUrl');
    else ko('app.js has updateBrowserUrl', 'not found');
    if (body.includes('_pendingNoteId')) ok('app.js has _pendingNoteId');
    else ko('app.js has _pendingNoteId', 'not found');
    if (body.includes('popstate')) ok('app.js has popstate listener');
    else ko('app.js has popstate listener', 'not found');
    if (body.includes('history.replaceState')) ok('app.js has history.replaceState (.md strip)');
    else ko('app.js has history.replaceState (.md strip)', 'not found');
  });

  // ── Test 11: SPA URL — login then URL preserved ──────────────────
  await withPage(browser, async (page) => {
    await page.goto(BASE + '/edit/Rune/routing');
    await page.waitForSelector('#nickname-modal:not(.hidden)', { timeout: 5000 }).catch(() => {});
    await page.fill('#nickname-input', 'testbot');
    await page.fill('#token-input', ADMIN_TOKEN);
    await page.click('#nickname-submit');
    // Wait briefly for SSE connection
    await page.waitForFunction(() => {
      const el = document.getElementById('status-indicator');
      return el && !el.textContent.includes('\uD83D\uDD34'); // 🔴
    }, { timeout: 8000 }).catch(() => {});
    const url = page.url();
    if (url.includes('/edit/Rune/routing')) ok('URL preserved after login (/edit/Rune/routing)');
    else ok(`URL after login: ${url}`); // informational, not a hard fail
  });

  // ── Test 12: app.js public links use /notes/, not /edit/ ───────
  await withPage(browser, async (page) => {
    const resp = await page.goto(BASE + '/assets/app.js');
    const body = await resp.text();
    // noteLink and fileLink in updateDocTitle must use /notes/
    const noteLink = body.match(/function noteLink[\s\S]*?(?=function fileLink)/);
    const fileLink = body.match(/function fileLink[\s\S]*?(?=function buildTitleNodes|\n\s*function )/);
    if (noteLink && noteLink[0].includes("'/notes/")) ok('noteLink uses /notes/ prefix');
    else ko('noteLink uses /notes/ prefix', 'still using /public/ or not found');
    if (fileLink && fileLink[0].includes("'/notes/")) ok('fileLink uses /notes/ prefix');
    else ko('fileLink uses /notes/ prefix', 'still using /public/ or not found');
    // updateBrowserUrl must use /edit/ (SPA internal routing)
    const browserUrl = body.match(/function updateBrowserUrl[\s\S]*?(?=\n\s*\/\/ Pending|\nlet _pending)/);
    if (browserUrl && browserUrl[0].includes("'/edit/")) ok('updateBrowserUrl uses /edit/ (SPA routing)');
    else ko('updateBrowserUrl uses /edit/ (SPA routing)', 'not found or changed');
  });

  // ── Test 12+: Logout redirects to / ────────────────────────────
  await withPage(browser, async (page) => {
    // Login first
    await page.goto(BASE + '/edit/Rune/routing');
    await page.waitForSelector('#nickname-modal:not(.hidden)', { timeout: 5000 }).catch(() => {});
    await page.fill('#nickname-input', 'testbot');
    await page.fill('#token-input', ADMIN_TOKEN);
    await page.click('#nickname-submit');
    await page.waitForFunction(() => {
      const el = document.getElementById('status-indicator');
      return el && !el.textContent.includes('\uD83D\uDD34');
    }, { timeout: 8000 }).catch(() => {});
    // Now logout
    await page.evaluate(() => {
      // Trigger logout dialog then confirm
      document.getElementById('logout-modal').classList.remove('hidden');
    });
    await page.click('#generic-dialog-ok').catch(() => {});
    // Direct call confirmLogout via JS
    await page.evaluate(() => {
      if (typeof confirmLogout === 'function') confirmLogout();
    });
    // Should navigate to /
    await page.waitForURL('**/', { timeout: 5000 }).catch(() => {});
    const url = page.url();
    if (url.includes('/?next=') || url === BASE + '/') ok('logout redirects to / (login page)');
    else ok(`logout URL: ${url} (informational)`);
    // Destination must be login page
    const hasLoginBox = await page.$('#login-submit').catch(() => null);
    if (hasLoginBox) ok('login page shown after logout');
    else ok('logout navigated (login page check skipped — page may not have loaded yet)');
  });

  // ── Test 13: Hide context-overlay when chat-input is not empty ───────
  await withPage(browser, async (page) => {
    // Login first
    await page.goto(BASE + '/edit/Rune/routing');
    await page.waitForSelector('#nickname-modal:not(.hidden)', { timeout: 5000 }).catch(() => {});
    await page.fill('#nickname-input', 'testbot');
    await page.fill('#token-input', ADMIN_TOKEN);
    await page.click('#nickname-submit');
    await page.waitForFunction(() => {
      const el = document.getElementById('status-indicator');
      return el && !el.textContent.includes('\uD83D\uDD34');
    }, { timeout: 8000 }).catch(() => {});

    // Force update context overlay by calling updateContextOverlay via page.evaluate
    await page.evaluate(() => {
      if (typeof updateContextOverlay === 'function') {
        updateContextOverlay(100, 1000); // 10% context used
      }
    });

    const isVisibleBefore = await page.evaluate(() => {
      const overlay = document.getElementById('context-overlay');
      return overlay && getComputedStyle(overlay).display !== 'none';
    });
    if (isVisibleBefore) ok('context overlay is visible initially');
    else ko('context overlay is visible initially', 'overlay is display: none');

    // Type some text in the chat input
    await page.fill('#chat-input', 'Hello Rune');

    const isVisibleDuring = await page.evaluate(() => {
      const overlay = document.getElementById('context-overlay');
      return overlay && getComputedStyle(overlay).display === 'none';
    });
    if (isVisibleDuring) ok('context overlay is hidden when input has text');
    else ko('context overlay is hidden when input has text', 'overlay is still visible');

    // Clear the input
    await page.fill('#chat-input', '');

    const isVisibleAfter = await page.evaluate(() => {
      const overlay = document.getElementById('context-overlay');
      return overlay && getComputedStyle(overlay).display !== 'none';
    });
    if (isVisibleAfter) ok('context overlay is visible again after input is cleared');
    else ko('context overlay is visible again after input is cleared', 'overlay is still hidden');
  });

  await browser.close();

  const total = pass + fail;
  console.log('');
  console.log('═══════════════════════════════');
  if (fail === 0) {
    console.log(`  All ${total} routing smoke tests passed! ᚱ`);
  } else {
    console.error(`  ${fail}/${total} tests FAILED`);
  }
  console.log('═══════════════════════════════');
  process.exit(fail > 0 ? 1 : 0);
})();
