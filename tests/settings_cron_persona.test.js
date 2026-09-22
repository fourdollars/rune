// Unit test for Rune Notes Settings: Persona files and Scheduled Cron jobs
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
    add(cls) { this.classes.add(cls); }
    remove(cls) { this.classes.delete(cls); }
    contains(cls) { return this.classes.has(cls); }
    toggle(cls, force) {
      if (force !== undefined) {
        if (force) this.classes.add(cls);
        else this.classes.delete(cls);
        return force;
      }
      if (this.classes.has(cls)) {
        this.classes.delete(cls);
        return false;
      } else {
        this.classes.add(cls);
        return true;
      }
    }
  }

  class MockElement {
    constructor(tagName = 'div', id = '') {
      this.tagName = tagName.toUpperCase();
      this.id = id;
      this.attributes = new Map();
      this.dataset = {};
      this.classList = new MockClassList();
      this.children = [];
      this.parentNode = null;
      this.style = {};
      this._textContent = '';
      this.listeners = new Map();
      this.disabled = false;
      this.checked = false;
      this.selectionStart = 0;
      this.selectionEnd = 0;
      if (id) elements.set(id, this);
    }
    addEventListener(event, handler) {
      if (!this.listeners.has(event)) this.listeners.set(event, []);
      this.listeners.get(event).push(handler);
    }
    dispatchEvent(event) {
      const handlers = this.listeners.get(event.type) || [];
      handlers.forEach(h => h(event));
    }
    setSelectionRange(start, end) {
      this.selectionStart = start;
      this.selectionEnd = end;
    }
    get id() { return this._id || ''; }
    set id(val) {
      if (this._id) elements.delete(this._id);
      this._id = val;
      if (val) elements.set(val, this);
    }
    get className() { return Array.from(this.classList.classes).join(' '); }
    set className(val) {
      this.classList.classes.clear();
      if (val) val.split(/\s+/).filter(Boolean).forEach(c => this.classList.add(c));
    }
    get textContent() { return this._textContent; }
    set textContent(val) {
      this._textContent = String(val);
      this.children = [];
    }
    get value() { return this._value; }
    set value(val) {
      this._value = String(val);
      if (this.selectionStart === 0 && this.selectionEnd === 0) {
        this.selectionStart = this._value.length;
        this.selectionEnd = this._value.length;
      }
    }
    get innerHTML() { return this._textContent; }
    set innerHTML(val) {
      this.children = [];
      this._textContent = String(val);
    }
    focus() {}
    setAttribute(k, v) { this.attributes.set(k, String(v)); }
    getAttribute(k) { return this.attributes.get(k) || null; }
    closest(sel) {
      let curr = this;
      while (curr) {
        if (sel.startsWith('.') && curr.classList.contains(sel.slice(1))) return curr;
        curr = curr.parentNode;
      }
      return null;
    }
    appendChild(child) {
      child.parentNode = this;
      this.children.push(child);
      return child;
    }
    append(...children) {
      for (const child of children) {
        if (typeof child === 'string') {
          const textEl = new MockElement('span');
          textEl.textContent = child;
          this.appendChild(textEl);
        } else if (child) {
          this.appendChild(child);
        }
      }
    }
    replaceChildren(...newChildren) {
      this.children = [];
      this._textContent = '';
      this.append(...newChildren);
    }
    querySelector(sel) {
      if (sel.startsWith('.')) {
        const cls = sel.slice(1);
        const search = (node) => {
          for (const c of node.children) {
            if (c.classList && c.classList.contains(cls)) return c;
            const nested = search(c);
            if (nested) return nested;
          }
          return null;
        };
        return search(this);
      }
      return this.children.find(c => c.tagName.toLowerCase() === sel.toLowerCase()) || null;
    }
    querySelectorAll(sel) {
      return this.children.filter(c => c.tagName.toLowerCase() === sel.toLowerCase());
    }
  }

  const document = {
    getElementById(id) {
      return elements.get(id) || null;
    },
    createElement(tag) {
      return new MockElement(tag);
    },
    createTextNode(text) {
      const el = new MockElement('span');
      el.textContent = text;
      return el;
    },
    getElementsByName(name) {
      return Array.from(elements.values()).filter(e => e.name === name || e.getAttribute('name') === name);
    },
    querySelector(sel) {
      if (sel.startsWith('#')) {
        const parts = sel.split(/\s+/);
        if (parts.length === 1) {
          return elements.get(parts[0].slice(1)) || null;
        }
        if (parts.length === 2 && parts[1].startsWith('.')) {
          const root = elements.get(parts[0].slice(1));
          if (root) return root.querySelector('.' + parts[1].slice(1));
        }
      }
      if (sel.startsWith('.')) {
        const match = sel.match(/^\.([a-zA-Z0-9_-]+)(?:\[([a-zA-Z0-9_-]+)="([^"]+)"\])?$/);
        if (match) {
          const [, cls, attr, val] = match;
          return Array.from(elements.values()).find(e => {
            if (!e.classList.contains(cls)) return false;
            if (attr) {
              if (attr.startsWith('data-')) {
                const dataKey = attr.slice(5);
                return e.dataset[dataKey] === val;
              }
              return e.getAttribute(attr) === val;
            }
            return true;
          }) || null;
        }
      }
      return null;
    },
    querySelectorAll(sel) {
      if (sel.startsWith('.')) {
        const parts = sel.split(/\s+/).map(p => p.replace(/^\./, ''));
        const lastCls = parts[parts.length - 1];
        return Array.from(elements.values()).filter(e => e.classList.contains(lastCls));
      }
      if (sel.startsWith('[')) {
        const match = sel.match(/^\[([a-zA-Z0-9_-]+)="([^"]+)"\]$/);
        if (match) {
          const [, attr, val] = match;
          return Array.from(elements.values()).filter(e => {
            if (e.getAttribute(attr) === val || e.attributes.get(attr) === val) return true;
            if (attr.startsWith('data-')) {
              const camelKey = attr.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
              return e.dataset[camelKey] === val || e.dataset[attr.slice(5)] === val;
            }
            return false;
          });
        }
      }
      return [];
    }
  };

  return { document, elements, MockElement };
}

