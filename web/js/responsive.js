import './state.js';

const compactQuery = window.matchMedia('(max-width: 768px)');
const drawerQuery = window.matchMedia('(max-width: 1279px)');
// Width kept clear on the right of the toolbar for the absolutely positioned
// overflow trigger, which is not a flow child and so cannot be measured.
const OVERFLOW_TRIGGER_RESERVE = 46;

let chosenPane = null;

function el(id) {
    return document.getElementById(id);
}

globalThis.isCompactViewport = function isCompactViewport() {
    return compactQuery.matches;
};

globalThis.isDrawerViewport = function isDrawerViewport() {
    return drawerQuery.matches;
};

function syncDrawerTriggers(open) {
    ['btn-compact-nav', 'btn-tree'].forEach(id => el(id)?.setAttribute('aria-expanded', String(open)));
}

globalThis.closeTreeDrawer = function closeTreeDrawer() {
    document.body.classList.remove('tree-open');
    syncDrawerTriggers(false);
};

globalThis.toggleTreeDrawer = function toggleTreeDrawer() {
    syncDrawerTriggers(document.body.classList.toggle('tree-open'));
};

globalThis.closeCompactMenu = function closeCompactMenu() {
    el('compact-menu')?.classList.add('hidden');
    el('btn-compact-more')?.setAttribute('aria-expanded', 'false');
};

globalThis.toggleCompactMenu = function toggleCompactMenu() {
    const menu = el('compact-menu');
    if (!menu) return;
    const open = menu.classList.toggle('hidden') === false;
    el('btn-compact-more')?.setAttribute('aria-expanded', String(open));
};

globalThis.showPane = function showPane(pane) {
    const app = el('app');
    if (!app) return;
    if (pane === 'editor') { showEdit = true; paneFocus = 'edit'; }
    if (pane === 'preview') { showPreview = true; paneFocus = 'preview'; }
    chosenPane = pane;
    app.dataset.pane = pane;
    closeTreeDrawer();
    closeCompactMenu();
    applyPanelLayout();
    if (pane === 'editor' && editorInstance) editorInstance.refresh();
};

globalThis.syncPaneSwitcher = function syncPaneSwitcher() {
    const app = el('app');
    const switcher = el('pane-switcher');
    if (!app || !switcher) return;
    const hasDocument = !el('btn-edit')?.classList.contains('hidden');
    // Re-evaluated every call: 'chat' is only forced while no document exists.
    app.dataset.pane = hasDocument
        ? (chosenPane || (paneFocus === 'preview' ? 'preview' : 'editor'))
        : 'chat';
    switcher.querySelectorAll('button').forEach(button => {
        const usable = button.dataset.pane === 'chat' || hasDocument;
        const active = usable && app.dataset.pane === button.dataset.pane;
        button.classList.toggle('hidden', !usable);
        button.classList.toggle('active', active);
        button.setAttribute('aria-pressed', String(active));
    });
};

function toolbarParts() {
    return { bar: el('editor-toolbar'), menu: el('editor-toolbar-menu'), trigger: el('editor-toolbar-overflow') };
}

globalThis.closeToolbarOverflow = function closeToolbarOverflow() {
    const { menu, trigger } = toolbarParts();
    menu?.classList.add('hidden');
    trigger?.setAttribute('aria-expanded', 'false');
};

globalThis.toggleToolbarOverflow = function toggleToolbarOverflow() {
    const { menu, trigger } = toolbarParts();
    if (!menu) return;
    const open = menu.classList.toggle('hidden') === false;
    trigger?.setAttribute('aria-expanded', String(open));
};

function isSeparator(node) {
    return node?.classList.contains('toolbar-separator');
}

globalThis.layoutEditorToolbar = function layoutEditorToolbar() {
    const { bar, menu, trigger } = toolbarParts();
    if (!bar || !menu || !trigger) return;
    while (menu.firstChild) bar.appendChild(menu.firstChild);
    closeToolbarOverflow();
    trigger.classList.add('hidden');
    if (!bar.clientWidth || bar.scrollWidth <= bar.clientWidth + 1) return;

    trigger.classList.remove('hidden');
    const limit = bar.getBoundingClientRect().right - OVERFLOW_TRIGGER_RESERVE;
    let guard = bar.childElementCount + 2;
    while (guard-- > 0 && bar.lastElementChild
        && bar.lastElementChild.getBoundingClientRect().right > limit) {
        menu.insertBefore(bar.lastElementChild, menu.firstChild);
    }
    while (isSeparator(bar.lastElementChild)) {
        menu.insertBefore(bar.lastElementChild, menu.firstChild);
    }
};

function closeTransientOverlays(target) {
    if (!(target instanceof Element)) return;
    if (!target.closest('#compact-menu, [data-action="toggle-compact-menu"]')) closeCompactMenu();
    if (!target.closest('#editor-toolbar-menu, [data-action="toggle-toolbar-overflow"]')) closeToolbarOverflow();
}

function onBreakpointChange() {
    closeTreeDrawer();
    closeCompactMenu();
    closeToolbarOverflow();
    syncPaneSwitcher();
    layoutEditorToolbar();
}

export function initResponsive() {
    document.addEventListener('click', event => closeTransientOverlays(event.target));
    compactQuery.addEventListener('change', onBreakpointChange);
    drawerQuery.addEventListener('change', onBreakpointChange);

    const container = el('editor-container');
    if (container && typeof ResizeObserver === 'function') {
        new ResizeObserver(() => layoutEditorToolbar()).observe(container);
    }
    syncPaneSwitcher();
    layoutEditorToolbar();
}
