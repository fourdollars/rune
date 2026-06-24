# Hide Context Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hide the context used overlay in the chat input panel when the message input has text, and show it again when the input is empty.

**Architecture:** Use CSS `:placeholder-shown` pseudo-class and sibling combinator to control display status of `#context-overlay` based on `#chat-input`. Add a Playwright test to verify the functionality.

**Tech Stack:** CSS, Vanilla JS, Playwright (Chromium)

## Global Constraints
- Do not break `cargo fmt` — run `cargo fmt --all` before committing.
- Do not add `// removed` or `// TODO` shims.
- CSS changes should be placed in `web/style.css`.
- Playwright tests should be run and verified using the embedded assets in notes serve mode.

---

### Task 1: Implement CSS Rule to Hide Context Overlay

**Files:**
- Modify: `web/style.css`

**Interfaces:**
- Consumes: `#chat-input` and `#context-overlay` layout in `web/index.html`
- Produces: CSS rules that toggle `#context-overlay` visibility based on `#chat-input` value

- [ ] **Step 1: Write the implementation in `web/style.css`**

Add the following rule to [web/style.css](file:///home/sylee/side/rune/web/style.css) around line 1640 (near other `.context-overlay` rules):

```css
/* Hide context overlay when chat input contains text */
#chat-input:not(:placeholder-shown) ~ #context-overlay {
    display: none !important;
}
```

- [ ] **Step 2: Commit CSS change**

Run:
```bash
git add web/style.css
git commit -m "feat(web): hide context overlay when chat input is not empty"
```

---

### Task 2: Add Playwright Test Case

**Files:**
- Modify: `tests/routing_smoke.js`

**Interfaces:**
- Consumes: Playwright tests in `tests/routing_smoke.js`
- Produces: A new test case verifying the visibility of `#context-overlay` during input state changes

- [ ] **Step 1: Add Test 13 to `tests/routing_smoke.js`**

Insert the following test case in [tests/routing_smoke.js](file:///home/sylee/side/rune/tests/routing_smoke.js) before `await browser.close();` (around line 210):

```js
  // ── Test 13: Hide context-overlay when chat-input is not empty ───────
  await withPage(browser, async (page) => {
    // Login first
    await page.goto(BASE + '/notes/Rune/routing');
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
```

- [ ] **Step 2: Build the project and run the smoke tests to verify**

Compile the release build to embed frontend changes:
```bash
cargo build --release
```

Start the server in background:
```bash
./target/release/rune notes --bind 0.0.0.0 &
SERVER_PID=$!
sleep 2
```

Run the smoke test:
```bash
node tests/routing_smoke.js
```
Verify that all tests, including Test 13, pass.

Shut down the server:
```bash
kill $SERVER_PID
```

- [ ] **Step 3: Commit test changes**

Run:
```bash
git add tests/routing_smoke.js
git commit -m "test(e2e): add test case for toggling context overlay on input"
```
