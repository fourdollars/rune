import './state.js';
import { icon } from './icons.js';

// One shared menu hosted outside #note-tree, so every row keeps a single
// always-visible action control instead of one button per capability.
const ITEMS = {
    note: [
        { action: 'create-file', label: 'New file', icon: 'file-plus' },
        { action: 'note-settings', label: 'Settings', icon: 'settings' },
    ],
    file: [
        { action: 'rename-file', label: 'Rename', icon: 'pencil' },
        { action: 'delete-file', label: 'Delete', icon: 'trash', danger: true },
    ],
};

let openTrigger = null;

function menuEl() {
    return document.getElementById('row-menu');
}

function items() {
    const menu = menuEl();
    return menu ? [...menu.querySelectorAll('button')] : [];
}

globalThis.closeRowMenu = function closeRowMenu(restoreFocus) {
    const menu = menuEl();
    if (!menu || menu.classList.contains('hidden')) return;
    menu.classList.add('hidden');
    menu.replaceChildren();
    if (openTrigger) {
        openTrigger.setAttribute('aria-expanded', 'false');
        if (restoreFocus && openTrigger.isConnected) openTrigger.focus();
    }
    openTrigger = null;
};

function build(menu, trigger) {
    const kind = trigger.dataset.kind === 'file' ? 'file' : 'note';
    ITEMS[kind].forEach(spec => {
        const button = document.createElement('button');
        button.type = 'button';
        button.role = 'menuitem';
        button.className = 'row-menu-item' + (spec.danger ? ' danger' : '');
        button.dataset.action = spec.action;
        button.dataset.note = trigger.dataset.note;
        if (trigger.dataset.file) button.dataset.file = trigger.dataset.file;
        button.append(icon(spec.icon), document.createTextNode(spec.label));
        menu.appendChild(button);
    });
}

function place(menu, trigger) {
    const rect = trigger.getBoundingClientRect();
    menu.style.visibility = 'hidden';
    menu.classList.remove('hidden');
    const width = menu.offsetWidth;
    const height = menu.offsetHeight;
    const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
    const below = rect.bottom + 4;
    const top = below + height > window.innerHeight - 8 ? Math.max(8, rect.top - height - 4) : below;
    menu.style.left = `${Math.round(left)}px`;
    menu.style.top = `${Math.round(top)}px`;
    menu.style.visibility = '';
}

globalThis.toggleRowMenu = function toggleRowMenu(trigger) {
    const menu = menuEl();
    if (!menu) return;
    const wasOpen = openTrigger === trigger && !menu.classList.contains('hidden');
    closeRowMenu();
    if (wasOpen) return;
    build(menu, trigger);
    place(menu, trigger);
    openTrigger = trigger;
    trigger.setAttribute('aria-expanded', 'true');
    items()[0]?.focus();
};

function moveFocus(step) {
    const list = items();
    if (list.length === 0) return;
    const index = list.indexOf(document.activeElement);
    const next = (index + step + list.length * 2) % list.length;
    list[index === -1 ? 0 : next].focus();
}

export function initRowMenu() {
    // Registered after initActions, so a menu item's command runs before the
    // menu is torn down.
    document.addEventListener('click', event => {
        const target = event.target;
        if (target instanceof Element && target.closest('[data-action="row-menu"]')) return;
        closeRowMenu();
    });
    document.addEventListener('keydown', event => {
        const menu = menuEl();
        if (!menu || menu.classList.contains('hidden')) return;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            moveFocus(event.key === 'ArrowDown' ? 1 : -1);
        } else if (event.key === 'Tab') {
            closeRowMenu();
        }
    });
    window.addEventListener('resize', () => closeRowMenu());
    document.getElementById('note-tree')?.addEventListener('scroll', () => closeRowMenu());
}