async function runTests() {
  console.log('=== Rune Notes Settings: Persona & Cron Jobs Test Suite ===');

  const settingsRaw = fs.readFileSync(path.join(__dirname, '../web/js/settings.js'), 'utf8');
  const settingsCode = settingsRaw.replace(/^import\s+[^;]+;/gm, '');

  // Test 1: loadPersonaStatus handles direct API response { ok: true, files: [...] }
  {
    const { document, elements, MockElement } = createDOM();
    const grid = new MockElement('div', 'persona-files-grid');

    let apiCalled = false;
    const mockApi = async (endpoint) => {
      if (endpoint.includes('persona/status')) {
        apiCalled = true;
        return {
          ok: true,
          files: [
            { name: 'AGENTS.md', present: true, size: 1024 },
            { name: 'HEARTBEAT.md', present: false, size: 0 },
            { name: 'IDENTITY.md', present: true, size: 512 }
          ]
        };
      }
      return { ok: false, error: 'Not found' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      PERSONA_DESCRIPTIONS: {
        'AGENTS.md': 'Agent capabilities and guidelines',
        'HEARTBEAT.md': 'Heartbeat periodic checks and background tasks',
        'IDENTITY.md': 'Agent persona and name definition'
      },
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    await sandbox.loadPersonaStatus();
    assert.ok(apiCalled, 'api should be called for persona/status');
    assert.strictEqual(grid.children.length, 3, 'Grid should render 3 persona cards');

    const agentsCard = grid.children[0];
    assert.ok(agentsCard.className.includes('present'), 'AGENTS.md should be marked present');

    const heartbeatCard = grid.children[1];
    assert.ok(heartbeatCard.className.includes('missing'), 'HEARTBEAT.md should be marked missing');
    assert.ok(heartbeatCard.className.includes('featured'), 'HEARTBEAT.md should have featured class');

    console.log('✓ Test 1 passed: loadPersonaStatus rendered persona files without Network error');
  }

  // Test 2: loadCronJobs handles direct API response { ok: true, jobs: [...], persona_files_enabled: true }
  {
    const { document, elements, MockElement } = createDOM();
    const list = new MockElement('div', 'cron-jobs-list');
    const presetsContainer = new MockElement('div', 'cron-presets-container');

    const mockJobs = [
      {
        id: 'job-123',
        name: 'Heartbeat Check',
        schedule_type: 'interval',
        schedule_value: '30m',
        prompt: 'Check HEARTBEAT.md status',
        model: 'deepseek-chat',
        silent_if_no_action: true,
        enabled: true,
        last_run_at: '2026-09-21T12:00:00Z',
        last_status: 'silent_ok'
      }
    ];

    let apiCalled = false;
    const mockApi = async (endpoint) => {
      if (endpoint.includes('/jobs')) {
        apiCalled = true;
        return {
          ok: true,
          jobs: mockJobs,
          persona_files_enabled: true
        };
      }
      return { ok: false, error: 'Not found' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      PERSONA_DESCRIPTIONS: {},
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    await sandbox.loadCronJobs();
    assert.ok(apiCalled, 'api should be called for jobs');
    assert.strictEqual(list.children.length, 1, 'Job list should render 1 card');
    assert.strictEqual(presetsContainer.classList.contains('hidden'), false, 'Presets container should not be hidden when persona_files_enabled is true');

    const card = list.children[0];
    assert.ok(card.className.includes('active'), 'Active job should have active card class');

    console.log('✓ Test 2 passed: loadCronJobs rendered scheduled jobs correctly');
  }

  // Test 3: applyCronPreset sets English prompt strings
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'cron-job-modal');
    const title = new MockElement('h3', 'cron-job-modal-title');
    const jobId = new MockElement('input', 'cron-job-id');
    const jobName = new MockElement('input', 'cron-job-name');
    const schedVal = new MockElement('input', 'cron-schedule-value');
    const prompt = new MockElement('textarea', 'cron-job-prompt');
    const modelSelect = new MockElement('select', 'cron-job-model');
    const silentCheck = new MockElement('input', 'cron-job-silent');
    const enabledCheck = new MockElement('input', 'cron-job-enabled');

    const sandbox = {
      document,
      globalThis: {},
      api: async () => ({ ok: true }),
      settingsNoteId: 'my-note',
      PERSONA_DESCRIPTIONS: {},
      models: [{ id: 'copilot/gpt-4o' }],
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    sandbox.applyCronPreset('heartbeat');
    assert.strictEqual(jobName.value, 'Heartbeat Periodic Check');
    assert.strictEqual(schedVal.value, '30m');
    assert.ok(prompt.value.includes('Follow @HEARTBEAT.md'), 'Heartbeat prompt must be in English');
    assert.strictEqual(silentCheck.checked, true);

    sandbox.applyCronPreset('memory');
    assert.strictEqual(jobName.value, 'Daily Memory Distillation');
    assert.ok(prompt.value.includes('MEMORY.md'), 'Memory prompt must reference MEMORY.md');
    assert.ok(!/[^\x00-\x7F]/.test(prompt.value), 'Memory prompt must contain ASCII English only');

    sandbox.applyCronPreset('weekly');
    assert.strictEqual(jobName.value, 'Weekly Summary Report');
    assert.ok(prompt.value.includes('weekly-report.md'), 'Weekly prompt must reference weekly-report.md');
    assert.ok(!/[^\x00-\x7F]/.test(prompt.value), 'Weekly prompt must contain ASCII English only');

    console.log('✓ Test 3 passed: applyCronPreset verified with English prompt presets');
  }

  // Test 4: viewCronLogs processes log list without Network error
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'cron-logs-modal');
    const tbody = new MockElement('tbody', 'cron-logs-tbody');
    const stats = new MockElement('div', 'cron-logs-stats');

    const mockLogs = [
      {
        timestamp: '2026-09-21T14:30:00Z',
        trigger: 'cron',
        status: 'silent_ok',
        duration_ms: 320,
        files_modified: [],
        output_snippet: 'HEARTBEAT_OK',
        model: 'openrouter/auto',
        thinking: 'low',
        steps: 1,
        tokens_in: 50,
        tokens_out: 10,
        tokens_used: 60,
        tools: []
      },
      {
        timestamp: '2026-09-21T15:00:00Z',
        trigger: 'manual',
        status: 'success',
        duration_ms: 680,
        files_modified: ['status.md'],
        output_snippet: 'Updated status',
        model: 'deepseek/deepseek-chat',
        thinking: 'high',
        steps: 3,
        tokens_in: 200,
        tokens_out: 150,
        tokens_used: 350,
        tools: ['read_markdown', 'write_markdown']
      }
    ];

    let apiCalled = false;
    const mockApi = async (endpoint) => {
      if (endpoint.includes('/logs')) {
        apiCalled = true;
        return {
          ok: true,
          logs: mockLogs
        };
      }
      return { ok: false, error: 'Not found' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      PERSONA_DESCRIPTIONS: {},
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    await sandbox.viewCronLogs('job-123');
    assert.ok(apiCalled, 'api should be called for logs');
    const mainRows = tbody.children.filter(r => r.className.includes('log-row'));
    const detailRows = tbody.children.filter(r => r.className.includes('log-detail-row'));
    assert.strictEqual(mainRows.length, 2, 'Table should contain 2 main log rows');
    assert.strictEqual(detailRows.length, 2, 'Table should contain 2 expandable detail rows');
    assert.ok(stats.innerHTML.includes('Total Runs: <strong>2</strong>'), 'Stats should show total runs');
    assert.ok(stats.innerHTML.includes('Silent Skips: <strong class="badge-silent">1</strong>'), 'Stats should show silent count');
    assert.ok(stats.innerHTML.includes('Success: <strong class="badge-present">2</strong>'), 'Stats should show success count');

    // Test expanding detail row
    const firstRow = mainRows[0];
    const firstDetail = elements.get('log-detail-0');
    assert.ok(firstDetail, 'Detail row for index 0 should exist');
    assert.strictEqual(firstDetail.classList.contains('hidden'), true, 'Detail row should start hidden');

    // Verify metadata rendered in detail row
    const firstMeta = firstDetail.querySelector('.log-detail-meta');
    assert.ok(firstMeta, 'firstMeta should exist');
    assert.ok(firstMeta.innerHTML.includes('openrouter/auto'), 'Should show model in detail row');
    assert.ok(firstMeta.innerHTML.includes('Thinking:'), 'Should show thinking label');
    assert.ok(firstMeta.innerHTML.includes('low'), 'Should show thinking value');
    assert.ok(firstMeta.innerHTML.includes('Steps:'), 'Should show steps label');
    assert.ok(firstMeta.innerHTML.includes('Tokens:'), 'Should show tokens label');
    assert.ok(firstMeta.innerHTML.includes('50 in / 10 out (60 total)'), 'Should show token breakdown');
    assert.ok(firstMeta.innerHTML.includes('Tools Used:'), 'Should show tools label');

    const secondDetail = elements.get('log-detail-1');
    const secondMeta = secondDetail.querySelector('.log-detail-meta');
    assert.ok(secondMeta, 'secondMeta should exist');
    assert.ok(secondMeta.innerHTML.includes('deepseek/deepseek-chat'), 'Should show model in 2nd detail row');
    assert.ok(secondMeta.innerHTML.includes('200 in / 150 out (350 total)'), 'Should show 2nd token breakdown');
    assert.ok(secondMeta.innerHTML.includes('read_markdown, write_markdown'), 'Should show tools used in 2nd detail row');
    assert.ok(secondMeta.innerHTML.includes('status.md'), 'Should show modified files');

    sandbox.toggleLogDetail(firstRow);
    assert.strictEqual(firstDetail.classList.contains('hidden'), false, 'Detail row should be visible after toggle');
    assert.strictEqual(firstRow.classList.contains('is-expanded'), true, 'Row should have is-expanded class');
    assert.strictEqual(firstRow.getAttribute('aria-expanded'), 'true');

    // Toggle again collapses
    sandbox.toggleLogDetail(firstRow);
    assert.strictEqual(firstDetail.classList.contains('hidden'), true, 'Detail row should be hidden again after second toggle');
    assert.strictEqual(firstRow.classList.contains('is-expanded'), false);

    console.log('✓ Test 4 passed: viewCronLogs processed execution history, metadata, and expandable detail rows');
  }

  // Test 5: Error responses from API gracefully set error messages without showing undefined
  {
    const { document, elements, MockElement } = createDOM();
    const grid = new MockElement('div', 'persona-files-grid');

    const mockErrorApi = async () => {
      return { ok: false, error: 'Database connection failed' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockErrorApi,
      settingsNoteId: 'my-note',
      PERSONA_DESCRIPTIONS: {},
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    await sandbox.loadPersonaStatus();
    assert.ok(grid.innerHTML.includes('Database connection failed'), 'Grid should display actual error message');
    assert.ok(!grid.innerHTML.includes('undefined'), 'Grid must never display undefined');

    console.log('✓ Test 5 passed: error handling verified without displaying undefined');
  }

  // Test 6: api helper handles empty body / 404 without throwing 'Unexpected end of JSON input'
  {
    const apiRaw = fs.readFileSync(path.join(__dirname, '../web/js/api.js'), 'utf8');
    const apiCode = apiRaw.replace(/^export\s+/gm, '');

    // Mock empty 404 fetch response
    const mockEmptyFetch = async () => ({
      ok: false,
      status: 404,
      statusText: 'Not Found',
      text: async () => ''
    });

    const sandbox = {
      fetch: mockEmptyFetch,
      globalThis: {},
      console
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(apiCode, sandbox);

    const res = await sandbox.api('notes/test/persona/status');
    assert.strictEqual(res.ok, false, 'Empty 404 response should have ok: false');
    assert.strictEqual(res.error, 'HTTP 404 Not Found', 'Error should describe HTTP 404 cleanly');
    assert.ok(!res.error.includes('Unexpected end of JSON input'), 'Must never fail with Unexpected end of JSON input');

    console.log('✓ Test 6 passed: api helper handles empty HTTP 404 without Unexpected end of JSON input');
  }

  // Test 7: api helper handles HTML error page without crashing
  {
    const apiRaw = fs.readFileSync(path.join(__dirname, '../web/js/api.js'), 'utf8');
    const apiCode = apiRaw.replace(/^export\s+/gm, '');

    // Mock HTML 502 Bad Gateway response
    const mockHtmlFetch = async () => ({
      ok: false,
      status: 502,
      statusText: 'Bad Gateway',
      text: async () => '<html><body>502 Bad Gateway</body></html>'
    });

    const sandbox = {
      fetch: mockHtmlFetch,
      globalThis: {},
      console
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(apiCode, sandbox);

    const res = await sandbox.api('notes/test/jobs');
    assert.strictEqual(res.ok, false, 'HTML response should have ok: false');
    assert.ok(res.error.includes('502 Bad Gateway'), 'Error should contain HTML snippet or status description');
    assert.ok(!res.error.includes('Unexpected token'), 'Must not throw uncaught Unexpected token JSON parse error');

    console.log('✓ Test 7 passed: api helper handles non-JSON HTML body gracefully');
  }

  // Test 8: showNoteSettings hides Persona and Cron tabs when opt-in flags are false (default)
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'note-settings-modal');
    const title = new MockElement('h2', 'note-settings-title');
    const nameInput = new MockElement('input', 'note-settings-name');
    const deleteBtn = new MockElement('button', 'btn-delete-note');

    const generalTab = new MockElement('button');
    generalTab.className = 'settings-tab active';
    generalTab.dataset.tab = 'general';

    const personaTab = new MockElement('button');
    personaTab.className = 'settings-tab';
    personaTab.dataset.tab = 'persona';

    const cronTab = new MockElement('button');
    cronTab.className = 'settings-tab';
    cronTab.dataset.tab = 'cron';

    const generalPanel = new MockElement('div', 'settings-tab-general');
    generalPanel.className = 'settings-tab-panel';
    const personaPanel = new MockElement('div', 'settings-tab-persona');
    personaPanel.className = 'settings-tab-panel hidden';
    const cronPanel = new MockElement('div', 'settings-tab-cron');
    cronPanel.className = 'settings-tab-panel hidden';

    elements.set('tab-gen', generalTab);
    elements.set('tab-persona', personaTab);
    elements.set('tab-cron', cronTab);

    const sandbox = {
      document,
      globalThis: {},
      notes: [{ id: 'my-note', name: 'My Note' }],
      personaFilesEnabled: false,
      cronJobsEnabled: false,
      renderNoteIconTrigger: () => {},
      markSelectedEmoji: () => {},
      closeEmojiPicker: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    sandbox.showNoteSettings('my-note');
    assert.strictEqual(personaTab.style.display, 'none', 'Persona Files tab button must be hidden when persona_files is not enabled');
    assert.strictEqual(cronTab.style.display, 'none', 'Scheduled Jobs tab button must be hidden when cron_jobs is not enabled');

    // Test enabling flags
    sandbox.personaFilesEnabled = true;
    sandbox.cronJobsEnabled = true;
    sandbox.showNoteSettings('my-note');
    assert.strictEqual(personaTab.style.display, '', 'Persona Files tab button must be visible when persona_files = true');
    assert.strictEqual(cronTab.style.display, '', 'Scheduled Jobs tab button must be visible when cron_jobs = true');

    console.log('✓ Test 8 passed: showNoteSettings correctly hides/shows tabs based on opt-in configuration flags');
  }

  // Test 9: Quick Presets visibility when cron_jobs = true and persona_files = false
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'note-settings-modal');
    const title = new MockElement('h2', 'note-settings-title');
    const nameInput = new MockElement('input', 'note-settings-name');
    const deleteBtn = new MockElement('button', 'btn-delete-note');

    const generalTab = new MockElement('button');
    generalTab.className = 'settings-tab active';
    generalTab.dataset.tab = 'general';

    const cronTab = new MockElement('button');
    cronTab.className = 'settings-tab';
    cronTab.dataset.tab = 'cron';

    const heartbeatPreset = new MockElement('button');
    heartbeatPreset.className = 'btn-secondary btn-chip';
    heartbeatPreset.dataset.action = 'apply-cron-preset';
    heartbeatPreset.dataset.preset = 'heartbeat';
    heartbeatPreset.dataset.requirePersona = 'true';

    const memoryPreset = new MockElement('button');
    memoryPreset.className = 'btn-secondary btn-chip';
    memoryPreset.dataset.action = 'apply-cron-preset';
    memoryPreset.dataset.preset = 'memory';
    memoryPreset.dataset.requirePersona = 'true';

    const weeklyPreset = new MockElement('button');
    weeklyPreset.className = 'btn-secondary btn-chip';
    weeklyPreset.dataset.action = 'apply-cron-preset';
    weeklyPreset.dataset.preset = 'weekly';

    elements.set('tab-gen', generalTab);
    elements.set('tab-cron', cronTab);
    elements.set('preset-hb', heartbeatPreset);
    elements.set('preset-mem', memoryPreset);
    elements.set('preset-week', weeklyPreset);

    const sandbox = {
      document,
      globalThis: {},
      notes: [{ id: 'my-note', name: 'My Note' }],
      personaFilesEnabled: false,
      cronJobsEnabled: true,
      renderNoteIconTrigger: () => {},
      markSelectedEmoji: () => {},
      closeEmojiPicker: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    sandbox.showNoteSettings('my-note');

    // Weekly report must always be visible when cron_jobs is enabled
    assert.ok(weeklyPreset.style.display !== 'none', 'Weekly report preset must be visible when cron_jobs = true');
    // Heartbeat and Memory presets must be hidden when persona_files = false
    assert.strictEqual(heartbeatPreset.style.display, 'none', 'Heartbeat preset must be hidden when persona_files = false');
    assert.strictEqual(memoryPreset.style.display, 'none', 'Memory preset must be hidden when persona_files = false');

    // When persona_files is enabled, all presets become visible
    sandbox.personaFilesEnabled = true;
    sandbox.showNoteSettings('my-note');
    assert.ok(weeklyPreset.style.display !== 'none', 'Weekly report preset must remain visible');
    assert.strictEqual(heartbeatPreset.style.display, '', 'Heartbeat preset must be visible when persona_files = true');
    assert.strictEqual(memoryPreset.style.display, '', 'Memory preset must be visible when persona_files = true');

    console.log('✓ Test 9 passed: Weekly Report preset is visible when cron_jobs=true even if persona_files=false');
  }

  // Test 10: Schedule Type toggle between Interval and Cron Syntax
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'cron-job-modal');
    const title = new MockElement('h2', 'cron-job-modal-title');
    const idInput = new MockElement('input', 'cron-job-id');
    const nameInput = new MockElement('input', 'cron-job-name');
    const promptInput = new MockElement('textarea', 'cron-job-prompt');
    const valueInput = new MockElement('input', 'cron-schedule-value');
    const silentCb = new MockElement('input', 'cron-job-silent');
    const enabledCb = new MockElement('input', 'cron-job-enabled');

    const intervalRadio = new MockElement('input');
    intervalRadio.name = 'cron-schedule-type';
    intervalRadio.value = 'interval';
    intervalRadio.checked = true;

    const cronRadio = new MockElement('input');
    cronRadio.name = 'cron-schedule-type';
    cronRadio.value = 'cron';
    cronRadio.checked = false;

    const intervalChipsContainer = new MockElement('div', 'cron-interval-chips');
    const chip15m = new MockElement('button');
    chip15m.className = 'chip-btn';
    chip15m.dataset.val = '15m';

    const chip30m = new MockElement('button');
    chip30m.className = 'chip-btn active';
    chip30m.dataset.val = '30m';

    const chip1h = new MockElement('button');
    chip1h.className = 'chip-btn';
    chip1h.dataset.val = '1h';

    intervalChipsContainer.children = [chip15m, chip30m, chip1h];

    elements.set('modal', modal);
    elements.set('title', title);
    elements.set('id-input', idInput);
    elements.set('name-input', nameInput);
    elements.set('prompt-input', promptInput);
    elements.set('val-input', valueInput);
    elements.set('silent-cb', silentCb);
    elements.set('enabled-cb', enabledCb);
    elements.set('rad-interval', intervalRadio);
    elements.set('rad-cron', cronRadio);
    elements.set('chips-container', intervalChipsContainer);
    elements.set('chip-15m', chip15m);
    elements.set('chip-30m', chip30m);
    elements.set('chip-1h', chip1h);

    const sandbox = {
      document,
      globalThis: {},
      models: []
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    // Initial show modal for new job
    sandbox.showCronJobModal(null);
    assert.strictEqual(intervalRadio.checked, true);
    assert.strictEqual(intervalChipsContainer.style.display, '');
    assert.strictEqual(valueInput.value, '30m');

    // User switches to Cron Syntax
    sandbox.setCronScheduleType('cron');
    assert.strictEqual(cronRadio.checked, true);
    assert.strictEqual(intervalChipsContainer.style.display, 'none', 'Interval chips must disappear when Cron Syntax is selected');
    assert.strictEqual(valueInput.value, '*/30 * * * *', 'Schedule value should have a cron template');
    assert.ok(valueInput.placeholder.includes('* * * *'), 'Placeholder should indicate cron expression format');

    // User switches back to Interval
    sandbox.setCronScheduleType('interval');
    assert.strictEqual(intervalRadio.checked, true);
    assert.strictEqual(intervalChipsContainer.style.display, '', 'Interval chips must reappear when Interval is selected');
    assert.strictEqual(valueInput.value, '30m', 'Schedule value should convert back to interval format');
    assert.strictEqual(chip30m.classList.contains('active'), true);

    // Clicking an interval chip
    sandbox.selectIntervalChip('15m');
    assert.strictEqual(valueInput.value, '15m');
    assert.strictEqual(chip15m.classList.contains('active'), true);
    assert.strictEqual(intervalChipsContainer.style.display, '');

    // Switching to cron when 15m was selected converts to 15m cron template
    sandbox.setCronScheduleType('cron');
    assert.strictEqual(valueInput.value, '*/15 * * * *');
    assert.strictEqual(intervalChipsContainer.style.display, 'none');

    console.log('✓ Test 10 passed: Schedule type toggle hides/shows interval chips and switches templates appropriately');
  }

  // Test 11: Verify AGENTS.md persona template matches the Rune Notebook workspace guide
  {
    const personaRsContent = fs.readFileSync(path.join(__dirname, '../src/serve/persona.rs'), 'utf8');
    assert.ok(personaRsContent.includes('# AGENTS.md - Your Workspace'), 'AGENTS.md persona template must be the workspace guide');
    assert.ok(personaRsContent.includes('This notebook is home. Treat it that way.'), 'AGENTS.md template must contain notebook home intro');
    assert.ok(personaRsContent.includes('### 🧠 MEMORY.md - Your Long-Term Memory'), 'AGENTS.md template must contain memory guidance');
    assert.ok(!personaRsContent.includes('include_str!("../../AGENTS.md")'), 'AGENTS.md persona template must not be the root repo specification');

    console.log('✓ Test 11 passed: AGENTS.md persona template verified against canonical Rune Notebook guide');
  }

  // Test 12: loadCronJobs and renderCronJobsList render running state and timeout badge
  {
    const { document, elements, MockElement } = createDOM();
    const list = new MockElement('div', 'cron-jobs-list');

    const mockJobs = [
      {
        id: 'job-run-1',
        name: 'Running Task',
        schedule_type: 'interval',
        schedule_value: '15m',
        prompt: 'Check status',
        model: 'deepseek-chat',
        silent_if_no_action: false,
        enabled: true,
        timeout_secs: 120,
      },
      {
        id: 'job-idle-2',
        name: 'Idle Task',
        schedule_type: 'cron',
        schedule_value: '0 0 * * *',
        prompt: 'Nightly backup',
        model: null,
        silent_if_no_action: true,
        enabled: false,
        timeout_secs: 45,
      }
    ];

    const mockApi = async (endpoint) => {
      if (endpoint.includes('/jobs')) {
        return {
          ok: true,
          jobs: mockJobs,
          running_job_ids: ['job-run-1'],
          cron_jobs_enabled: true,
        };
      }
      return { ok: false, error: 'Not found' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      runningCronJobIds: new Set(),
      PERSONA_DESCRIPTIONS: {},
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    await sandbox.loadCronJobs();
    assert.strictEqual(list.children.length, 2, 'Should render 2 job cards');

    const runningCard = list.children[0];
    assert.ok(runningCard.className.includes('running'), 'Running card must have running class');
    const runningBadges = runningCard.children[0].children[0].children; // header -> titleGroup -> badges
    const hasRunningBadge = runningBadges.some(b => b.textContent === '⏳ Running...' && b.className.includes('badge-running'));
    assert.ok(hasRunningBadge, 'Should display ⏳ Running... badge');
    const hasTimeout120 = runningBadges.some(b => b.textContent === 'Timeout: 120s');
    assert.ok(hasTimeout120, 'Should display Timeout: 120s badge');

    // Check Run Now button on running card
    const runBtn = runningCard.children[0].children[1].children.find(c => c.dataset.action === 'run-cron-job');
    assert.ok(runBtn, 'Run button should exist');
    assert.strictEqual(runBtn.disabled, true, 'Run button on running job must be disabled');
    assert.strictEqual(runBtn.textContent, '⏳ Running...', 'Run button text must be ⏳ Running...');

    // Check idle card
    const idleCard = list.children[1];
    assert.ok(idleCard.className.includes('paused'), 'Idle paused card must have paused class');
    const idleBadges = idleCard.children[0].children[0].children;
    const hasTimeout45 = idleBadges.some(b => b.textContent === 'Timeout: 45s');
    assert.ok(hasTimeout45, 'Should display Timeout: 45s badge');
    const idleRunBtn = idleCard.children[0].children[1].children.find(c => c.dataset.action === 'run-cron-job');
    assert.strictEqual(idleRunBtn.disabled, false, 'Run button on idle job must be enabled');
    assert.strictEqual(idleRunBtn.textContent, '▶ Run Now');

    console.log('✓ Test 12 passed: loadCronJobs and renderCronJobsList render running state and timeout badge correctly');
  }

  // Test 13: showCronJobModal populates timeout_secs and saveCronJob sends it in payload
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'cron-job-modal');
    const title = new MockElement('h3', 'cron-job-modal-title');
    const jobId = new MockElement('input', 'cron-job-id');
    const jobName = new MockElement('input', 'cron-job-name');
    const schedVal = new MockElement('input', 'cron-schedule-value');
    const prompt = new MockElement('textarea', 'cron-job-prompt');
    const modelSelect = new MockElement('select', 'cron-job-model');
    const thinkingSelect = new MockElement('select', 'cron-job-thinking');
    const timeoutInput = new MockElement('input', 'cron-job-timeout');
    const silentCheck = new MockElement('input', 'cron-job-silent');
    const enabledCheck = new MockElement('input', 'cron-job-enabled');

    let savedPayload = null;
    const mockApi = async (endpoint, payload) => {
      if (endpoint.includes('/jobs')) {
        savedPayload = payload;
        return { ok: true, job: { id: 'job-123', ...payload } };
      }
      return { ok: false, error: 'Not found' };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      currentCronJobs: [],
      loadCronJobs: () => {},
      PERSONA_DESCRIPTIONS: {},
      availableModels: [
        { id: 'openrouter/auto' },
        { id: 'deepseek/deepseek-chat' },
        { id: 'claude-3-5-sonnet' }
      ],
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    // 1. New job default timeout is 60s and models are populated
    sandbox.showCronJobModal(null);
    assert.strictEqual(timeoutInput.value, '60s', 'Default timeout must be 60s for new job');
    assert.strictEqual(modelSelect.children.length, 4, 'Model select should have 1 inherit option + 3 models');
    assert.strictEqual(modelSelect.children[0].value, '', 'First option is inherit default');
    assert.strictEqual(modelSelect.children[1].value, 'openrouter/auto');
    assert.strictEqual(modelSelect.children[2].value, 'deepseek/deepseek-chat');
    assert.strictEqual(modelSelect.children[3].value, 'claude-3-5-sonnet');

    // 2. Edit existing job with 180s timeout, model, and thinking override
    sandbox.showCronJobModal({
      id: 'job-abc',
      name: 'Custom Timeout Job',
      schedule_type: 'interval',
      schedule_value: '1h',
      prompt: 'Do something',
      model: 'deepseek/deepseek-chat',
      thinking: 'high',
      timeout_secs: 180,
    });
    assert.strictEqual(timeoutInput.value, '180s', 'Timeout input should match job.timeout_secs with s suffix');
    assert.strictEqual(modelSelect.value, 'deepseek/deepseek-chat', 'Model select should match job.model');
    assert.strictEqual(thinkingSelect.value, 'high', 'Thinking select should match job.thinking');

    // 3. Save job with updated model, thinking, and timeout
    modelSelect.value = 'claude-3-5-sonnet';
    thinkingSelect.value = 'xhigh';
    timeoutInput.value = '300';
    await sandbox.saveCronJob();
    assert.ok(savedPayload, 'Payload must be saved');
    assert.strictEqual(savedPayload.model, 'claude-3-5-sonnet', 'Payload must include updated model');
    assert.strictEqual(savedPayload.thinking, 'xhigh', 'Payload must include updated thinking');
    assert.strictEqual(savedPayload.timeout_secs, 300, 'Payload must include timeout_secs: 300');

    console.log('✓ Test 13 passed: showCronJobModal populates models/thinking/timeout_secs and saveCronJob sends them in payload');
  }

  // Test 14: runCronJob prevents duplicate re-entrant triggers and updates running state
  {
    const { document, elements, MockElement } = createDOM();
    const list = new MockElement('div', 'cron-jobs-list');

    let runCallCount = 0;
    const mockApi = async (endpoint) => {
      if (endpoint.includes('/run')) {
        runCallCount++;
        return { ok: true, result: { status: 'success', duration_ms: 150 } };
      }
      return { ok: true, jobs: [] };
    };

    const sandbox = {
      document,
      globalThis: {},
      api: mockApi,
      settingsNoteId: 'my-note',
      runningCronJobIds: new Set(),
      currentCronJobs: [{ id: 'job-1', name: 'Task 1', schedule_type: 'interval', schedule_value: '30m', prompt: 'P', enabled: true }],
      loadCronJobs: () => {},
      alert: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    // Mark job-1 as already running
    sandbox.runningCronJobIds.add('job-1');
    await sandbox.runCronJob('job-1');
    assert.strictEqual(runCallCount, 0, 'runCronJob must not invoke API if job is already in runningCronJobIds');

    // When not running, runCronJob invokes API and clears running state afterwards
    sandbox.runningCronJobIds.clear();
    await sandbox.runCronJob('job-1');
    assert.strictEqual(runCallCount, 1, 'runCronJob should invoke API once');
    assert.strictEqual(sandbox.runningCronJobIds.has('job-1'), false, 'runningCronJobIds should be cleared after execution');

    console.log('✓ Test 14 passed: runCronJob prevents duplicate execution when job is already running');
  }

  // Test 15: SSE cron_job_status event toggles runningCronJobIds and triggers UI re-render
  {
    const sseRaw = fs.readFileSync(path.join(__dirname, '../web/js/sse-events.js'), 'utf8');
    const sseCode = sseRaw.replace(/^import\s+[^;]+;/gm, '');

    let renderCalledCount = 0;
    const sandbox = {
      globalThis: {},
      settingsNoteId: 'note-1',
      runningCronJobIds: new Set(),
      renderCronJobsList: () => {
        renderCalledCount++;
      }
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(sseCode, sandbox);

    // Receive cron_job_status with is_running = true
    sandbox.handleMessage({
      type: 'cron_job_status',
      note_id: 'note-1',
      job_id: 'job-xyz',
      is_running: true,
    });
    assert.strictEqual(sandbox.runningCronJobIds.has('job-xyz'), true, 'job-xyz should be added to runningCronJobIds');
    assert.strictEqual(renderCalledCount, 1, 'renderCronJobsList should be called on job start');

    // Receive cron_job_status with is_running = false
    sandbox.handleMessage({
      type: 'cron_job_status',
      note_id: 'note-1',
      job_id: 'job-xyz',
      is_running: false,
    });
    assert.strictEqual(sandbox.runningCronJobIds.has('job-xyz'), false, 'job-xyz should be removed from runningCronJobIds');
    assert.strictEqual(renderCalledCount, 2, 'renderCronJobsList should be called on job finish');

    console.log('✓ Test 15 passed: SSE cron_job_status event dynamically updates running state');
  }

  // Test 16: saveCronJob and runCronJob do not use window.alert notifications
  {
    const { document, elements, MockElement } = createDOM();
    const modal = new MockElement('div', 'cron-job-modal');
    const formErr = new MockElement('div', 'cron-job-form-error');
    const idInput = new MockElement('input', 'cron-job-id');
    const nameInput = new MockElement('input', 'cron-job-name');
    const promptInput = new MockElement('textarea', 'cron-job-prompt');
    const schedValInput = new MockElement('input', 'cron-schedule-value');
    const timeoutInput = new MockElement('input', 'cron-job-timeout');
    const silentCheck = new MockElement('input', 'cron-job-silent');
    const enabledCheck = new MockElement('input', 'cron-job-enabled');

    elements.set('cron-job-modal', modal);
    elements.set('cron-job-form-error', formErr);
    elements.set('cron-job-id', idInput);
    elements.set('cron-job-name', nameInput);
    elements.set('cron-job-prompt', promptInput);
    elements.set('cron-schedule-value', schedValInput);
    elements.set('cron-job-timeout', timeoutInput);
    elements.set('cron-job-silent', silentCheck);
    elements.set('cron-job-enabled', enabledCheck);

    let alertCalled = false;
    let systemMessages = [];

    const sandbox = {
      document,
      globalThis: {},
      api: async () => ({ ok: true, result: { status: 'success', duration_ms: 100 } }),
      settingsNoteId: 'my-note',
      runningCronJobIds: new Set(),
      currentCronJobs: [],
      loadCronJobs: () => {},
      addSystemMessage: (msg) => { systemMessages.push(msg); },
      alert: () => { alertCalled = true; }
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    // Empty name should set formErr and NOT call window.alert
    nameInput.value = '';
    await sandbox.saveCronJob();
    assert.strictEqual(alertCalled, false, 'saveCronJob must not invoke window.alert on missing name');
    assert.strictEqual(formErr.textContent, 'Please enter a job name.');
    assert.strictEqual(formErr.classList.contains('hidden'), false);

    // Run job should NOT call window.alert
    await sandbox.runCronJob('job-1');
    assert.strictEqual(alertCalled, false, 'runCronJob must not invoke window.alert on completion');

    console.log('✓ Test 16 passed: verified window.alert is completely removed in favor of inline error elements');
  }

  // Test 17: Static codebase audit forbidding window.alert, alert(), window.confirm, window.prompt across web/js/
  {
    const jsDir = path.join(__dirname, '../web/js');
    const files = fs.readdirSync(jsDir).filter(f => f.endsWith('.js'));
    const forbiddenPatterns = [
      /\bwindow\.alert\s*\(/,
      /(?<!\w)alert\s*\(/,
      /\bwindow\.confirm\s*\(/,
      /\bwindow\.prompt\s*\(/,
    ];

    for (const file of files) {
      const content = fs.readFileSync(path.join(jsDir, file), 'utf8');
      const lines = content.split('\n');
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        // Skip comment lines and dom.js override block
        if (line.trim().startsWith('//') || line.trim().startsWith('*') || file === 'dom.js') continue;
        for (const pattern of forbiddenPatterns) {
          assert.ok(
            !pattern.test(line),
            `Forbidden dialog call matching ${pattern} found in web/js/${file}:${i + 1} -> "${line.trim()}"`
          );
        }
      }
    }

    console.log('✓ Test 17 passed: static codebase audit verified no window.alert or blocking dialogs in web/js/');
  }

  // Test 18: Instruction Prompt supports + for skills and @ for files autocomplete
  {
    const { document, elements, MockElement } = createDOM();
    const promptEl = new MockElement('textarea', 'cron-job-prompt');
    const autoEl = new MockElement('div', 'cron-prompt-autocomplete');
    const chatInput = new MockElement('textarea', 'chat-input');
    const chatAuto = new MockElement('div', 'chat-autocomplete');

    const autoRaw = fs.readFileSync(path.join(__dirname, '../web/js/autocomplete.js'), 'utf8');
    const autoCode = autoRaw.replace(/^import\s+[^;]+;/gm, '').replace(/^export\s+/gm, '');

    const mockSkills = [
      { name: 'code-review', description: 'Perform code review' },
      { name: 'summarize', description: 'Summarize text' }
    ];

    const mockApi = async (endpoint) => {
      if (endpoint === 'skills') {
        return { ok: true, data: { skills: mockSkills } };
      }
      return { ok: false };
    };

    const sandbox = {
      document,
      globalThis: {
        fileList: ['HEARTBEAT.md', 'TODO.md', 'MEMORY.md']
      },
      api: mockApi,
      console
    };
    sandbox.globalThis.document = document;

    vm.createContext(sandbox);
    vm.runInContext(autoCode, sandbox);

    await sandbox.fetchSkills();
    sandbox.initAutocomplete();

    // 1. Test typing @ in cron-job-prompt triggers file suggestions
    promptEl.value = 'Inspect @HEART';
    promptEl.selectionStart = promptEl.value.length;
    promptEl.selectionEnd = promptEl.value.length;
    sandbox.updateAutocomplete(promptEl, autoEl);

    assert.strictEqual(autoEl.classList.contains('hidden'), false, 'Autocomplete popup should be open for @');
    assert.strictEqual(autoEl.children.length, 1, 'Should find 1 matching file HEARTBEAT.md');
    const fileNameEl = autoEl.children[0].children.find(c => c.className.includes('chat-autocomplete-name'));
    assert.strictEqual(fileNameEl.textContent, 'HEARTBEAT.md');

    // Apply item via enter key
    sandbox.handleAutocompleteKeydown({
      key: 'Enter',
      preventDefault: () => {},
      stopPropagation: () => {}
    });
    assert.strictEqual(promptEl.value, 'Inspect @HEARTBEAT.md ', 'Selected file should be inserted with trailing space');
    assert.strictEqual(autoEl.classList.contains('hidden'), true, 'Autocomplete popup should close after selection');

    // 2. Test typing + in cron-job-prompt triggers skill suggestions
    promptEl.value = 'Please run +code';
    promptEl.selectionStart = promptEl.value.length;
    promptEl.selectionEnd = promptEl.value.length;
    sandbox.updateAutocomplete(promptEl, autoEl);

    assert.strictEqual(autoEl.classList.contains('hidden'), false, 'Autocomplete popup should be open for +');
    assert.strictEqual(autoEl.children.length, 1, 'Should find 1 matching skill code-review');
    const skillNameEl = autoEl.children[0].children.find(c => c.className.includes('chat-autocomplete-name'));
    assert.strictEqual(skillNameEl.textContent, 'code-review');

    // Apply skill item
    sandbox.applyItem(0);
    assert.strictEqual(promptEl.value, 'Please run +code-review ', 'Selected skill should be inserted with trailing space');

    console.log('✓ Test 18 passed: Instruction Prompt supports + for skills and @ for files autocomplete');
  }

  // Test 19: formatDurationMs, parseDurationSeconds, and Execution Timeout label & defaults
  {
    const { document, elements, MockElement } = createDOM();
    const chatRaw = fs.readFileSync(path.join(__dirname, '../web/js/chat.js'), 'utf8');
    const chatCode = chatRaw.replace(/^import\s+[^;]+;/gm, '').replace(/^export\s+/gm, '');
    const indexHtml = fs.readFileSync(path.join(__dirname, '../web/index.html'), 'utf8');

    const sandbox = {
      document,
      globalThis: {},
      console
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(chatCode, sandbox);
    vm.runInContext(settingsCode, sandbox);

    // 1. Verify formatDurationMs
    assert.strictEqual(sandbox.formatDurationMs(104578), '1m45s', '104578ms should format to 1m45s');
    assert.strictEqual(sandbox.formatDurationMs(80123), '1m20s', '80123ms should format to 1m20s');
    assert.strictEqual(sandbox.formatDurationMs(450), '0s', '450ms should round to 0s');
    assert.strictEqual(sandbox.formatDurationMs(600), '1s', '600ms should round to 1s');
    assert.strictEqual(sandbox.formatDurationMs(12345), '12s', '12345ms should format to 12s');
    assert.strictEqual(sandbox.formatDurationMs(60000), '1m', '60000ms should format to 1m');
    assert.strictEqual(sandbox.formatDurationMs(0), '0s', '0ms should format to 0s');

    // 2. Verify parseDurationSeconds
    assert.strictEqual(sandbox.parseDurationSeconds('60s'), 60, '60s should parse to 60');
    assert.strictEqual(sandbox.parseDurationSeconds('2m'), 120, '2m should parse to 120');
    assert.strictEqual(sandbox.parseDurationSeconds('1m 30s'), 90, '1m 30s should parse to 90');
    assert.strictEqual(sandbox.parseDurationSeconds('1m 10s 100ms'), 70, '1m 10s 100ms should parse to 70s');
    assert.strictEqual(sandbox.parseDurationSeconds('1m 20s 123ms'), 80, '1m 20s 123ms should parse to 80s');
    assert.strictEqual(sandbox.parseDurationSeconds('120'), 120, '120 should parse to 120');

    // 3. Verify index.html label and default value
    assert.ok(indexHtml.includes('<label for="cron-job-timeout">Execution Timeout *</label>'), 'Label must be Execution Timeout *');
    assert.ok(indexHtml.includes('id="cron-job-timeout" value="60s"'), 'Input must have default value 60s');

    // 4. Verify showCronJobModal sets default 60s
    const modal = new MockElement('div', 'cron-job-modal');
    const timeoutInput = new MockElement('input', 'cron-job-timeout');
    const promptInput = new MockElement('textarea', 'cron-job-prompt');
    const nameInput = new MockElement('input', 'cron-job-name');
    const idInput = new MockElement('input', 'cron-job-id');
    const titleEl = new MockElement('h2', 'cron-job-modal-title');
    const silentCheck = new MockElement('input', 'cron-job-silent');
    const enabledCheck = new MockElement('input', 'cron-job-enabled');

    sandbox.showCronJobModal(null);
    assert.strictEqual(timeoutInput.value, '60s', 'Default timeout in showCronJobModal should be 60s');

    sandbox.showCronJobModal({ timeout_secs: 120 });
    assert.strictEqual(timeoutInput.value, '120s', 'Existing job timeout_secs should display with suffix s');

    console.log('✓ Test 19 passed: Duration formatting and Execution Timeout defaults verified');
  }

  // Test 20: Note Settings modal close button, cancel button removal, and delete button hiding on Persona/Cron tabs
  {
    const { document, elements, MockElement } = createDOM();
    const indexHtml = fs.readFileSync(path.join(__dirname, '../web/index.html'), 'utf8');

    // 1. Verify HTML template
    assert.ok(indexHtml.includes('<button type="button" class="modal-close-btn" data-action="hide-note-settings" data-icon="x"'), 'Note settings must have top-right close button with x icon');
    assert.ok(!indexHtml.includes('<button class="btn-secondary" data-action="hide-note-settings">Cancel</button>'), 'Cancel button must be removed from note settings modal actions');

    // 2. Verify JS tab switching and Delete button visibility
    const modal = new MockElement('div', 'note-settings-modal');
    const title = new MockElement('h2', 'note-settings-title');
    const nameInput = new MockElement('input', 'note-settings-name');
    const deleteBtn = new MockElement('button', 'btn-delete-note');
    const saveBtn = new MockElement('button', 'btn-save-note-settings');

    const generalPanel = new MockElement('div', 'settings-tab-general');
    generalPanel.className = 'settings-tab-panel';
    const personaPanel = new MockElement('div', 'settings-tab-persona');
    personaPanel.className = 'settings-tab-panel hidden';
    const cronPanel = new MockElement('div', 'settings-tab-cron');
    cronPanel.className = 'settings-tab-panel hidden';

    const modalActions = new MockElement('div');
    modalActions.className = 'modal-actions';
    modal.children.push(modalActions);

    const generalTab = new MockElement('button');
    generalTab.className = 'settings-tab active';
    generalTab.dataset.tab = 'general';

    const personaTab = new MockElement('button');
    personaTab.className = 'settings-tab';
    personaTab.dataset.tab = 'persona';

    const cronTab = new MockElement('button');
    cronTab.className = 'settings-tab';
    cronTab.dataset.tab = 'cron';

    const sandbox = {
      document,
      globalThis: {},
      notes: [{ id: 'custom-note', name: 'Custom Note' }, { id: 'default', name: 'Default Note' }],
      settingsNoteId: null,
      selectedNoteIcon: null,
      renderNoteIconTrigger: () => {},
      markSelectedEmoji: () => {},
      closeEmojiPicker: () => {},
      loadPersonaStatus: () => {},
      loadCronJobs: () => {}
    };
    sandbox.globalThis = sandbox;

    vm.createContext(sandbox);
    vm.runInContext(settingsCode, sandbox);

    // Open custom-note: initially on General tab
    sandbox.showNoteSettings('custom-note');
    assert.strictEqual(deleteBtn.style.display, '', 'Delete button should be visible on General tab for non-default notes');
    assert.strictEqual(saveBtn.style.display, '', 'Save button should be visible on General tab');

    // Switch to Persona Files tab
    sandbox.switchSettingsTab('persona');
    assert.strictEqual(deleteBtn.style.display, 'none', 'Delete button must be hidden on Persona Files tab');
    assert.strictEqual(saveBtn.style.display, 'none', 'Save button must be hidden on Persona Files tab');
    assert.strictEqual(modalActions.style.display, 'none', 'Modal actions must be hidden on Persona Files tab');

    // Switch to Scheduled Jobs (Cron) tab
    sandbox.switchSettingsTab('cron');
    assert.strictEqual(deleteBtn.style.display, 'none', 'Delete button must be hidden on Cron tab');
    assert.strictEqual(saveBtn.style.display, 'none', 'Save button must be hidden on Cron tab');
    assert.strictEqual(modalActions.style.display, 'none', 'Modal actions must be hidden on Cron tab');

    // Switch back to General tab
    sandbox.switchSettingsTab('general');
    assert.strictEqual(deleteBtn.style.display, '', 'Delete button must be restored when switching back to General tab');
    assert.strictEqual(saveBtn.style.display, '', 'Save button must be restored when switching back to General tab');
    assert.strictEqual(modalActions.style.display, '', 'Modal actions must be restored when switching back to General tab');

    // Open default note: Delete button should be hidden even on General tab
    sandbox.showNoteSettings('default');
    assert.strictEqual(deleteBtn.style.display, 'none', 'Delete button must always be hidden for default note');

    console.log('✓ Test 20 passed: Note Settings Delete button hidden on Persona/Cron and top-right close icon verified');
  }

  console.log('All Settings Persona & Cron Jobs tests passed successfully! 🎉');
}

runTests().catch(err => {
  console.error(err);
  process.exit(1);
});


