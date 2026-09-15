// Playwright e2e test for AI Credits & Quota UI in Rune Notes
const { chromium } = require('/tmp/node_modules/playwright');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const assert = require('assert');

async function run() {
  console.log('=== Starting Playwright Quota UI Test ===');

  const tmpDir = '/tmp/rune-test-quota-' + Date.now();
  fs.mkdirSync(tmpDir, { recursive: true });
  const tmpConfig = path.join(tmpDir, 'rune.toml');
  fs.writeFileSync(tmpConfig, `
log_level = "info"
provider = "github-copilot"

[notes]
bind = "127.0.0.1"
port = 9588

[notes.local]
admins = ["admin:admin123"]
users = []
guests = []

[policy]
mode = "unrestricted"
`);

  const PORT = 9588;
  const server = spawn('./target/debug/rune', ['notes', '--config', tmpConfig, '--bind', '127.0.0.1', '--port', String(PORT), '--unrestricted'], {
    env: { ...process.env, RUST_LOG: 'info' },
    stdio: 'pipe',
  });

  server.stdout.on('data', d => process.stdout.write('[Rune stdout] ' + d));
  server.stderr.on('data', d => process.stderr.write('[Rune stderr] ' + d));

  async function waitForServer(port, retries = 30) {
    for (let i = 0; i < retries; i++) {
      try {
        await new Promise((resolve, reject) => {
          const req = http.get(`http://127.0.0.1:${port}/api/auth/config`, (res) => {
            resolve();
          });
          req.on('error', reject);
          req.setTimeout(500, () => { req.destroy(); reject(new Error('timeout')); });
        });
        return;
      } catch {
        await new Promise(r => setTimeout(r, 200));
      }
    }
    throw new Error('Server did not become ready in time');
  }

  await waitForServer(PORT);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await context.newPage();

  try {
    // 1. Log in via Local Auth
    console.log(`Logging in at http://127.0.0.1:${PORT}/ ...`);
    await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('#username');
    await page.fill('#username', 'admin');
    await page.fill('#password', 'admin123');
    await Promise.all([
      page.waitForURL(/\/edit\//, { timeout: 10000 }),
      page.click('#local-login-form button[type="submit"]'),
    ]);
    await page.waitForTimeout(1000);

    // 2. Check if DOM elements exist
    const quotaIndicator = await page.$('#quota-indicator');
    console.log('DOM #quota-indicator exists:', !!quotaIndicator);
    assert(quotaIndicator, '#quota-indicator must exist in DOM');

    const quotaPopover = await page.$('#quota-popover');
    console.log('DOM #quota-popover exists:', !!quotaPopover);
    assert(quotaPopover, '#quota-popover must exist in DOM');

    const modelModalQuota = await page.$('#model-modal-quota');
    console.log('DOM #model-modal-quota exists:', !!modelModalQuota);
    assert(modelModalQuota, '#model-modal-quota must exist in DOM');

    // Verify live provider usage is loaded from backend
    await page.waitForFunction(() => window.providerUsage && window.providerUsage.quota_remaining !== undefined, { timeout: 5000 });
    const liveUsage = await page.evaluate(() => window.providerUsage);
    console.log('Live backend providerUsage:', JSON.stringify(liveUsage));
    assert(liveUsage.quota_remaining > 0, 'Live quota remaining must be > 0');
    assert.strictEqual(liveUsage.provider, 'github-copilot');

    // 3. Test simulating providerUsage update on frontend (Option 1: Chat Header Badge)
    console.log('Simulating Copilot providerUsage with 38,500 / 45,000 AI Credits...');
    await page.evaluate(() => {
      window.providerUsage = {
        provider: 'github-copilot',
        plan_name: 'Copilot Pro',
        quota_remaining: 38500,
        quota_entitlement: 45000,
        quota_percent_remaining: 85.55,
        session_tokens: 1234,
        session_requests: 5
      };
      window.updateUsageIndicator();
    });

    await page.waitForTimeout(500);

    // Verify #quota-indicator is visible and displays correct text
    const isIndicatorVisible = await page.evaluate(() => {
      const el = document.getElementById('quota-indicator');
      return el && !el.classList.contains('hidden') && el.offsetParent !== null;
    });
    const indicatorText = await page.$eval('#quota-text', el => el.textContent);
    console.log('Quota indicator visible:', isIndicatorVisible, 'Text:', indicatorText);
    assert(isIndicatorVisible, 'Quota indicator should be visible in chat header');
    assert(indicatorText.includes('38.5k') || indicatorText.includes('86%'), `Indicator text must show credits: ${indicatorText}`);

    // 4. Test clicking #quota-indicator to open #quota-popover
    console.log('Clicking #quota-indicator to toggle popover...');
    await page.click('#quota-indicator');
    await page.waitForTimeout(300);

    const isPopoverVisible = await page.evaluate(() => {
      const el = document.getElementById('quota-popover');
      return el && !el.classList.contains('hidden');
    });
    const popoverRemaining = await page.$eval('#quota-popover-remaining', el => el.textContent);
    const popoverPlan = await page.$eval('#quota-popover-plan', el => el.textContent);
    console.log('Popover visible:', isPopoverVisible, 'Remaining:', popoverRemaining, 'Plan:', popoverPlan);
    assert(isPopoverVisible, 'Popover must be visible on click');
    assert(popoverRemaining.includes('38,500'), 'Popover must show remaining 38,500');
    assert(popoverPlan.includes('Copilot Pro'), 'Popover must show plan Copilot Pro');

    // Take screenshot of popover
    await page.screenshot({ path: '/tmp/quota_popover.png' });

    // Close popover
    await page.click('#quota-indicator');
    await page.waitForTimeout(200);

    // 5. Test opening #model-modal (Option 2: Model Switch Modal Banner)
    console.log('Testing #model-modal quota banner...');
    await page.evaluate(() => {
      window.isAdmin = true;
      window.availableModels = [
        { id: 'claude-3.7-sonnet', provider: 'github-copilot', context_window: 200000, reasoning_efforts: ['low', 'medium', 'high'] },
        { id: 'gpt-4o', provider: 'github-copilot', context_window: 128000, reasoning_efforts: [] }
      ];
      window.showModelDialog();
    });
    await page.waitForTimeout(500);

    const isModalQuotaVisible = await page.evaluate(() => {
      const el = document.getElementById('model-modal-quota');
      return el && !el.classList.contains('hidden');
    });
    const modalQuotaText = await page.$eval('#model-modal-quota-text', el => el.textContent);
    console.log('Model modal quota banner visible:', isModalQuotaVisible, 'Banner text:', modalQuotaText);
    assert(isModalQuotaVisible, 'Model modal quota banner must be visible');
    assert(modalQuotaText.includes('38,500'), 'Modal quota text must show 38,500');

    // Take screenshot of modal
    await page.screenshot({ path: '/tmp/quota_modal.png' });

    console.log('✅ ALL PLAYWRIGHT TESTS PASSED SUCCESSFULLY!');
  } finally {
    await browser.close();
    server.kill();
    try { fs.rmSync(tmpDir, { recursive: true, force: true }); } catch {}
  }
}

run().catch(err => {
  console.error('Playwright Test Failed:', err);
  process.exit(1);
});
