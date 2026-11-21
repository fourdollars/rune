// Unit test for Rune Notes Admin Settings modal and config reload UI
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

function createDOM() {
  const elements = new Map();

  class MockClassList {
    constructor() {
      this.classes = new Set();
    }
    add(...cls) { cls.forEach(c => this.classes.add(c)); }
    remove(...cls) { cls.forEach(c => this.classes.delete(c)); }
    contains(cls) { return this.classes.has(cls); }
    toggle(cls, force) {
      if (force === undefined) {
        if (this.classes.has(cls)) { this.classes.delete(cls); return false; }
        this.classes.add(cls); return true;
      }
      if (force) { this.classes.add(cls); return true; }
      this.classes.delete(cls); return false;
    }
  }

  class MockElement {
    constructor(tagName = 'div', id = '') {
      this.tagName = tagName.toUpperCase();
      this.id = id;
      this.attributes = new Map();
      this.classList = new MockClassList();
      this.dataset = {};
      this.value = '';
      this.checked = false;
      this.placeholder = '';
      this.textContent = '';
      if (id) elements.set(id, this);
    }
    get className() { return Array.from(this.classList.classes).join(' '); }
    set className(val) {
      this.classList.classes.clear();
      if (val) val.split(/\s+/).filter(Boolean).forEach(c => this.classList.add(c));
    }
    setAttribute(k, v) { this.attributes.set(k, String(v)); }
    getAttribute(k) { return this.attributes.get(k) || null; }
    appendChild(child) {
      this.children = this.children || [];
      this.children.push(child);
      return child;
    }
    replaceChildren(...nodes) {
      this.children = [...nodes];
    }
  }

  const tabs = ['provider', 'notes'].map(name => {
    const btn = new MockElement('button');
    btn.dataset.adminTab = name;
    return btn;
  });
  const panels = ['provider', 'notes'].map(name => {
    const panel = new MockElement('div');
    panel.dataset.adminPanel = name;
    return panel;
  });

  const ids = [
    'admin-settings-modal',
    'admin-settings-status', 'admin-cfg-provider', 'admin-cfg-api-key',
    'admin-cfg-base-url', 'admin-cfg-base-url-field', 'admin-cfg-model', 'admin-cfg-notes-model',
    'admin-cfg-thinking', 'admin-cfg-skills-dir', 'admin-cfg-openrouter-zdr', 'admin-cfg-zdr-field',
    'admin-cfg-embedding-enabled', 'admin-cfg-embedding-model',
    'admin-detect-models-btn', 'admin-model-search', 'admin-model-detect-status', 'admin-model-list',
    'admin-cfg-notes-title', 'admin-cfg-notes-desc', 'admin-cfg-notes-bind',
    'admin-cfg-notes-port', 'admin-cfg-agent-skills', 'admin-cfg-persona-files',
    'admin-cfg-cron-jobs', 'admin-cfg-local-enabled', 'admin-cfg-local-fields',
    'admin-cfg-local-admins', 'admin-cfg-local-users', 'admin-cfg-local-guests',
    'admin-cfg-github-enabled', 'admin-cfg-github-fields',
    'admin-cfg-github-client-id', 'admin-cfg-github-client-secret',
    'admin-cfg-github-admins', 'admin-cfg-github-users', 'admin-cfg-github-guests',
    'admin-copilot-box', 'admin-copilot-start-btn',
    'admin-copilot-cancel-btn', 'admin-copilot-code-row',
    'admin-copilot-user-code', 'admin-copilot-verify-link', 'admin-copilot-status',
  ];
  ids.forEach(id => new MockElement('input', id));
  elements.get('admin-settings-modal').classList.add('hidden');

  const document = {
    getElementById(id) { return elements.get(id) || null; },
    createElement(tag) { return new MockElement(tag); },
    querySelectorAll(selector) {
      if (selector.includes('data-admin-tab')) return tabs;
      if (selector.includes('data-admin-panel')) return panels;
      return [];
    },
  };

  return { document, elements, tabs, panels };
}

