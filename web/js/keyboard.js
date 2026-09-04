import './state.js';

// Topmost-first. Escape unwinds exactly one layer per press and never touches
// the editor buffer, so unsaved content is safe.
const LAYERS = [
    ['#command-palette', () => closeCommandPalette()],
    ['#row-menu', () => closeRowMenu(true)],
    ['#emoji-picker-popover', () => closeEmojiPicker(true)],
    ['#generic-dialog-modal', () => document.getElementById('generic-dialog-cancel')?.click()],
    ['#dir-browser-modal', () => hideDirBrowser()],
    ['#note-settings-modal', () => hideNoteSettings()],
    ['#model-modal', () => hideModelDialog()],
    ['#search-modal', () => hideSearchDialog()],
    ['#archive-modal', () => hideArchiveDialog()],
    ['#new-note-modal', () => hideNewNoteDialog()],
    ['#logout-modal', () => hideLogoutDialog()],
    ['#editor-toolbar-menu', () => closeToolbarOverflow()],
    ['#compact-menu', () => closeCompactMenu()],
];

function closeTopmostOverlay() {
    for (const [selector, close] of LAYERS) {
        const node = document.querySelector(selector);
        if (node && !node.classList.contains('hidden')) {
            close();
            return true;
        }
    }
    if (document.body.classList.contains('tree-open')) {
        closeTreeDrawer();
        return true;
    }
    return false;
}

function inEditor(target) {
    return target instanceof Element && !!target.closest('.CodeMirror');
}

// CodeMirror owns Ctrl-K inside the editor (insert link); everywhere else the
// same key opens the palette.
function isPaletteChord(event) {
    if (event.key !== 'k' && event.key !== 'K') return false;
    if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return false;
    return !inEditor(event.target);
}

function activatesLikeButton(target) {
    if (!(target instanceof Element)) return null;
    const node = target.closest('[role="button"], [role="option"]');
    // Resize handles run off mousedown/mouseup and own their key handling.
    if (!node || node.tagName === 'BUTTON' || node.classList.contains('resize-handle')) return null;
    return node;
}

export function initKeyboard() {
    document.addEventListener('keydown', event => {
        if (event.key === 'Escape') {
            if (closeTopmostOverlay()) event.preventDefault();
            return;
        }
        if (isPaletteChord(event)) {
            event.preventDefault();
            if (isCommandPaletteOpen()) closeCommandPalette();
            else openCommandPalette();
            return;
        }
        if (event.key !== 'Enter' && event.key !== ' ') return;
        const node = activatesLikeButton(event.target);
        if (!node) return;
        event.preventDefault();
        node.click();
    });
}
