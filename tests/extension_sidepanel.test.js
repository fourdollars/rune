const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

console.log("=== Browser Extension Sidepanel Test Suite ===");

function createMockElement(id = '', tag = 'div') {
  const listeners = {};
  const classes = new Set();
  const children = [];
  const attributes = {};

  return {
    id,
    tagName: tag.toUpperCase(),
    textContent: '',
    title: '',
    value: '',
    hidden: false,
    disabled: false,
    innerHTML: '',
    scrollTop: 0,
    scrollHeight: 100,
    style: {},
    classList: {
      add: (...c) => c.forEach((cls) => classes.add(cls)),
      remove: (...c) => c.forEach((cls) => classes.delete(cls)),
      contains: (cls) => classes.has(cls),
    },
    children,
    appendChild: (child) => {
      children.push(child);
      return child;
    },
    replaceChildren: (...newChildren) => {
      children.length = 0;
      children.push(...newChildren);
    },
    setAttribute: (k, v) => { attributes[k] = String(v); },
    getAttribute: (k) => attributes[k] ?? null,
    removeAttribute: (k) => { delete attributes[k]; },
    focus: () => {},
    blur: () => {},
    addEventListener: (evt, fn) => {
      if (!listeners[evt]) listeners[evt] = [];
      listeners[evt].push(fn);
    },
    removeEventListener: (evt, fn) => {
      if (listeners[evt]) {
        listeners[evt] = listeners[evt].filter((f) => f !== fn);
      }
    },
    dispatchEvent: (evt) => {
      const fns = listeners[evt.type || evt] || [];
      fns.forEach((fn) => fn(evt));
    },
    querySelector: () => createMockElement(),
    querySelectorAll: () => [],
    _trigger: (evt, data = {}) => {
      const fns = listeners[evt] || [];
      const eventObj = {
        type: evt,
        stopPropagation: () => {},
        preventDefault: () => {},
        target: null,
        ...data,
      };
      fns.forEach((fn) => fn(eventObj));
    },
    _listeners: listeners,
  };
}

