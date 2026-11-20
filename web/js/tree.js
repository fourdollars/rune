import './state.js';
import { icon } from './icons.js';
// --- Note Management ---

const FOLDED_NOTES_KEY = 'rune_folded_notes';

function getFoldedNotes() {
    try {
        const raw = localStorage.getItem(FOLDED_NOTES_KEY);
        if (!raw) return new Set();
        const parsed = JSON.parse(raw);
        return Array.isArray(parsed) ? new Set(parsed.filter(id => typeof id === 'string')) : new Set();
    } catch {
        return new Set();
    }
}

function saveFoldedNotes(folded) {
    try {
        if (folded.size === 0) localStorage.removeItem(FOLDED_NOTES_KEY);
        else localStorage.setItem(FOLDED_NOTES_KEY, JSON.stringify([...folded]));
    } catch {}
}

function areAllNotesFolded(folded = getFoldedNotes()) {
    return notes.length > 0 && notes.every(note => folded.has(note.id));
}

function updateFoldAllButton(folded = getFoldedNotes()) {
    const btn = document.getElementById('btn-toggle-fold-all');
    if (!btn) return;
    btn.disabled = notes.length === 0;
    const allFolded = areAllNotesFolded(folded);
    const label = allFolded ? 'Unfold all notebooks' : 'Fold all notebooks';
    const iconName = allFolded ? 'unfold-all' : 'fold-all';
    btn.dataset.icon = iconName;
    btn.title = label;
    btn.setAttribute('aria-label', label);
    btn.setAttribute('aria-pressed', String(allFolded));
    if (typeof setRuneIcon === 'function') setRuneIcon(btn, iconName);
}

globalThis.setNoteFolded = function setNoteFolded(noteId, isFolded) {
    if (!noteId) return;
    const folded = getFoldedNotes();
    if (isFolded) folded.add(noteId);
    else folded.delete(noteId);
    saveFoldedNotes(folded);
    updateFoldAllButton(folded);
};

globalThis.foldAllNotes = function foldAllNotes() {
    const folded = new Set(notes.map(note => note.id).filter(Boolean));
    saveFoldedNotes(folded);
    renderNoteList();
};

globalThis.unfoldAllNotes = function unfoldAllNotes() {
    saveFoldedNotes(new Set());
    renderNoteList();
};

globalThis.toggleFoldAllNotes = function toggleFoldAllNotes() {
    if (areAllNotesFolded(getFoldedNotes())) unfoldAllNotes();
    else foldAllNotes();
};

function typeIcon(name, emoji) {
    const span = document.createElement('span');
    span.className = 'icon icon-type';
    if (emoji) span.textContent = emoji;
    else span.appendChild(icon(name));
    return span;
}

function visibilityControl(isPublic, description, action, data) {
    const control = document.createElement(isAdmin ? 'button' : 'span');
    control.className = 'icon icon-vis' + (isAdmin ? ' clickable' : '') + (isPublic ? '' : ' private');
    control.appendChild(icon(isPublic ? 'globe' : 'lock'));
    const state = isPublic ? 'Public' : 'Private';
    const hint = isAdmin ? (isPublic ? ' — click to make private' : ' — click to publish') : '';
    control.title = `${state} ${description}${hint}`;
    control.setAttribute('aria-label', control.title);
    if (isAdmin) {
        control.type = 'button';
        control.dataset.action = action;
        control.setAttribute('aria-pressed', String(isPublic));
        Object.assign(control.dataset, data);
    }
    return control;
}

function overflowControl(description, data) {
    const wrapper = document.createElement('span');
    wrapper.className = 'actions note-actions';
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'row-menu-btn';
    button.appendChild(icon('more'));
    button.title = `Actions for ${description}`;
    button.setAttribute('aria-label', `Actions for ${description}`);
    button.setAttribute('aria-haspopup', 'menu');
    button.setAttribute('aria-expanded', 'false');
    button.dataset.action = 'row-menu';
    Object.assign(button.dataset, data);
    wrapper.appendChild(button);
    return wrapper;
}