async function runTests() {
  console.log('=== Rune Notes Admin Settings Test Suite ===');
  const { document, elements, panels } = createDOM();
  const apiCalls = [];

  const mockConfigResponse = {
    ok: true,
    has_api_key: true,
    has_github_client_secret: true,
    config: {
      provider: 'openrouter',
      base_url: '',
      model: 'openrouter/auto',
      notes_model: 'openrouter/auto',
      thinking: 'low',
      openrouter_zdr: true,
      skills_dir: '~/skills',
      embedding_enabled: false,
      notes_port: 9527,
      notes_bind: '127.0.0.1',
      notes_title: 'My Notes',
      notes_desc: 'Desc',
      agent_skills: true,
      persona_files: true,
      cron_jobs: false,
      local_enabled: true,
      local_admins: ['admin'],
      local_users: [],
      local_guests: [],
      github_enabled: true,
      github_client_id: 'Ov23li123',
      github_admins: ['alice'],
      github_users: [],
      github_guests: [],
    },
  };

  const context = {
    document,
    isAdmin: false,
    personaFilesEnabled: false,
    cronJobsEnabled: false,
    api: async (endpoint, body, method) => {
      apiCalls.push({ endpoint, body, method });
      if (endpoint === 'admin/config' && method === 'GET') return mockConfigResponse;
      if (endpoint === 'admin/config' && method === 'PUT') {
        const restart = body.config && body.config.notes_port === 9999;
        return { ok: true, restart_required: restart, models_count: 3 };
      }
      if (endpoint === 'admin/config/reload' && method === 'POST') {
        return { ok: true, restart_required: false, models_count: 5 };
      }
      if (endpoint === 'admin/copilot/start' && method === 'POST') {
        return {
          ok: true,
          device_code: 'dev-code-1',
          user_code: 'ABCD-1234',
          verification_uri: 'https://github.com/login/device',
          interval: 0,
          expires_in: 900,
        };
      }
      if (endpoint === 'admin/copilot/poll' && method === 'POST') {
        return { ok: true, status: 'authorized', access_token: 'ghu_mock_copilot_token' };
      }
      if (endpoint === 'admin/models' && method === 'POST') {
        return {
          ok: true,
          models: [
            { id: 'gpt-4.1', context_window: 128000, reasoning_efforts: [] },
            { id: 'claude-sonnet-4', context_window: 200000, reasoning_efforts: ['low', 'high'] },
          ],
        };
      }
      return { ok: false };
    },
    setTimeout: (fn) => { fn(); return 1; },
    clearTimeout: () => {},
    console,
  };
  context.globalThis = context;
  vm.createContext(context);

  const adminJs = fs.readFileSync(path.join(__dirname, '../web/js/admin-settings.js'), 'utf8')
    .replace(/^import .*?;\s*/gm, '');
  vm.runInContext(adminJs, context);

  context.isAdmin = true;

  // Test 1: Opening modal loads config and masks secrets
  await context.showAdminSettings();
  assert.strictEqual(elements.get('admin-settings-modal').classList.contains('hidden'), false);
  assert.strictEqual(elements.get('admin-cfg-provider').value, 'openrouter');
  assert.strictEqual(elements.get('admin-cfg-api-key').value, '');
  assert.ok(elements.get('admin-cfg-api-key').placeholder.includes('leave blank to keep'));
  assert.strictEqual(elements.get('admin-cfg-github-client-secret').value, '');
  assert.ok(elements.get('admin-cfg-github-client-secret').placeholder.includes('leave blank to keep'));
  assert.strictEqual(elements.get('admin-cfg-local-admins').value, 'admin');
  console.log('✓ Test 2 passed: Modal loads config and masks secrets');

  // Test 3: Structured save with port change shows restart warning and sends no raw_toml
  elements.get('admin-cfg-notes-port').value = '9999';
  await context.saveAdminSettings();
  const putStructured = apiCalls.find(c => c.endpoint === 'admin/config' && c.method === 'PUT');
  assert.strictEqual(putStructured.body.raw_toml, undefined);
  assert.strictEqual(putStructured.body.config.notes_port, 9999);
  assert.strictEqual(putStructured.body.config.api_key, null);
  assert.ok(elements.get('admin-settings-status').classList.contains('warning'));
  console.log('✓ Test 3 passed: Structured save preserves blank secret and flags restart_required');

  // Test 4: Switching between Provider and Notes tabs works
  context.switchAdminSettingsTab('notes');
  assert.strictEqual(panels[1].classList.contains('hidden'), false);
  assert.strictEqual(panels[0].classList.contains('hidden'), true);
  console.log('✓ Test 4 passed: Provider/Notes tab switching works');

  // Test 5: Provider-adaptive ZDR & Base URL visibility + Copilot device flow triggers model detection
  assert.strictEqual(elements.get('admin-cfg-zdr-field').classList.contains('hidden'), false);
  assert.strictEqual(elements.get('admin-cfg-base-url-field').classList.contains('hidden'), true);

  elements.get('admin-cfg-provider').value = 'github-copilot';
  context.updateAdminCopilotVisibility();
  assert.strictEqual(elements.get('admin-copilot-box').classList.contains('hidden'), false);
  assert.strictEqual(elements.get('admin-cfg-zdr-field').classList.contains('hidden'), true);
  assert.strictEqual(elements.get('admin-cfg-base-url-field').classList.contains('hidden'), true);

  elements.get('admin-cfg-provider').value = 'ollama';
  context.updateAdminCopilotVisibility();
  assert.strictEqual(elements.get('admin-cfg-zdr-field').classList.contains('hidden'), true);
  assert.strictEqual(elements.get('admin-cfg-base-url-field').classList.contains('hidden'), false);

  elements.get('admin-cfg-provider').value = 'github-copilot';
  context.updateAdminCopilotVisibility();
  await context.startAdminCopilotFlow();
  await new Promise(r => setImmediate(r));
  assert.strictEqual(elements.get('admin-copilot-user-code').textContent, 'ABCD-1234');
  assert.strictEqual(elements.get('admin-cfg-api-key').value, 'ghu_mock_copilot_token');
  assert.ok(apiCalls.some(c => c.endpoint === 'admin/models' && c.method === 'POST'));
  assert.strictEqual(elements.get('admin-model-list').children.length, 2);

  context.toggleAdminDetectedModel('claude-sonnet-4');
  assert.ok(elements.get('admin-cfg-notes-model').value.includes('claude-sonnet-4'));
  console.log('✓ Test 5 passed: Provider-adaptive ZDR/BaseURL + Copilot flow auto-detects selectable models');

  console.log('All admin settings tests passed successfully!');
}

runTests().catch(err => {
  console.error(err);
  process.exit(1);
});
