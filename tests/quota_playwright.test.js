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
  let apiKeyLine = '';
  try {
    const userToml = fs.readFileSync(path.join(process.env.HOME, '.rune/rune.toml'), 'utf8');
    const m = userToml.match(/api_key\s*=\s*"([^"]+)"/);
    if (m) apiKeyLine = `api_key = "${m[1]}"`;
  } catch {}

  const tmpConfig = path.join(tmpDir, 'rune.toml');
  fs.writeFileSync(tmpConfig, `
${apiKeyLine}
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

  page.on('console', msg => console.log('[Browser console]', msg.text()));
  page.on('pageerror', err => console.log('[Browser error]', err.message));

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

    // Verify live provider usage if network available
    try {
      await page.waitForFunction(() => window.providerUsage && (window.providerUsage.quota_remaining !== undefined || window.providerUsage.details !== undefined || window.providerUsage.provider !== undefined), { timeout: 3000 });
      const liveUsage = await page.evaluate(() => window.providerUsage);
      console.log('Live backend providerUsage:', JSON.stringify(liveUsage));
    } catch {
      console.log('Live providerUsage not received within 3s, proceeding with deterministic simulation...');
    }

    // 3. Test Thinking Level Button & Modal
    console.log('Testing #thinking-btn and #thinking-modal...');
    await page.evaluate(() => {
      window.isAdmin = true;
      window.activeModel = 'claude-3.7-sonnet';
      window.availableModels = [
        { id: 'claude-3.7-sonnet', provider: 'github-copilot', context_window: 200000, reasoning_efforts: ['low', 'medium', 'high'] },
        { id: 'gpt-4o', provider: 'github-copilot', context_window: 128000, reasoning_efforts: [] }
      ];
      window.updateModelIndicator();
      window.updateThinkingButton();
    });
    await page.waitForTimeout(300);

    const isThinkingBtnVisible = await page.evaluate(() => {
      const btn = document.getElementById('thinking-btn');
      return btn && btn.offsetParent !== null;
    });
    console.log('Thinking button visible:', isThinkingBtnVisible);
    assert(isThinkingBtnVisible, 'Thinking button should be visible in chat header');

    // Click thinking button to open modal
    await page.click('#thinking-btn');
    await page.waitForTimeout(300);

    const isThinkingModalVisible = await page.evaluate(() => {
      const modal = document.getElementById('thinking-modal');
      return modal && !modal.classList.contains('hidden');
    });
    console.log('Thinking modal visible on click:', isThinkingModalVisible);
    assert(isThinkingModalVisible, 'Thinking modal must be visible on button click');

    // Click "medium" option
    const mediumOption = await page.$('.thinking-option[data-level="medium"]');
    assert(mediumOption, 'Medium thinking option should exist');
    await mediumOption.click();
    await page.waitForTimeout(300);

    const isThinkingModalClosed = await page.evaluate(() => {
      const modal = document.getElementById('thinking-modal');
      return modal && modal.classList.contains('hidden');
    });
    console.log('Thinking modal closed after selection:', isThinkingModalClosed);
    assert(isThinkingModalClosed, 'Thinking modal should close after selection');

    // 4. Test simulating providerUsage update on frontend (Battery Icon in Chat Header)
    console.log('Simulating Copilot providerUsage with 38,500 / 45,000 AI Credits...');
    await page.waitForFunction(() => typeof window.updateUsageIndicator === 'function', { timeout: 10000 });
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

    // Verify #quota-indicator is visible and has tooltip
    const isIndicatorVisible = await page.evaluate(() => {
      const el = document.getElementById('quota-indicator');
      return el && !el.classList.contains('hidden') && el.offsetParent !== null;
    });
    const indicatorTitle = await page.$eval('#quota-indicator', el => el.getAttribute('title') || '');
    console.log('Quota battery indicator visible:', isIndicatorVisible, 'Tooltip:', indicatorTitle);
    assert(isIndicatorVisible, 'Quota indicator should be visible in chat header');
    assert(indicatorTitle.includes('38.5k') || indicatorTitle.includes('86%'), `Indicator tooltip must show credits: ${indicatorTitle}`);

    // 5. Test clicking #quota-indicator to open #quota-popover
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

    // 5b. Test simulating OpenRouter providerUsage with USD balance and limit
    console.log('Simulating OpenRouter providerUsage with $8.50 / $10.00 balance...');
    await page.evaluate(() => {
      window.providerUsage = {
        provider: 'openrouter',
        plan_name: 'OpenRouter (Team Key)',
        quota_remaining: 850,
        quota_entitlement: 1000,
        quota_percent_remaining: 85.0,
        session_tokens: 2500,
        session_requests: 6,
        details: {
          balance: 8.5,
          limit: 10.0,
          usage: 1.5,
          total_credits: 50.0,
          total_usage: 10.0
        }
      };
      window.updateUsageIndicator();
    });
    await page.waitForTimeout(300);

    const openrouterTitle = await page.$eval('#quota-indicator', el => el.getAttribute('title') || '');
    console.log('OpenRouter tooltip:', openrouterTitle);
    assert.strictEqual(openrouterTitle, 'Budgets: $8.50 (85%)', `OpenRouter tooltip must show Budgets: $8.50 (85%): ${openrouterTitle}`);

    await page.click('#quota-indicator');
    await page.waitForTimeout(300);
    const orTitle = await page.$eval('.quota-popover-title', el => el.textContent);
    const orRemaining = await page.$eval('#quota-popover-remaining', el => el.textContent);
    const isPlanRowHidden = await page.$eval('#quota-popover-plan-row', el => el.style.display === 'none' || window.getComputedStyle(el).display === 'none');
    console.log('OpenRouter popover title:', orTitle, 'remaining:', orRemaining, 'Plan row hidden:', isPlanRowHidden);
    assert.strictEqual(orTitle, 'OpenRouter Budgets', `Popover title must be OpenRouter Budgets: ${orTitle}`);
    assert(orRemaining.includes('$8.50 / $10.00'), `Popover remaining must show $8.50 / $10.00: ${orRemaining}`);
    assert(isPlanRowHidden, 'Plan row must be hidden for OpenRouter');

    await page.click('#quota-indicator');
    await page.waitForTimeout(200);

    // 6. Test opening #model-modal (Model Switch Modal)
    console.log('Testing #model-modal...');
    await page.evaluate(() => {
      window.isAdmin = true;
      window.availableModels = [
        { id: 'claude-3.7-sonnet', provider: 'github-copilot', context_window: 200000, reasoning_efforts: ['low', 'medium', 'high'] },
        { id: 'gpt-4o', provider: 'github-copilot', context_window: 128000, reasoning_efforts: [] }
      ];
      window.showModelDialog();
    });
    await page.waitForTimeout(500);

    const isModalVisible = await page.evaluate(() => {
      const el = document.getElementById('model-modal');
      return el && !el.classList.contains('hidden');
    });
    const modelOptionsCount = await page.$$eval('#model-list .model-option', els => els.length);
    console.log('Model modal visible:', isModalVisible, 'Options count:', modelOptionsCount);
    assert(isModalVisible, 'Model modal must be visible');
    assert(modelOptionsCount === 2, 'Model modal should show 2 model options');

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