function label(text, bold) {
    const span = document.createElement('span');
    span.className = 'label';
    span.textContent = text;
    if (bold) span.style.fontWeight = '500';
    return span;
}

function emptyTree(tree) {
    const empty = document.createElement('div');
    empty.className = 'explorer-empty';
    empty.append(document.createTextNode('No notes yet.'), document.createElement('br'), document.createTextNode('Use the '));
    const strong = document.createElement('b');
    strong.textContent = 'New note';
    empty.append(strong, document.createTextNode(' button below to create one.'));
    tree.appendChild(empty);
    const search = document.getElementById('file-search-input');
    if (search) search.style.display = 'none';
}

function fileRow(note, fname) {
    const row = document.createElement('div');
    row.className = 'explorer-row' + (note.id === currentNoteId && fname === currentFilename ? ' active' : '');
    row.style.paddingLeft = '20px';
    const isPublic = !!(note.fileVisibility || {})[fname];

    row.appendChild(typeIcon(fname.endsWith('.md') ? 'file-text' : 'file'));
    row.appendChild(label(fname));
    row.appendChild(visibilityControl(isPublic, `file “${fname}”`, 'toggle-file-visibility', {
        note: note.id,
        file: fname,
    }));
    if (isAdmin) {
        row.appendChild(overflowControl(`file “${fname}”`, { kind: 'file', note: note.id, file: fname }));
    }

    row.dataset.action = 'switch-file';
    row.dataset.note = note.id;
    row.dataset.file = fname;
    return row;
}

function folderRow(note, isFolded = false) {
    const row = document.createElement('div');
    row.className = 'explorer-row' + (note.id === currentNoteId && !(note.files || []).length ? ' active' : '');
    row.dataset.action = 'toggle-note';
    row.dataset.note = note.id;
    row.setAttribute('aria-expanded', String(!isFolded));

    const chevron = document.createElement('span');
    chevron.className = 'chevron' + (isFolded ? '' : ' open');
    chevron.textContent = '›';
    chevron.setAttribute('aria-hidden', 'true');

    row.appendChild(chevron);
    row.appendChild(typeIcon(note.id === currentNoteId ? 'folder-open' : 'folder', note.icon));
    row.appendChild(label(note.name, true));
    row.appendChild(visibilityControl(!!note.public, `note “${note.name}”`, 'toggle-note-visibility', {
        note: note.id,
    }));
    if (isAdmin) {
        row.appendChild(overflowControl(`note “${note.name}”`, { kind: 'note', note: note.id }));
    }
    return row;
}

globalThis.renderNoteList = function renderNoteList() {
    const tree = document.getElementById('note-tree');
    if (!tree) return;
    if (typeof closeRowMenu === 'function') closeRowMenu();
    tree.replaceChildren();
    if (notes.length === 0) {
        emptyTree(tree);
        updateFoldAllButton(new Set());
        return;
    }

    const searchInput = document.getElementById('file-search-input');
    if (searchInput) searchInput.style.display = '';
    const query = searchInput ? searchInput.value.toLowerCase().trim() : '';
    const foldedNotes = getFoldedNotes();
    updateFoldAllButton(foldedNotes);

    notes.forEach(note => {
        const files = (note.files || []).filter(fname => !query || fname.toLowerCase().includes(query));
        if (query && files.length === 0 && !note.name.toLowerCase().includes(query)) return;

        // When searching, auto-expand all sections so results are visible
        const isFolded = !query && foldedNotes.has(note.id);

        const section = document.createElement('div');
        section.className = 'explorer-section';
        section.appendChild(folderRow(note, isFolded));

        const children = document.createElement('div');
        children.className = 'explorer-children' + (isFolded ? ' collapsed' : '');
        files.forEach(fname => children.appendChild(fileRow(note, fname)));
        section.appendChild(children);
        tree.appendChild(section);
    });
};
