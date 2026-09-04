// Rune Notes — Web UI smoke / regression suite (Playwright)
//
// Two suites:
//   regression — user-visible behaviour that must stay green before AND after
//                the UI refactor. Asserts on behaviour, never on inline
//                onclick= attributes or global function names.
//   goals      — target state for the UI overhaul. Expected to FAIL before the
//                refactor and PASS after it.
//
// Usage:
//   node tests/ui_smoke.js                     # both suites
//   node tests/ui_smoke.js --suite=regression
//   node tests/ui_smoke.js --suite=goals
//
// Env:
//   RUNE_UI_BASE  base URL      (default http://127.0.0.1:9599)
//   RUNE_UI_USER  username      (default admin)
//   RUNE_UI_PASS  password      (default admin123)

const { chromium } = require('/tmp/node_modules/playwright');

const BASE = process.env.RUNE_UI_BASE || 'http://127.0.0.1:9599';
const USER = process.env.RUNE_UI_USER || 'admin';
const PASS = process.env.RUNE_UI_PASS || 'admin123';

const args = process.argv.slice(2);
const suiteArg = (args.find((a) => a.startsWith('--suite=')) || '').split('=')[1] || 'all';

let pass = 0;
let fail = 0;
const failures = [];

function ok(name) {
  console.log(`  \u2713 ${name}`);
  pass++;
}
function ko(name, detail) {
  console.error(`  \u2717 ${name}: ${detail}`);
  fail++;
  failures.push(`${name}: ${detail}`);
}
async function check(name, fn) {
  try {
    const res = await fn();
    if (res === true || res === undefined) ok(name);
    else ko(name, String(res));
  } catch (err) {
    ko(name, err.message);
  }
}
function section(title) {
  console.log(`\n\u2500\u2500 ${title} \u2500\u2500`);
}

