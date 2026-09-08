import './state.js';
import { icon } from './icons.js';

// Every command the chrome exposes, so it is reachable without hunting for the
// control that happens to own it at the current breakpoint.
let currentMode = 'commands';
let modeHistory = [];
let matches = [];
let cursor = 0;

const COMMANDS = [
    // Navigation
    { label: 'Switch file…', icon: 'file-text', keys: 'Ctrl+O', section: 'Navigation', run: () => openPaletteMode('files') },
    { label: 'Switch note…', icon: 'folder', section: 'Navigation', run: () => openPaletteMode('notes') },

    // Files & Notes
    { label: 'New file', icon: 'file-plus', admin: true, section: 'Files & Notes', run: () => createFile() },
    { label: 'Rename current file', icon: 'pencil', admin: true, section: 'Files & Notes', run: renameCurrent },
    { label: 'Delete current file', icon: 'trash', admin: true, section: 'Files & Notes', run: () => deleteCurrentFile() },
    { label: 'New note', icon: 'plus', admin: true, section: 'Files & Notes', run: () => showNewNoteDialog() },
    { label: 'Open note settings', icon: 'settings', admin: true, section: 'Files & Notes', run: () => showNoteSettings(currentNoteId) },
    { label: 'Toggle note visibility', icon: 'globe', admin: true, section: 'Files & Notes', run: toggleCurrentNoteVisibility },

    // View & Panels
    { label: 'Toggle editor', icon: 'pencil', section: 'View & Panels', run: () => toggleEdit() },
    { label: 'Toggle preview', icon: 'eye', section: 'View & Panels', run: () => togglePreview() },
    { label: 'Swap editor / preview', icon: 'swap', keys: 'Ctrl+Shift+E', section: 'View & Panels', run: () => swapEditorPreview() },
    { label: 'Toggle sync scroll', icon: 'sync-scroll', section: 'View & Panels', run: () => toggleSyncScroll() },

    // AI & Chat
    { label: 'Switch model', icon: 'chip', admin: true, section: 'AI & Chat', run: () => showModelDialog() },
    { label: 'Search chat history', icon: 'search', section: 'AI & Chat', run: () => showSearchDialog() },
    { label: 'Archive chat', icon: 'archive', section: 'AI & Chat', run: () => showArchiveDialog() },

    // Account
    { label: 'Log out', icon: 'logout', section: 'Account', run: () => showLogoutDialog() },
];

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

function makeIcon(name, emoji) {
    if (emoji) {
        const span = document.createElement('span');
        span.className = 'icon-emoji';
        span.style.width = '16px';
        span.style.height = '16px';
        span.style.display = 'inline-flex';
        span.style.alignItems = 'center';
        span.style.justifyContent = 'center';
        span.style.fontSize = '14px';
        span.style.lineHeight = '1';
        span.textContent = emoji;
        return span;
    }
    return icon(name || 'dot');
}

function getFilesItems() {
    const items = [];
    const currentNote = (notes || []).find(n => n.id === currentNoteId);
    const currentNoteName = currentNote ? (currentNote.name || currentNote.id) : 'Current Note';

    if (Array.isArray(fileList)) {
        fileList.forEach(file => {
            const isCurrent = file === currentFilename;
            items.push({
                label: file,
                searchKey: file,
                icon: 'file-text',
                section: `Current Note (${currentNoteName})`,
                keys: isCurrent ? 'Current' : '',
                run: () => switchFile(file),
            });
        });
    }
    if (Array.isArray(notes)) {
        notes.forEach(note => {
            if (note.id !== currentNoteId && Array.isArray(note.files)) {
                const noteName = note.name || note.id;
                note.files.forEach(file => {
                    items.push({
                        label: `${noteName} / ${file}`,
                        searchKey: `${noteName} ${file}`,
                        icon: 'file-text',
                        section: 'Other Notes',
                        keys: noteName,
                        run: () => switchNote(note.id, file),
                    });
                });
            }
        });
    }
    return items;
}

function getNotesItems() {
    const items = [];
    if (Array.isArray(notes)) {
        notes.forEach(note => {
            const isCurrent = note.id === currentNoteId;
            const noteName = note.name || note.id;
            items.push({
                label: noteName,
                searchKey: `${noteName} ${note.id}`,
                icon: isCurrent ? 'folder-open' : 'folder',
                emoji: note.icon || null,
                section: 'Notes',
                keys: isCurrent ? 'Current Note' : 'Note',
                run: () => switchNote(note.id),
            });
        });
    }
    return items;
}

