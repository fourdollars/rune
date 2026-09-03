// Unit test for Rune Notes online users counter and sorted popover list
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
    toggle(cls) {
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
      this.classList = new MockClassList();
      this.children = [];
      this._textContent = '';
      if (id) elements.set(id, this);
    }
    get className() { return Array.from(this.classList.classes).join(' '); }
    set className(val) {
      this.classList.classes.clear();
      if (val) val.split(/\s+/).filter(Boolean).forEach(c => this.classList.add(c));
    }
    get textContent() { return this._textContent; }
    set textContent(val) { this._textContent = String(val); }
    get innerHTML() { return ''; }
    set innerHTML(val) {
      this.children = [];
      this._textContent = '';
    }
    setAttribute(k, v) { this.attributes.set(k, String(v)); }
    getAttribute(k) { return this.attributes.get(k) || null; }
    appendChild(child) {
      this.children.push(child);
      return child;
    }
  }

  const document = {
    getElementById(id) {
      return elements.get(id) || null;
    },
    createElement(tag) {
      return new MockElement(tag);
    },
    addEventListener() {}
  };

  // Pre-populate elements matching index.html
  const onlineCount = new MockElement('span', 'online-count');
  onlineCount.textContent = '0';

  const popoverCount = new MockElement('span', 'online-users-popover-count');
  popoverCount.textContent = '0';

  const popover = new MockElement('div', 'online-users-popover');
  popover.classList.add('hidden');

  const btn = new MockElement('button', 'btn-online-users');
  btn.setAttribute('aria-expanded', 'false');

  const list = new MockElement('ul', 'online-users-list');

  return { document, elements };
}

async function runTests() {
  console.log('=== Rune Notes Online Users Test Suite ===');

  const { document, elements } = createDOM();
  const context = {
    document,
    globalThis: {},
    myNickname: 'alice',
    console
  };
  vm.createContext(context);

  // Read chat-history.js
  const chatHistoryJs = fs.readFileSync(path.join(__dirname, '../web/js/chat-history.js'), 'utf8');
  // Strip import statements for vm runner
  const sanitizedJs = chatHistoryJs.replace(/^import\s+[^;]+;/gm, '');
  vm.runInContext(sanitizedJs, context);

  // Test 1: updateOnlineCount updates counts
  {
    context.globalThis.updateOnlineCount(3, ['charlie', 'alice', 'bob']);
    const countEl = document.getElementById('online-count');
    const popoverCountEl = document.getElementById('online-users-popover-count');
    assert.strictEqual(countEl.textContent, '3', 'Badge count should be 3');
    assert.strictEqual(popoverCountEl.textContent, '3', 'Popover count should be 3');
    console.log('✓ Test 1 passed: updateOnlineCount updates badge and header counts');
  }

  // Test 2: Users are sorted in alphabetical order and current user tagged with (you)
  {
    const listEl = document.getElementById('online-users-list');
    assert.strictEqual(listEl.children.length, 3, 'Should render 3 list items');

    const names = listEl.children.map(li => {
      const nameSpan = li.children.find(c => c.classList.contains('online-user-name'));
      return nameSpan ? nameSpan.textContent : '';
    });

    assert.deepStrictEqual(names, ['alice (you)', 'bob', 'charlie'], 'Users should be sorted alphabetically with current user tagged (you)');
    assert.ok(listEl.children[0].classList.contains('me'), 'First item (alice) should have .me class');
    assert.ok(!listEl.children[1].classList.contains('me'), 'Second item (bob) should not have .me class');
    console.log('✓ Test 2 passed: Users rendered in alphabetical order with (you) tag');
  }

  // Test 3: Empty user list renders fallback
  {
    context.globalThis.updateOnlineCount(0, []);
    const listEl = document.getElementById('online-users-list');
    assert.strictEqual(listEl.children.length, 1);
    assert.strictEqual(listEl.children[0].textContent, 'No users online');
    console.log('✓ Test 3 passed: Empty user list renders fallback message');
  }

  // Test 4: toggleOnlineUsers toggles popover visibility and aria-expanded
  {
    const popover = document.getElementById('online-users-popover');
    const btn = document.getElementById('btn-online-users');

    assert.ok(popover.classList.contains('hidden'));
    assert.strictEqual(btn.getAttribute('aria-expanded'), 'false');

    context.globalThis.toggleOnlineUsers();
    assert.ok(!popover.classList.contains('hidden'), 'Popover should not have .hidden after toggle');
    assert.strictEqual(btn.getAttribute('aria-expanded'), 'true');

    context.globalThis.toggleOnlineUsers();
    assert.ok(popover.classList.contains('hidden'), 'Popover should have .hidden after second toggle');
    assert.strictEqual(btn.getAttribute('aria-expanded'), 'false');
    console.log('✓ Test 4 passed: toggleOnlineUsers toggles visibility and aria-expanded');
  }

  // Test 5: closeOnlineUsers ensures popover is hidden
  {
    const popover = document.getElementById('online-users-popover');
    const btn = document.getElementById('btn-online-users');

    context.globalThis.toggleOnlineUsers(); // open
    assert.ok(!popover.classList.contains('hidden'));

    context.globalThis.closeOnlineUsers(); // close
    assert.ok(popover.classList.contains('hidden'), 'closeOnlineUsers should hide popover');
    assert.strictEqual(btn.getAttribute('aria-expanded'), 'false');
    console.log('✓ Test 5 passed: closeOnlineUsers hides popover');
  }

  // Test 6: OAuth method prefixed logins (github:..., local:..., lp:...)
  {
    context.myNickname = 'github:fourdollars';
    context.globalThis.updateOnlineCount(3, ['local:admin', 'github:fourdollars', 'lp:alice']);
    const listEl = document.getElementById('online-users-list');
    assert.strictEqual(listEl.children.length, 3);

    const names = listEl.children.map(li => {
      const nameSpan = li.children.find(c => c.classList.contains('online-user-name'));
      return nameSpan ? nameSpan.textContent : '';
    });

    assert.deepStrictEqual(names, ['github:fourdollars (you)', 'local:admin', 'lp:alice']);
    assert.ok(listEl.children[0].classList.contains('me'));
    console.log('✓ Test 6 passed: OAuth method prefixes (github:..., local:..., lp:...) render properly');
  }

  console.log('All online users tests passed successfully! 🎉');
}

runTests().catch(err => {
  console.error(err);
  process.exit(1);
});