/** Log in and land on the editor SPA with the note tree populated. */
async function login(page) {
  await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' });
  await page.fill('#username', USER);
  await page.fill('#password', PASS);
  await Promise.all([
    page.waitForURL(/\/edit\//, { timeout: 15000 }),
    page.click('#local-login-form button[type="submit"]'),
  ]);
  // `attached`, not `visible`: the left panel may start collapsed.
  await page.waitForSelector('#note-tree .explorer-row', { state: 'attached', timeout: 15000 });
  await page.waitForTimeout(800);
}

/** Force the left panel open regardless of the persisted collapse state. */
async function openLeftPanel(page) {
  const collapsed = await page.evaluate(
    () => document.getElementById('panel-left')?.classList.contains('collapsed') ?? false,
  );
  if (collapsed) {
    await page.click('#resize-left');
    await page.waitForTimeout(300);
  }
}

/** Visible = non-zero box AND not clipped outside its scroll container. */
async function isReachable(page, selector) {
  return page.evaluate((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return false;
    return r.left >= 0 && r.right <= window.innerWidth + 1;
  }, selector);
}

// ───────────────────────────── regression ─────────────────────────────

async function runRegression(browser) {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await ctx.newPage();
  const consoleErrors = [];
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => consoleErrors.push(`pageerror: ${e.message}`));

  section('regression: auth');
  await check('login page renders sign-in form', async () => {
    await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' });
    const hasUser = (await page.$('#username')) !== null;
    const hasPass = (await page.$('#password')) !== null;
    return (hasUser && hasPass) || 'username/password inputs missing';
  });
  await check('login redirects to /edit/', async () => {
    await login(page);
    return page.url().includes('/edit/') || `url was ${page.url()}`;
  });
  await check('/api/me reports admin role', async () => {
    const me = await page.evaluate(async () => (await fetch('/api/me')).json());
    return me.role === 'admin' || `role was ${me.role}`;
  });

  section('regression: note tree');
  await openLeftPanel(page);
  await check('tree lists every note directory', async () => {
    const names = await page.$$eval('#note-tree .label', (els) => els.map((e) => e.textContent));
    const missing = ['AI', 'Demo', 'Rune'].filter((n) => !names.includes(n));
    return missing.length === 0 || `missing notes: ${missing.join(',')}`;
  });
  await check('tree lists markdown files', async () => {
    const names = await page.$$eval('#note-tree .label', (els) => els.map((e) => e.textContent));
    const md = names.filter((n) => n.endsWith('.md'));
    return md.length >= 13 || `only ${md.length} .md entries`;
  });
  await check('file search filters the tree', async () => {
    await page.fill('#file-search-input', 'Hello');
    await page.waitForTimeout(400);
    const visible = await page.$$eval('#note-tree .label', (els) =>
      els.filter((e) => e.offsetParent !== null).map((e) => e.textContent),
    );
    const hit = visible.some((n) => n.includes('Hello'));
    const noise = visible.filter((n) => n.endsWith('.md') && !n.includes('Hello'));
    await page.fill('#file-search-input', '');
    await page.waitForTimeout(400);
    if (!hit) return 'Hello.md not shown';
    return noise.length === 0 || `unfiltered leftovers: ${noise.join(',')}`;
  });

  section('regression: file switching');
  await check('clicking a file loads it into the editor', async () => {
    await page.getByText('Hello.md', { exact: true }).first().click();
    await page.waitForTimeout(1200);
    const title = await page.textContent('#split-title');
    return title.includes('Hello') || `split-title was "${title}"`;
  });
  await check('file switch updates the browser URL', async () => {
    return page.url().includes('Hello') || `url was ${page.url()}`;
  });
  await check('editor holds the loaded document text', async () => {
    const len = await page.evaluate(
      () => document.querySelector('.CodeMirror')?.CodeMirror?.getValue().length ?? 0,
    );
    return len > 0 || 'CodeMirror value empty';
  });
  await check('preview renders the loaded document', async () => {
    const html = await page.innerHTML('#preview');
    return html.trim().length > 0 || 'preview empty';
  });

  section('regression: editor / preview toggles');
  await check('preview toggle hides and restores the preview pane', async () => {
    await page.click('#btn-preview');
    await page.waitForTimeout(400);
    const hidden = await page.evaluate(
      () => document.getElementById('preview-container')?.classList.contains('hidden'),
    );
    await page.click('#btn-preview');
    await page.waitForTimeout(400);
    const shown = await page.evaluate(
      () => !document.getElementById('preview-container')?.classList.contains('hidden'),
    );
    return (hidden && shown) || `hidden=${hidden} restored=${shown}`;
  });
  await check('editor toggle shows and restores the editor pane', async () => {
    await page.click('#btn-edit');
    await page.waitForTimeout(400);
    const shown = await page.evaluate(() => {
      const el = document.getElementById('editor-container');
      return !!el && !el.classList.contains('hidden') && el.getBoundingClientRect().width > 0;
    });
    await page.click('#btn-edit');
    await page.waitForTimeout(400);
    const hidden = await page.evaluate(() => {
      const el = document.getElementById('editor-container');
      return !el || el.classList.contains('hidden') || el.getBoundingClientRect().width === 0;
    });
    await page.click('#btn-edit');
    await page.waitForTimeout(400);
    return (shown && hidden) || `shown=${shown} hidden=${hidden}`;
  });
  await check('sync-scroll toggle flips its active state', async () => {
    const before = await page.evaluate(
      () => document.getElementById('btn-sync-scroll')?.classList.contains('active'),
    );
    await page.click('#btn-sync-scroll');
    await page.waitForTimeout(250);
    const after = await page.evaluate(
      () => document.getElementById('btn-sync-scroll')?.classList.contains('active'),
    );
    await page.click('#btn-sync-scroll');
    await page.waitForTimeout(250);
    return before !== after || 'active class did not change';
  });

  section('regression: markdown toolbar');
  await check('bold action wraps the selection in **', async () => {
    await page.evaluate(() => {
      const cm = document.querySelector('.CodeMirror').CodeMirror;
      cm.setValue('smoketest');
      cm.setSelection({ line: 0, ch: 0 }, { line: 0, ch: 9 });
      cm.focus();
    });
    await page.getByRole('button', { name: 'B', exact: true }).first().click();
    await page.waitForTimeout(300);
    const val = await page.evaluate(() => document.querySelector('.CodeMirror').CodeMirror.getValue());
    return val.includes('**smoketest**') || `value was "${val}"`;
  });
  await check('every toolbar action is bound', async () => {
    const actions = ['bold', 'italic', 'header', 'link', 'image', 'code', 'ul', 'ol', 'task', 'table'];
    const results = await page.evaluate((acts) => {
      const cm = document.querySelector('.CodeMirror').CodeMirror;
      const broken = [];
      for (const a of acts) {
        cm.setValue('x');
        cm.setSelection({ line: 0, ch: 0 }, { line: 0, ch: 1 });
        const before = cm.getValue();
        try {
          window.insertFormat ? window.insertFormat(a) : null;
        } catch (e) {
          broken.push(`${a}:threw`);
          continue;
        }
        if (cm.getValue() === before) broken.push(`${a}:noop`);
      }
      return broken;
    }, actions);
    return results.length === 0 || `broken: ${results.join(',')}`;
  });

  section('regression: dialogs');
  const dialogs = [
    ['model', '#model-modal', () => page.click('#model-name')],
    ['search', '#search-modal', () => page.click('#btn-search')],
    ['archive', '#archive-modal', () => page.click('#btn-archive')],
    ['new note', '#new-note-modal', () => page.click('#btn-new-note')],
    ['logout', '#logout-modal', () => page.click('#btn-logout')],
  ];
  for (const [label, sel, open] of dialogs) {
    await check(`${label} dialog opens and closes`, async () => {
      await open();
      await page.waitForTimeout(500);
      const opened = await page.evaluate(
        (s) => !document.querySelector(s)?.classList.contains('hidden'),
        sel,
      );
      if (!opened) return 'did not open';
      await page.click(`${sel} .btn-secondary`);
      await page.waitForTimeout(400);
      const closed = await page.evaluate(
        (s) => document.querySelector(s)?.classList.contains('hidden'),
        sel,
      );
      return closed || 'did not close';
    });
  }
  await check('model dialog lists models and filters by search', async () => {
    await page.click('#model-name');
    await page.waitForTimeout(600);
    const total = await page.$$eval('#model-list > *', (e) => e.length);
    await page.fill('#model-search-input', 'sonnet');
    await page.waitForTimeout(400);
    const filtered = await page.$$eval('#model-list > *', (els) =>
      els.filter((e) => e.offsetParent !== null).length,
    );
    await page.click('#model-modal .btn-secondary');
    await page.waitForTimeout(300);
    if (total < 2) return `model list had ${total} entries`;
    return filtered < total || `filter did not narrow (${filtered}/${total})`;
  });

  section('regression: chat panel');
  await check('chat input and send control exist', async () => {
    const input = (await page.$('#chat-input')) !== null;
    const send = (await page.$('#chat-send')) !== null;
    return (input && send) || `input=${input} send=${send}`;
  });
  await check('status indicator is present', async () => {
    return (await page.$('#status-indicator')) !== null || '#status-indicator missing';
  });

  section('regression: panel persistence');
  await check('left panel collapse state persists across reload', async () => {
    await openLeftPanel(page);
    await page.click('#resize-left');
    await page.waitForTimeout(400);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(1500);
    const collapsed = await page.evaluate(
      () => document.getElementById('panel-left')?.classList.contains('collapsed'),
    );
    await openLeftPanel(page);
    return collapsed || 'collapse state not restored';
  });

  section('regression: console hygiene');
  await check('no console errors during the run', async () => {
    return consoleErrors.length === 0 || consoleErrors.slice(0, 3).join(' | ');
  });

  await ctx.close();
}

