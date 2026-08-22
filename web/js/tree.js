import './state.js';
import { icon } from './icons.js';
// --- Note Management ---

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
    row.className = 'explorer-row';
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

function folderRow(note) {
    const row = document.createElement('div');
    row.className = 'explorer-row' + (note.id === currentNoteId && !(note.files || []).length ? ' active' : '');
    row.dataset.action = 'toggle-note';
    row.dataset.note = note.id;

    const chevron = document.createElement('span');
    chevron.className = 'chevron open';
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
        return;
    }

    const searchInput = document.getElementById('file-search-input');
    if (searchInput) searchInput.style.display = '';
    const query = searchInput ? searchInput.value.toLowerCase().trim() : '';

    notes.forEach(note => {
        const files = (note.files || []).filter(fname => !query || fname.toLowerCase().includes(query));
        if (query && files.length === 0 && !note.name.toLowerCase().includes(query)) return;

        const section = document.createElement('div');
        section.className = 'explorer-section';
        section.appendChild(folderRow(note));

        const children = document.createElement('div');
        children.className = 'explorer-children';
        // When searching, auto-expand all sections so results are visible
        if (query) children.classList.remove('collapsed');
        files.forEach(fname => children.appendChild(fileRow(note, fname)));
        section.appendChild(children);
        tree.appendChild(section);
    });
};