function available() {
    if (currentMode === 'files') {
        return getFilesItems();
    }
    if (currentMode === 'notes') {
        return getNotesItems();
    }
    if (currentMode === 'quickopen') {
        return [...getFilesItems(), ...getNotesItems()];
    }
    // Default: 'commands'
    return COMMANDS.filter(command => !command.admin || isAdmin);
}

function score(item, needle) {
    if (!needle) return 0;
    const label = item.label.toLowerCase();
    if (label === needle) return 100;
    if (label.startsWith(needle)) return 80;
    if (label.includes(needle)) return 40;
    if (item.searchKey && item.searchKey.toLowerCase().includes(needle)) return 20;
    return -1;
}

function updateModeLabels() {
    const { input, empty } = parts();
    if (!input) return;
    let placeholder = 'Type a command…';
    let emptyText = 'No matching command';

    if (currentMode === 'files') {
        placeholder = 'Select a file…';
        emptyText = 'No matching file';
    } else if (currentMode === 'notes') {
        placeholder = 'Select a note…';
        emptyText = 'No matching note';
    } else if (currentMode === 'quickopen') {
        placeholder = 'Go to file or note…';
        emptyText = 'No matching file or note';
    }

    input.placeholder = placeholder;
    input.setAttribute('aria-label', placeholder);
    if (empty) empty.textContent = emptyText;
}

function render(query) {
    const { list, empty } = parts();
    if (!list) return;
    const needle = query.trim().toLowerCase();
    const all = available();

    if (!needle) {
        matches = all;
    } else {
        matches = all
            .map(item => ({ item, s: score(item, needle) }))
            .filter(entry => entry.s >= 0)
            .sort((a, b) => b.s - a.s)
            .map(entry => entry.item);
    }

    if (cursor >= matches.length) cursor = 0;
    list.replaceChildren();
    let lastSection = null;
    matches.forEach((command, index) => {
        if (!needle && command.section && command.section !== lastSection) {
            lastSection = command.section;
            const header = document.createElement('div');
            header.className = 'palette-section';
            header.textContent = lastSection;
            list.appendChild(header);
        }

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
        keys.textContent = command.keys || '';
        row.append(makeIcon(command.icon, command.emoji), name, keys);
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
    modeHistory = [];
    currentMode = 'commands';
};

globalThis.openPaletteMode = function openPaletteMode(mode = 'commands', fromHistory = false) {
    const { overlay, input } = parts();
    if (!overlay || !input) return;
    if (!fromHistory && currentMode !== mode) {
        modeHistory.push(currentMode);
    }
    currentMode = mode;
    overlay.classList.remove('hidden');
    input.value = '';
    cursor = 0;
    updateModeLabels();
    render('');
    input.focus();
};

globalThis.openCommandPalette = function openCommandPalette() {
    modeHistory = [];
    openPaletteMode('commands', true);
};

globalThis.openQuickOpen = function openQuickOpen() {
    modeHistory = [];
    openPaletteMode('quickopen', true);
};

globalThis.runCommand = function runCommand(element) {
    const command = matches[Number(element.dataset.index)];
    if (!command) return;
    if (typeof command.run === 'function') {
        const isModeSwitch = command.run.toString().includes('openPaletteMode');
        if (!isModeSwitch) {
            closeCommandPalette();
        }
        command.run();
    }
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
        if (event.key === 'ArrowDown') {
            event.preventDefault();
            move(1);
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            move(-1);
        } else if (event.key === 'Backspace' && input.value === '' && modeHistory.length > 0) {
            event.preventDefault();
            const previousMode = modeHistory.pop();
            openPaletteMode(previousMode, true);
        } else if (event.key === 'Enter') {
            event.preventDefault();
            const command = matches[cursor];
            if (!command) return;
            if (typeof command.run === 'function') {
                const isModeSwitch = command.run.toString().includes('openPaletteMode');
                if (!isModeSwitch) {
                    closeCommandPalette();
                }
                command.run();
            }
        }
    });
    overlay.addEventListener('mousedown', event => {
        if (event.target === overlay) closeCommandPalette();
    });
}