function setupSidepanelContext() {
  const elements = {};
  const getEl = (id) => {
    if (!elements[id]) {
      elements[id] = createMockElement(id);
    }
    return elements[id];
  };

  const docListeners = {};
  const documentMock = {
    getElementById: (id) => getEl(id),
    createElement: (tag) => createMockElement('', tag),
    createTextNode: (txt) => ({ nodeType: 3, textContent: txt }),
    createDocumentFragment: () => {
      const frag = createMockElement('frag', 'div');
      frag.childNodes = [];
      return frag;
    },
    addEventListener: (evt, fn) => {
      if (!docListeners[evt]) docListeners[evt] = [];
      docListeners[evt].push(fn);
    },
    removeEventListener: (evt, fn) => {
      if (docListeners[evt]) {
        docListeners[evt] = docListeners[evt].filter((f) => f !== fn);
      }
    },
    body: createMockElement('body', 'body'),
  };

  const storageData = {};
  const sentMessages = [];
  let sseStreams = [];

  const browserMock = {
    storage: {
      local: {
        get: async (keys) => {
          if (typeof keys === 'string') {
            return { [keys]: storageData[keys] };
          }
          if (Array.isArray(keys)) {
            const res = {};
            keys.forEach((k) => { res[k] = storageData[k]; });
            return res;
          }
          return { ...storageData };
        },
        set: async (obj) => {
          Object.assign(storageData, obj);
        },
        remove: async (key) => {
          if (Array.isArray(key)) {
            key.forEach((k) => delete storageData[k]);
          } else {
            delete storageData[key];
          }
        },
      },
      sync: {
        get: async () => ({ serverUrl: 'http://localhost:9527' }),
        set: async () => {},
      },
    },
    runtime: {
      sendMessage: async (msg) => {
        sentMessages.push(msg);
        if (msg.type === 'rune:loadSession') {
          return {
            ok: true,
            data: {
              current_model: 'gemini-2.5-flash',
              history: msg.mockHistory ?? [],
            },
          };
        }
        return { ok: true };
      },
    },
    tabs: {
      query: async () => [{ id: 1, url: 'https://example.com' }],
      sendMessage: async () => null,
    },
  };

  const fetchCalls = [];
  const fetchMock = async (url, options = {}) => {
    fetchCalls.push({ url, options });
    if (url.includes('/api/events')) {
      const stream = {
        aborted: false,
        controller: null,
      };
      sseStreams.push(stream);
      return {
        ok: true,
        status: 200,
        body: {
          getReader: () => ({
            read: async () => ({ done: true, value: new Uint8Array() }),
          }),
        },
      };
    }
    if (url.includes('/api/notes')) {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          ok: true,
          notes: [
            { id: 'note-1', name: 'Note 1' },
            { id: 'note-2', name: 'Note 2' },
          ],
        }),
      };
    }
    return {
      ok: true,
      status: 200,
      json: async () => ({ ok: true }),
    };
  };

  const markedMock = {
    parse: (txt) => `<p>${txt}</p>`,
    Renderer: class {},
    setOptions: () => {},
    use: () => {},
  };

  const rawCode = fs.readFileSync(
    path.join(__dirname, '../browser-extension/src/sidepanel.js'),
    'utf8'
  );
  // Replace ES module import with mocked declarations for node VM
  const sidepanelCode = rawCode.replace(
    /import\s+\{([^}]+)\}\s+from\s+['"][^'"]+['"];/,
    '// import mocked'
  );

  const DOMParserMock = class {
    parseFromString(str) {
      let node = createMockElement('p', 'p');
      node.innerHTML = str;
      return {
        body: {
          get firstChild() {
            const n = node;
            node = null;
            return n;
          },
        },
      };
    }
  };

  const sandbox = {
    console,
    document: documentMock,
    window: { addEventListener: () => {} },
    navigator: { userAgent: 'Mozilla/5.0 NodeTest' },
    browser: browserMock,
    fetch: fetchMock,
    marked: markedMock,
    DOMParser: DOMParserMock,
    getSyncSettings: async () => ({ serverUrl: 'http://localhost:9527' }),
    getLocalAuth: async () => ({ accessToken: 'test-token' }),
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    AbortController,
    TextDecoder,
    Uint8Array,
    Date,
    JSON,
    Array,
    Object,
    String,
    Math,
    Boolean,
  };

  const script = new vm.Script(sidepanelCode);
  const context = vm.createContext(sandbox);
  script.runInContext(context);

  return {
    context,
    elements,
    storageData,
    sentMessages,
    fetchCalls,
    exec: (code) => vm.runInContext(code, context),
    getActiveNoteId: () => vm.runInContext('activeNoteId', context),
    setActiveNoteId: (id) => vm.runInContext(`activeNoteId = ${JSON.stringify(id)}`, context),
    getCachedNotes: () => vm.runInContext('cachedNotes', context),
    setCachedNotes: (notes) => vm.runInContext(`cachedNotes = ${JSON.stringify(notes)}`, context),
    getHistoryLoaded: () => vm.runInContext('historyLoaded', context),
  };
}