// ─────────────────────────────── goals ───────────────────────────────

async function runGoals(browser) {
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await ctx.newPage();
  await login(page);

  section('goal: discoverability');
  await check('file tree is visible on first load (no manual expand)', async () => {
    const fresh = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
    const p2 = await fresh.newPage();
    await login(p2);
    const collapsed = await p2.evaluate(
      () => document.getElementById('panel-left')?.classList.contains('collapsed'),
    );
    await fresh.close();
    return collapsed === false || 'left panel starts collapsed';
  });

  section('goal: responsive tiers');
  for (const w of [820, 1024, 1280]) {
    await check(`editor pane is usable at ${w}px (>=320px wide)`, async () => {
      await page.setViewportSize({ width: w, height: 900 });
      await page.waitForTimeout(700);
      const width = await page.evaluate(
        () => document.getElementById('editor-container')?.getBoundingClientRect().width ?? 0,
      );
      return width >= 320 || `editor pane was ${Math.round(width)}px`;
    });
    await check(`no horizontal document overflow at ${w}px`, async () => {
      const over = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      return over <= 1 || `overflow ${over}px`;
    });
  }
  await check('all toolbar actions reachable at 820px', async () => {
    await page.setViewportSize({ width: 820, height: 900 });
    await page.waitForTimeout(700);
    const hidden = await page.evaluate(() => {
      const bar = document.getElementById('editor-toolbar');
      if (!bar) return ['no toolbar'];
      const barRect = bar.getBoundingClientRect();
      return [...bar.querySelectorAll('button')]
        .filter((b) => {
          const r = b.getBoundingClientRect();
          return r.width === 0 || r.right > barRect.right + 1;
        })
        .map((b) => b.getAttribute('aria-label') || b.textContent.trim());
    });
    // Either nothing is clipped, or an overflow control exposes the rest.
    if (hidden.length === 0) return true;
    const hasOverflow = (await page.$('#editor-toolbar-overflow, [data-action="toolbar-overflow"]')) !== null;
    return hasOverflow || `clipped with no overflow menu: ${hidden.join(',')}`;
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.waitForTimeout(500);

  section('goal: accessible control language');
  await check('every icon button has an aria-label', async () => {
    const bad = await page.evaluate(() =>
      [...document.querySelectorAll('button')]
        .filter((b) => b.offsetParent !== null)
        .filter((b) => !b.getAttribute('aria-label') && b.textContent.trim().length <= 2)
        .map((b) => b.id || b.className || b.textContent.trim())
        .slice(0, 10),
    );
    return bad.length === 0 || `unlabelled: ${bad.join(',')}`;
  });
  await check('no emoji glyphs used as the sole control affordance', async () => {
    const emojiBtns = await page.evaluate(() =>
      [...document.querySelectorAll('button')]
        .filter((b) => b.offsetParent !== null)
        .filter((b) => /\p{Extended_Pictographic}/u.test(b.textContent))
        .map((b) => b.id || b.textContent.trim())
        .slice(0, 10),
    );
    return emojiBtns.length === 0 || `emoji buttons: ${emojiBtns.join(',')}`;
  });
  await check('icon buttons render as SVG', async () => {
    const svgCount = await page.evaluate(
      () => document.querySelectorAll('button svg, button use').length,
    );
    return svgCount > 0 || 'no inline SVG icons found';
  });

  section('goal: keyboard');
  await check('Escape closes every modal', async () => {
    const opens = [
      ['#model-modal', '#model-name'],
      ['#search-modal', '#btn-search'],
      ['#archive-modal', '#btn-archive'],
      ['#new-note-modal', '#btn-new-note'],
      ['#logout-modal', '#btn-logout'],
    ];
    const broken = [];
    await openLeftPanel(page);
    for (const [modal, trigger] of opens) {
      await page.click(trigger);
      await page.waitForTimeout(400);
      await page.keyboard.press('Escape');
      await page.waitForTimeout(400);
      const closed = await page.evaluate(
        (s) => document.querySelector(s)?.classList.contains('hidden'),
        modal,
      );
      if (!closed) {
        broken.push(modal);
        await page.click(`${modal} .btn-secondary`).catch(() => {});
        await page.waitForTimeout(300);
      }
    }
    return broken.length === 0 || `stayed open: ${broken.join(',')}`;
  });
  await check('Ctrl+K opens the command palette', async () => {
    await page.keyboard.press('Control+k');
    await page.waitForTimeout(500);
    const open = await page.evaluate(() => {
      const el = document.getElementById('command-palette');
      return !!el && !el.classList.contains('hidden');
    });
    await page.keyboard.press('Escape');
    return open || '#command-palette not shown';
  });

  section('goal: safe destructive actions');
  await check('toggling note visibility asks for confirmation', async () => {
    const patches = [];
    await page.route('**/api/notes/**', (route) => {
      if (route.request().method() === 'PATCH') patches.push(route.request().url());
      route.continue();
    });
    await openLeftPanel(page);
    await page.evaluate(() => {
      document.querySelectorAll('#note-tree .icon.clickable').forEach((e) => (e.style.opacity = '1'));
    });
    const icon = await page.$('#note-tree .icon.clickable');
    if (!icon) return 'no visibility toggle found';
    await icon.click();
    await page.waitForTimeout(600);
    const confirmShown = await page.evaluate(() =>
      [...document.querySelectorAll('.modal-overlay')].some((m) => !m.classList.contains('hidden')),
    );
    await page.keyboard.press('Escape');
    await page.unroute('**/api/notes/**');
    if (patches.length > 0 && !confirmShown) return 'PATCH fired with no confirmation';
    return confirmShown || 'no confirmation dialog appeared';
  });

  section('goal: no dead UI');
  await check('Note Settings has no empty filler form-group', async () => {
    await page.evaluate(() => {
      document.querySelectorAll('#note-tree .note-actions button, #note-tree button').forEach((b) => {
        b.style.opacity = '1';
      });
    });
    const gear = await page.$('#note-tree [data-action="note-settings"], #note-tree .note-actions button:last-child');
    if (!gear) return 'settings control not found';
    await gear.click();
    await page.waitForTimeout(600);
    const empties = await page.evaluate(() =>
      [...document.querySelectorAll('#note-settings-modal .form-group')].filter(
        (g) => g.textContent.trim() === '' && g.querySelectorAll('input,select,button').length === 0,
      ).length,
    );
    await page.keyboard.press('Escape');
    await page.click('#note-settings-modal .btn-secondary').catch(() => {});
    return empties === 0 || `${empties} empty form-group(s)`;
  });
  await check('chat does not stack duplicate connection notices', async () => {
    const texts = await page.$$eval('#chat-messages .system, #chat-messages > *', (els) =>
      els.map((e) => e.textContent.trim()).filter(Boolean),
    );
    const connectish = texts.filter((t) => /connected|joined/i.test(t));
    return connectish.length <= 1 || `${connectish.length} notices: ${connectish.join(' | ')}`;
  });

  await ctx.close();
}

// ─────────────────────────────── driver ───────────────────────────────

(async () => {
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.RUNE_UI_CHROME || undefined,
    args: ['--no-sandbox'],
  });
  try {
    if (suiteArg === 'all' || suiteArg === 'regression') await runRegression(browser);
    if (suiteArg === 'all' || suiteArg === 'goals') await runGoals(browser);
  } finally {
    await browser.close();
  }
  console.log(`\n${'\u2500'.repeat(56)}`);
  console.log(`  passed: ${pass}   failed: ${fail}`);
  if (failures.length) {
    console.log('\n  failures:');
    failures.forEach((f) => console.log(`    - ${f}`));
  }
  process.exit(fail === 0 ? 0 : 1);
})();
