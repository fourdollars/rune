// Unit test for Rune Notes folded notebook persistence in localStorage
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

function createStorage(initial = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem(key) {
      return data.has(key) ? data.get(key) : null;
    },
    setItem(key, value) {
      data.set(key, String(value));
    },
    removeItem(key) {
      data.delete(key);
    },
  };
}

function createDOM() {
  const elements = new Map();

  class MockClassList {
    constructor(owner) {
      this.owner = owner;
      this.classes = new Set();
    }
    add(...cls) { cls.forEach(c => this.classes.add(c)); }
    remove(...cls) { cls.forEach(c => this.classes.delete(c)); }
    contains(cls) { return this.classes.has(cls); }
    toggle(cls, force) {
      if (force === undefined) {
        if (this.classes.has(cls)) {
          this.classes.delete(cls);
          return false;
        }
        this.classes.add(cls);
        return true;
      }
      if (force) {
        this.classes.add(cls);
        return true;
      }
      this.classes.delete(cls);
      return false;
    }
  }

  class MockElement {
    constructor(tagName = 'div', id = '') {
      this.tagName = tagName.toUpperCase();
      this.id = id;
      this.attributes = new Map();
      this.classList = new MockClassList(this);
      this.children = [];
      this.parentElement = null;
      this.dataset = {};
      this.style = {};
      this.value = '';
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
    setAttribute(k, v) { this.attributes.set(k, String(v)); }
    getAttribute(k) { return this.attributes.get(k) || null; }
    appendChild(child) {
      if (child && typeof child === 'object') {
        child.parentElement = this;
      }
      this.children.push(child);
      return child;
    }
    append(...nodes) {
      nodes.forEach(n => this.appendChild(n));
    }
    replaceChildren(...nodes) {
      this.children = [];
      this.append(...nodes);
    }
    closest(selector) {
      const cls = selector.startsWith('.') ? selector.slice(1) : null;
      let cur = this;
      while (cur) {
        if (cls && cur.classList && cur.classList.contains(cls)) return cur;
        cur = cur.parentElement;
      }
      return null;
    }
    querySelector(selector) {
      const cls = selector.startsWith('.') ? selector.slice(1) : null;
      for (const child of this.children) {
        if (!child || typeof child !== 'object') continue;
        if (cls && child.classList && child.classList.contains(cls)) return child;
        if (typeof child.querySelector === 'function') {
          const found = child.querySelector(selector);
          if (found) return found;
        }
      }
      return null;
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
      return { textContent: String(text) };
    },
  };

  const tree = new MockElement('div', 'note-tree');
  const searchInput = new MockElement('input', 'file-search-input');
  const toggleAllBtn = new MockElement('button', 'btn-toggle-fold-all');

  return { document, tree, searchInput, toggleAllBtn };
}

function loadTreeContext(localStorage) {
  const { document, tree, searchInput, toggleAllBtn } = createDOM();
  const context = {
    document,
    localStorage,
    notes: [
      { id: 'alpha', name: 'Alpha', files: ['a.md'], public: false, fileVisibility: {} },
      { id: 'beta', name: 'Beta', files: ['b.md'], public: false, fileVisibility: {} },
    ],
    currentNoteId: 'alpha',
    currentFilename: 'a.md',
    isAdmin: false,
    icon: name => {
      const svg = document.createElement('svg');
      svg.dataset.iconName = name;
      return svg;
    },
    setRuneIcon: (el, name) => {
      const svg = document.createElement('svg');
      svg.dataset.iconName = name;
      el.replaceChildren(svg);
    },
    switchNote: noteId => {
      context.currentNoteId = noteId;
      context.renderNoteList();
    },
    console,
  };
  context.globalThis = context;
  vm.createContext(context);

  const treeJs = fs.readFileSync(path.join(__dirname, '../web/js/tree.js'), 'utf8')
    .replace(/^import .*?;\s*/gm, '');
  const treeActionsJs = fs.readFileSync(path.join(__dirname, '../web/js/tree-actions.js'), 'utf8');

  vm.runInContext(treeJs, context);
  vm.runInContext(treeActionsJs, context);

  return { context, tree, searchInput, toggleAllBtn };
}

function getSectionState(tree, noteId) {
  const section = tree.children.find(s => {
    const row = s.querySelector('.explorer-row');
    return row && row.dataset.note === noteId;
  });
  assert.ok(section, `Section for note ${noteId} should exist`);
  const row = section.querySelector('.explorer-row');
  const chevron = row.querySelector('.chevron');
  const children = section.querySelector('.explorer-children');
  return {
    row,
    folded: children.classList.contains('collapsed'),
    chevronOpen: chevron.classList.contains('open'),
    ariaExpanded: row.getAttribute('aria-expanded'),
  };
}

console.log('=== Rune Notes Folded Notebooks Test Suite ===');

// Test 1: Default state has all notebooks expanded
{
  const storage = createStorage();
  const { context, tree } = loadTreeContext(storage);
  context.renderNoteList();

  const alpha = getSectionState(tree, 'alpha');
  const beta = getSectionState(tree, 'beta');
  assert.strictEqual(alpha.folded, false);
  assert.strictEqual(alpha.chevronOpen, true);
  assert.strictEqual(alpha.ariaExpanded, 'true');
  assert.strictEqual(beta.folded, false);
  assert.strictEqual(beta.chevronOpen, true);
  assert.strictEqual(beta.ariaExpanded, 'true');
  console.log('✓ Test 1 passed: Notebooks default to expanded');
}

// Test 2: Folding a notebook persists across switchNote/renderNoteList and browser reload
{
  const storage = createStorage();
  const firstSession = loadTreeContext(storage);
  firstSession.context.renderNoteList();

  const betaBefore = getSectionState(firstSession.tree, 'beta');
  firstSession.context.toggleNoteRow(betaBefore.row);

  assert.deepStrictEqual(JSON.parse(storage.getItem('rune_folded_notes')), ['beta']);
  const betaAfterToggle = getSectionState(firstSession.tree, 'beta');
  assert.strictEqual(betaAfterToggle.folded, true);
  assert.strictEqual(betaAfterToggle.chevronOpen, false);
  assert.strictEqual(betaAfterToggle.ariaExpanded, 'false');

  // Simulate page reload with same localStorage
  const reloadedSession = loadTreeContext(storage);
  reloadedSession.context.renderNoteList();

  const alphaReloaded = getSectionState(reloadedSession.tree, 'alpha');
  const betaReloaded = getSectionState(reloadedSession.tree, 'beta');
  assert.strictEqual(alphaReloaded.folded, false);
  assert.strictEqual(betaReloaded.folded, true);
  assert.strictEqual(betaReloaded.chevronOpen, false);
  assert.strictEqual(betaReloaded.ariaExpanded, 'false');
  console.log('✓ Test 2 passed: Folded state persists across re-render and reload');
}

// Test 3: Search temporarily expands folded notebooks without clearing saved state
{
  const storage = createStorage({ rune_folded_notes: JSON.stringify(['beta']) });
  const { context, tree, searchInput } = loadTreeContext(storage);

  searchInput.value = 'b.md';
  context.renderNoteList();
  const betaDuringSearch = getSectionState(tree, 'beta');
  assert.strictEqual(betaDuringSearch.folded, false);
  assert.strictEqual(betaDuringSearch.chevronOpen, true);

  searchInput.value = '';
  context.renderNoteList();
  const betaAfterSearch = getSectionState(tree, 'beta');
  assert.strictEqual(betaAfterSearch.folded, true);
  assert.strictEqual(betaAfterSearch.chevronOpen, false);
  assert.deepStrictEqual(JSON.parse(storage.getItem('rune_folded_notes')), ['beta']);
  console.log('✓ Test 3 passed: Search expands temporarily and restores folded state');
}

// Test 4: Unfolding removes note from localStorage and clears key when empty
{
  const storage = createStorage({ rune_folded_notes: JSON.stringify(['beta']) });
  const { context, tree } = loadTreeContext(storage);
  context.renderNoteList();

  const betaBefore = getSectionState(tree, 'beta');
  context.toggleNoteRow(betaBefore.row);

  assert.strictEqual(storage.getItem('rune_folded_notes'), null);
  const betaAfter = getSectionState(tree, 'beta');
  assert.strictEqual(betaAfter.folded, false);
  assert.strictEqual(betaAfter.chevronOpen, true);
  assert.strictEqual(betaAfter.ariaExpanded, 'true');
  console.log('✓ Test 4 passed: Unfolding updates DOM and cleans up localStorage');
}

// Test 5: Invalid JSON or throwing localStorage does not break rendering
{
  const badJsonStorage = createStorage({ rune_folded_notes: '{not-valid-json' });
  const ctx1 = loadTreeContext(badJsonStorage);
  ctx1.context.renderNoteList();
  assert.strictEqual(getSectionState(ctx1.tree, 'alpha').folded, false);

  const throwingStorage = {
    getItem() { throw new Error('blocked'); },
    setItem() { throw new Error('blocked'); },
    removeItem() { throw new Error('blocked'); },
  };
  const ctx2 = loadTreeContext(throwingStorage);
  ctx2.context.renderNoteList();
  ctx2.context.toggleNoteRow(getSectionState(ctx2.tree, 'alpha').row);
  console.log('✓ Test 5 passed: Handles invalid JSON and inaccessible localStorage gracefully');
}

// Test 6: toggleFoldAllNotes / foldAllNotes / unfoldAllNotes and button state
{
  const storage = createStorage();
  const { context, tree, toggleAllBtn } = loadTreeContext(storage);
  context.renderNoteList();

  // Initially expanded -> button shows fold-all
  assert.strictEqual(toggleAllBtn.dataset.icon, 'fold-all');
  assert.strictEqual(toggleAllBtn.getAttribute('aria-pressed'), 'false');
  assert.strictEqual(toggleAllBtn.title, 'Fold all notebooks');

  // Fold all via smart toggle
  context.toggleFoldAllNotes();
  assert.deepStrictEqual(JSON.parse(storage.getItem('rune_folded_notes')).sort(), ['alpha', 'beta']);
  assert.strictEqual(getSectionState(tree, 'alpha').folded, true);
  assert.strictEqual(getSectionState(tree, 'beta').folded, true);
  assert.strictEqual(toggleAllBtn.dataset.icon, 'unfold-all');
  assert.strictEqual(toggleAllBtn.getAttribute('aria-pressed'), 'true');
  assert.strictEqual(toggleAllBtn.title, 'Unfold all notebooks');

  // Unfold one notebook -> button switches back to fold-all
  context.toggleNoteRow(getSectionState(tree, 'alpha').row);
  assert.strictEqual(toggleAllBtn.dataset.icon, 'fold-all');
  assert.strictEqual(toggleAllBtn.getAttribute('aria-pressed'), 'false');

  // Fold all again, then unfold all via smart toggle
  context.toggleFoldAllNotes();
  assert.strictEqual(toggleAllBtn.dataset.icon, 'unfold-all');
  context.toggleFoldAllNotes();
  assert.strictEqual(storage.getItem('rune_folded_notes'), null);
  assert.strictEqual(getSectionState(tree, 'alpha').folded, false);
  assert.strictEqual(getSectionState(tree, 'beta').folded, false);
  assert.strictEqual(toggleAllBtn.dataset.icon, 'fold-all');

  // Empty notes disables the button
  context.notes = [];
  context.renderNoteList();
  assert.strictEqual(toggleAllBtn.disabled, true);
  console.log('✓ Test 6 passed: Fold all / unfold all toggle and button state work properly');
}

console.log('All folded notebook tests passed successfully!');