async function runTests() {
  // ── Test 1: switchToNote clears messages and triggers loadNoteHistory ─────────
  {
    const fixture = setupSidepanelContext();
    const $messages = fixture.elements['messages'];
    $messages.innerHTML = '<div>Old message from note 1</div>';

    fixture.setActiveNoteId('note-1');
    fixture.exec('switchToNote("note-2")');

    assert.strictEqual(
      $messages.innerHTML,
      '',
      'switchToNote should immediately clear $messages.innerHTML'
    );
    assert.strictEqual(
      fixture.getActiveNoteId(),
      'note-2',
      'switchToNote should update activeNoteId to note-2'
    );

    await new Promise((r) => setTimeout(r, 20));

    const loadSessionCall = fixture.sentMessages.find(
      (m) => m.type === 'rune:loadSession' && m.noteId === 'note-2'
    );
    assert.ok(
      loadSessionCall,
      'switchToNote should send rune:loadSession for the new note'
    );
    console.log('✓ Test 1 passed: switchToNote clears messages and requests history');
  }

  // ── Test 2: Dropdown item click executes switchToNote ──────────────────────────
  {
    const fixture = setupSidepanelContext();
    const $messages = fixture.elements['messages'];
    $messages.innerHTML = '<div>Old message from note 1</div>';

    fixture.setCachedNotes([
      { id: 'note-1', name: 'Note 1' },
      { id: 'note-2', name: 'Note 2' },
    ]);
    fixture.setActiveNoteId('note-1');
    fixture.exec('renderNoteDropdown()');

    const $noteDropdown = fixture.elements['note-dropdown'];
    assert.strictEqual(
      $noteDropdown.children.length,
      2,
      'Dropdown should have 2 note items rendered'
    );

    const note2Item = $noteDropdown.children[1];
    note2Item._trigger('click');

    assert.strictEqual(
      fixture.getActiveNoteId(),
      'note-2',
      'Clicking note-2 should switch activeNoteId to note-2'
    );
    assert.strictEqual(
      $messages.innerHTML,
      '',
      'Clicking note-2 should have cleared the message area'
    );
    console.log('✓ Test 2 passed: Dropdown item click triggers switchToNote without premature return');
  }

  // ── Test 3: loadNoteHistory replaying messages into $messages ──────────────────
  {
    const fixture = setupSidepanelContext();
    const $messages = fixture.elements['messages'];

    fixture.context.browser.runtime.sendMessage = async (msg) => {
      if (msg.type === 'rune:loadSession') {
        return {
          ok: true,
          data: {
            current_model: 'gemini-2.5-flash',
            history: [
              { role: 'user', content: 'Hello from note 2!' },
              { role: 'assistant', content: 'Hi there!' },
            ],
          },
        };
      }
      return { ok: true };
    };

    fixture.setActiveNoteId('note-2');
    await fixture.exec('loadNoteHistory("note-2")');

    assert.ok(
      $messages.children.length >= 2,
      `Should have rendered 2 messages, got ${$messages.children.length}`
    );
    console.log('✓ Test 3 passed: loadNoteHistory successfully replayed history into DOM');
  }

  // ── Test 4: loadNoteHistory empties DOM when history is empty ──────────────────
  {
    const fixture = setupSidepanelContext();
    const $messages = fixture.elements['messages'];
    $messages.appendChild(createMockElement('old', 'div'));

    fixture.context.browser.runtime.sendMessage = async () => ({
      ok: true,
      data: { current_model: 'gemini-2.5-flash', history: [] },
    });

    fixture.setActiveNoteId('note-empty');
    await fixture.exec('loadNoteHistory("note-empty")');

    assert.strictEqual(
      $messages.innerHTML,
      '',
      'Empty history from server should ensure $messages is completely cleared'
    );
    console.log('✓ Test 4 passed: loadNoteHistory clears messages when server history is empty');
  }

  // ── Test 5: Stale loadNoteHistory request does not overwrite new note ─────────
  {
    const fixture = setupSidepanelContext();
    const $messages = fixture.elements['messages'];

    let resolveOld;
    const oldPromise = new Promise((r) => { resolveOld = r; });

    fixture.context.browser.runtime.sendMessage = async (msg) => {
      if (msg.noteId === 'note-slow') {
        await oldPromise;
        return {
          ok: true,
          data: {
            current_model: 'm1',
            history: [{ role: 'user', content: 'Stale slow content' }],
          },
        };
      }
      return { ok: true, data: { current_model: 'm1', history: [] } };
    };

    fixture.setActiveNoteId('note-slow');
    const slowPromise = fixture.exec('loadNoteHistory("note-slow")');

    // User immediately switches to note-fast
    fixture.setActiveNoteId('note-fast');

    resolveOld();
    await slowPromise;

    assert.strictEqual(
      $messages.children.length,
      0,
      'Stale response from previous note should be ignored and not rendered'
    );
    console.log('✓ Test 5 passed: Stale history responses ignored when user switches notes');
  }

  // ── Test 6: Note recovery on Note not found SSE error ──────────────────────────
  {
    const fixture = setupSidepanelContext();
    let switchedTarget = null;
    fixture.context.switchToNote = (id) => {
      switchedTarget = id;
    };

    fixture.setActiveNoteId('non-existent-note');
    fixture.exec(`handleSseEvent({
      event: 'error',
      data: JSON.stringify({ message: 'Note not found' })
    })`);

    await new Promise((r) => setTimeout(r, 40));

    assert.strictEqual(
      switchedTarget,
      'note-1',
      'Should auto-recover and switch to first available note (note-1)'
    );
    console.log('✓ Test 6 passed: SSE "Note not found" error triggers auto-recovery to available note');
  }

  // ── Test 7: Server note_switched broadcast switches note ───────────────────────
  {
    const fixture = setupSidepanelContext();
    let switchedTarget = null;
    fixture.context.switchToNote = (id) => {
      switchedTarget = id;
    };

    fixture.setActiveNoteId('note-1');
    fixture.exec(`handleSseEvent({
      event: 'note_switched',
      data: JSON.stringify({ note_id: 'note-2' })
    })`);

    assert.strictEqual(
      switchedTarget,
      'note-2',
      'Server note_switched broadcast should trigger switchToNote for the new note'
    );
    console.log('✓ Test 7 passed: Server note_switched broadcast triggers switchToNote');
  }

  // ── Test 8: Startup with non-existent note recovers to first available note ────
  {
    const fixture = setupSidepanelContext();
    fixture.storageData['lastSelectedNote'] = 'deleted-old-note';

    const targetNote = await fixture.exec('fetchNoteListAndRecover("deleted-old-note")');
    assert.strictEqual(
      targetNote,
      'note-1',
      'fetchNoteListAndRecover should fallback to first available note when lastSelectedNote is missing'
    );
    console.log('✓ Test 8 passed: Startup note recovery resolves to first available note');
  }

  // ── Test 9: Provider info in Switch Model modal title and tooltip ──────────────
  {
    const fixture = setupSidepanelContext();
    const $modelModalTitle = fixture.elements['model-modal-title'];
    const $modelName = fixture.elements['model-name'];

    // Test GitHub Copilot
    fixture.exec(`
      availableModels = [
        { id: 'claude-3.5-sonnet', provider: 'github-copilot', context_window: 200000 },
        { id: 'gpt-4o', provider: 'github-copilot', context_window: 128000 }
      ];
      activeModel = 'claude-3.5-sonnet';
      updateModelIndicator();
      showModelDialog();
    `);
    assert.strictEqual(
      $modelModalTitle.textContent,
      'Switch Model (GitHub Copilot)',
      'Modal title should show Switch Model (GitHub Copilot)'
    );
    assert.ok(
      $modelName.title.includes('GitHub Copilot'),
      'Model name tooltip should mention GitHub Copilot'
    );

    // Test OpenRouter w/ ZDR
    fixture.exec(`
      availableModels = [
        { id: 'anthropic/claude-3.5-sonnet', provider: 'openrouter-zdr' }
      ];
      activeModel = 'anthropic/claude-3.5-sonnet';
      updateModelIndicator();
      showModelDialog();
    `);
    assert.strictEqual(
      $modelModalTitle.textContent,
      'Switch Model (OpenRouter w/ ZDR)',
      'Modal title should show Switch Model (OpenRouter w/ ZDR)'
    );

    // Test Google Gemini
    fixture.exec(`
      availableModels = [
        { id: 'gemini-2.5-pro', provider: 'gemini' }
      ];
      activeModel = 'gemini-2.5-pro';
      updateModelIndicator();
      showModelDialog();
    `);
    assert.strictEqual(
      $modelModalTitle.textContent,
      'Switch Model (Google Gemini)',
      'Modal title should show Switch Model (Google Gemini)'
    );
    console.log('✓ Test 9 passed: Switch Model modal title and tooltip display LLM provider info');
  }

  // ─── Test 10: auth_result updates username and renders sender labels ─────
  {
    const fixture = setupSidepanelContext();
    const { elements } = fixture;

    // Simulate auth_result SSE event
    fixture.exec(`
      handleSseEvent({
        event: 'auth_result',
        data: JSON.stringify({ ok: true, login: 'alice', is_admin: false, is_guest: false })
      });
    `);

    // Replay history with messages from 'alice' and 'bob'
    fixture.exec(`
      replayHistory([
        { id: 1, role: 'user', nickname: 'alice', content: 'Hello' },
        { id: 2, role: 'user', nickname: 'bob', content: 'Hi alice' },
        { id: 3, role: 'assistant', nickname: '', content: 'Welcome' }
      ]);
    `);

    const $messages = elements['messages'];
    assert.strictEqual($messages.children.length, 3);
    const aliceSender = $messages.children[0].children[0].children[0].textContent;
    const bobSender = $messages.children[1].children[0].children[0].textContent;
    const assistantSender = $messages.children[2].children[0].children[0].textContent;

    assert.strictEqual(aliceSender, 'alice (you)', 'User own message should be labeled alice (you)');
    assert.strictEqual(bobSender, 'bob', 'Other user message should be labeled bob');
    assert.strictEqual(assistantSender, 'ᚱ', 'Assistant message should be labeled ᚱ');

    console.log('✓ Test 10 passed: auth_result updates username and renders sender labels');
  }

  console.log("All extension sidepanel tests passed successfully! 🎉");
}

runTests().catch((err) => {
  console.error("Test failure:", err);
  process.exit(1);
});
