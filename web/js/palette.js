import './state.js';
import { icon } from './icons.js';

// Every command the chrome exposes, so it is reachable without hunting for the
// control that happens to own it at the current breakpoint.
const COMMANDS = [
    { label: 'Toggle editor', icon: 'pencil', run: () => toggleEdit() },
    { label: 'Toggle preview', icon: 'eye', run: () => togglePreview() },
    { label: 'Swap editor / preview', icon: 'swap', keys: 'Ctrl+Shift+E', run: () => swapEditorPreview() },
    { label: 'Toggle sync scroll', icon: 'sync-scroll', run: () => toggleSyncScroll() },
    { label: 'Search chat history', icon: 'search', run: () => showSearchDialog() },
    { label: 'Archive chat', icon: 'archive', run: () => showArchiveDialog() },
    { label: 'Switch model', icon: 'chip', admin: true, run: () => showModelDialog() },
    { label: 'New note', icon: 'plus', admin: true, run: () => showNewNoteDialog() },
    { label: 'New file', icon: 'file-plus', admin: true, run: () => createFile() },
    { label: 'Rename current file', icon: 'pencil', admin: true, run: renameCurrent },
    { label: 'Delete current file', icon: 'trash', admin: true, run: () => deleteCurrentFile() },
    { label: 'Toggle note visibility', icon: 'globe', admin: true, run: toggleCurrentNoteVisibility },
    { label: 'Open note settings', icon: 'settings', admin: true, run: () => showNoteSettings(currentNoteId) },
    { label: 'Log out', icon: 'logout', run: () => showLogoutDialog() },
];

let matches = [];
let cursor = 0;

async function renameCurrent() {
    if (!currentFilename) return;
    const name = await showDialog({
        title: 'Rename File', input: true, inputValue: currentFilename, placeholder: 'new-name.md',
    });
    if (name) renameCurrentFile(name);
}

function toggleCurrentNoteVisibility() {
    const control = document.querySelector(
        `#note-tree [data-action="toggle-note-visibility"][data-note="${CSS.escape(currentNoteId)}"]`,
    );
    if (control) toggleNoteVisibility(control);
}

function parts() {
    return {
        overlay: document.getElementById('command-palette'),
        input: document.getElementById('command-palette-input'),
        list: document.getElementById('command-palette-list'),
        empty: document.getElementById('command-palette-empty'),
    };
}

function available() {
    return COMMANDS.filter(command => !command.admin || isAdmin);
}

function render(query) {
    const { list, empty } = parts();
    if (!list) return;
    const needle = query.trim().toLowerCase();
    matches = available().filter(command => command.label.toLowerCase().includes(needle));
    if (cursor >= matches.length) cursor = 0;
    list.replaceChildren();
    matches.forEach((command, index) => {
        const row = document.createElement('div');
        row.className = 'palette-item' + (index === cursor ? ' selected' : '');
        row.id = `palette-item-${index}`;
        row.role = 'option';
        row.setAttribute('aria-selected', String(index === cursor));
        row.dataset.action = 'run-command';
        row.dataset.index = String(index);
        const name = document.createElement('span');
        name.className = 'palette-item-label';
        name.textContent = command.label;
        const keys = document.createElement('kbd');
        keys.className = 'palette-item-keys';
        keys.textContent = command.keys || '—';
        row.append(icon(command.icon), name, keys);
        list.appendChild(row);
    });
    empty?.classList.toggle('hidden', matches.length > 0);
    parts().input?.setAttribute('aria-activedescendant', matches.length ? `palette-item-${cursor}` : '');
    list.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
}

globalThis.isCommandPaletteOpen = function isCommandPaletteOpen() {
    const { overlay } = parts();
    return !!overlay && !overlay.classList.contains('hidden');
};

globalThis.closeCommandPalette = function closeCommandPalette() {
    parts().overlay?.classList.add('hidden');
};

globalThis.openCommandPalette = function openCommandPalette() {
    const { overlay, input } = parts();
    if (!overlay || !input) return;
    overlay.classList.remove('hidden');
    input.value = '';
    cursor = 0;
    render('');
    input.focus();
};

globalThis.runCommand = function runCommand(element) {
    const command = matches[Number(element.dataset.index)];
    closeCommandPalette();
    command?.run();
};

function move(step) {
    if (matches.length === 0) return;
    cursor = (cursor + step + matches.length) % matches.length;
    render(parts().input.value);
}

export function initPalette() {
    const { overlay, input } = parts();
    if (!overlay || !input) return;
    input.addEventListener('input', () => { cursor = 0; render(input.value); });
    input.addEventListener('keydown', event => {
        if (event.key === 'ArrowDown') { event.preventDefault(); move(1); }
        else if (event.key === 'ArrowUp') { event.preventDefault(); move(-1); }
        else if (event.key === 'Enter') {
            event.preventDefault();
            const command = matches[cursor];
            closeCommandPalette();
            command?.run();
        }
    });
    overlay.addEventListener('mousedown', event => {
        if (event.target === overlay) closeCommandPalette();
    });
}
