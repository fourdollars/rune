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
      this._value = '';
      this.disabled = false;
      this.checked = false;
      if (id) elements.set(id, this);
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
    set value(val) { this._value = String(val); }
    get innerHTML() { return this._textContent; }
    set innerHTML(val) {
      this.children = [];
      this._textContent = String(val);
    }
    setAttribute(k, v) { this.attributes.set(k, String(v)); }
    getAttribute(k) { return this.attributes.get(k) || null; }
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
    getElementsByName(name) {
      return Array.from(elements.values()).filter(e => e.name === name || e.getAttribute('name') === name);
    },
    querySelector(sel) {
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
        output_snippet: 'HEARTBEAT_OK'
      },
      {
        timestamp: '2026-09-21T15:00:00Z',
        trigger: 'manual',
        status: 'success',
        duration_ms: 680,
        files_modified: ['status.md'],
        output_snippet: 'Updated status'
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
    assert.strictEqual(tbody.children.length, 2, 'Table should contain 2 log rows');
    assert.ok(stats.innerHTML.includes('Total Runs: <strong>2</strong>'), 'Stats should show total runs');
    assert.ok(stats.innerHTML.includes('Silent Skips: <strong class="badge-silent">1</strong>'), 'Stats should show silent count');
    assert.ok(stats.innerHTML.includes('Success: <strong class="badge-present">2</strong>'), 'Stats should show success count');

    console.log('✓ Test 4 passed: viewCronLogs processed execution history and summary statistics');
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

  console.log('All Settings Persona & Cron Jobs tests passed successfully! 🎉');
}

runTests().catch(err => {
  console.error(err);
  process.exit(1);
});
