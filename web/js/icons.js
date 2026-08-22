// Rune icon set — one coherent family of inline SVGs.
// 24x24 viewBox, 1.75px strokes, round joins, monochrome via currentColor,
// so the light/dark palette needs no per-theme icon assets.
// A leading '*' on a path marks it as filled instead of stroked.

const NS = 'http://www.w3.org/2000/svg';

const PATHS = {
    pencil: ['M12 20h8.5', 'M16.4 3.6a2.1 2.1 0 0 1 3 3L7.6 18.4 3.3 19.7l1.3-4.3Z'],
    eye: ['M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z', 'M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z'],
    'sync-scroll': ['M4 3.5v17', 'M20 3.5v17', 'M12 6.5v11', 'm9 9.5 3-3 3 3', 'm9 14.5 3 3 3-3'],
    search: ['M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14Z', 'm20.5 20.5-4.4-4.4'],
    archive: ['M2.5 4.5h19v4h-19z', 'M4.5 8.5V19a1.5 1.5 0 0 0 1.5 1.5h12a1.5 1.5 0 0 0 1.5-1.5V8.5', 'M10 12.5h4'],
    logout: ['M15 3.5h3.5a2 2 0 0 1 2 2v13a2 2 0 0 1-2 2H15', 'm10 16.5 4.5-4.5L10 7.5', 'M14.5 12h-11'],
    plus: ['M12 5v14', 'M5 12h14'],
    'file-plus': ['M13.8 3.2H7a2 2 0 0 0-2 2v13.6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.4Z', 'M13.8 3.2v5.2H19', 'M12 11.5v6', 'M9 14.5h6'],
    settings: [
        'M12 15.4a3.4 3.4 0 1 0 0-6.8 3.4 3.4 0 0 0 0 6.8Z',
        'M19.3 14.6a1.5 1.5 0 0 0 .3 1.7l.1.1a1.9 1.9 0 1 1-2.7 2.7l-.1-.1a1.5 1.5 0 0 0-2.5 1.1v.2a1.9 1.9 0 1 1-3.8 0v-.1a1.5 1.5 0 0 0-2.5-1.1l-.1.1a1.9 1.9 0 1 1-2.7-2.7l.1-.1a1.5 1.5 0 0 0-1.1-2.5h-.2a1.9 1.9 0 1 1 0-3.8h.1a1.5 1.5 0 0 0 1.1-2.5l-.1-.1a1.9 1.9 0 1 1 2.7-2.7l.1.1a1.5 1.5 0 0 0 2.5-1.1v-.2a1.9 1.9 0 1 1 3.8 0v.1a1.5 1.5 0 0 0 2.5 1.1l.1-.1a1.9 1.9 0 1 1 2.7 2.7l-.1.1a1.5 1.5 0 0 0 1.1 2.5h.2a1.9 1.9 0 1 1 0 3.8h-.1a1.5 1.5 0 0 0-1.4.9Z',
    ],
    trash: ['M3.8 6.8h16.4', 'M10 11v6.5', 'M14 11v6.5', 'M6.2 6.8 7.1 20a1.4 1.4 0 0 0 1.4 1.3h7a1.4 1.4 0 0 0 1.4-1.3l.9-13.2', 'M9.2 6.8V4.6a1.6 1.6 0 0 1 1.6-1.6h2.4a1.6 1.6 0 0 1 1.6 1.6v2.2'],
    lock: ['M5.8 10.5h12.4a1.3 1.3 0 0 1 1.3 1.3v7.6a1.3 1.3 0 0 1-1.3 1.3H5.8a1.3 1.3 0 0 1-1.3-1.3v-7.6a1.3 1.3 0 0 1 1.3-1.3Z', 'M8 10.5V7.2a4 4 0 0 1 8 0v3.3'],
    globe: ['M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z', 'M3.2 12h17.6', 'M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18Z'],
    folder: ['M3.2 6.8a2 2 0 0 1 2-2h3.6l2.1 2.6h7.9a2 2 0 0 1 2 2v8.8a2 2 0 0 1-2 2H5.2a2 2 0 0 1-2-2Z'],
    'folder-open': ['M3.2 10.4V6.8a2 2 0 0 1 2-2h3.6l2.1 2.6h7.9a2 2 0 0 1 2 2v1', 'M3.2 10.4h17.9a1 1 0 0 1 1 1.3l-1.8 6.4a2 2 0 0 1-1.9 1.5H5.2a2 2 0 0 1-2-2Z'],
    'file-text': ['M13.8 3.2H7a2 2 0 0 0-2 2v13.6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.4Z', 'M13.8 3.2v5.2H19', 'M8.5 13h7', 'M8.5 16.6h4.5'],
    file: ['M13.8 3.2H7a2 2 0 0 0-2 2v13.6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.4Z', 'M13.8 3.2v5.2H19'],
    copy: ['M9.4 8.6h9a1.3 1.3 0 0 1 1.3 1.3v9a1.3 1.3 0 0 1-1.3 1.3h-9a1.3 1.3 0 0 1-1.3-1.3v-9A1.3 1.3 0 0 1 9.4 8.6Z', 'M5.2 15.4h-.6a1.3 1.3 0 0 1-1.3-1.3v-9a1.3 1.3 0 0 1 1.3-1.3h9a1.3 1.3 0 0 1 1.3 1.3v.6'],
    check: ['m4.8 12.6 4.6 4.6L19.2 7.4'],
    x: ['m6 6 12 12', 'M18 6 6 18'],
    menu: ['M3.8 7h16.4', 'M3.8 12h16.4', 'M3.8 17h16.4'],
    more: ['*M7.5 12a1.4 1.4 0 1 1-2.8 0 1.4 1.4 0 0 1 2.8 0Z', '*M13.4 12a1.4 1.4 0 1 1-2.8 0 1.4 1.4 0 0 1 2.8 0Z', '*M19.3 12a1.4 1.4 0 1 1-2.8 0 1.4 1.4 0 0 1 2.8 0Z'],
    send: ['M21 3 10.5 13.5', 'M21 3 14.4 21.2 10.5 13.5 2.8 9.6Z'],
    link: ['M10.2 14.4a4.4 4.4 0 0 0 6.5.3l2.5-2.5a4.4 4.4 0 0 0-6.2-6.2l-1.5 1.4', 'M13.8 9.6a4.4 4.4 0 0 0-6.5-.3l-2.5 2.5a4.4 4.4 0 0 0 6.2 6.2l1.5-1.4'],
    image: ['M4.4 4h15.2a1.3 1.3 0 0 1 1.3 1.3v13.4a1.3 1.3 0 0 1-1.3 1.3H4.4a1.3 1.3 0 0 1-1.3-1.3V5.3A1.3 1.3 0 0 1 4.4 4Z', 'M9.3 10.6a1.6 1.6 0 1 0 0-3.2 1.6 1.6 0 0 0 0 3.2Z', 'm3.1 16.6 5.2-5.1 3.8 3.8 3-2.9 5.8 5.7'],
    code: ['m9 17.2-5.2-5.2L9 6.8', 'm15 6.8 5.2 5.2-5.2 5.2'],
    list: ['M9.2 6.5h11.4', 'M9.2 12h11.4', 'M9.2 17.5h11.4', '*M5.6 6.5a1.2 1.2 0 1 1-2.4 0 1.2 1.2 0 0 1 2.4 0Z', '*M5.6 12a1.2 1.2 0 1 1-2.4 0 1.2 1.2 0 0 1 2.4 0Z', '*M5.6 17.5a1.2 1.2 0 1 1-2.4 0 1.2 1.2 0 0 1 2.4 0Z'],
    'list-ordered': ['M9.8 6.5h10.8', 'M9.8 12h10.8', 'M9.8 17.5h10.8', 'M4.3 4.6h1v3.8', 'M3.4 8.4h2.6', 'M3.4 10.6a1.3 1.3 0 0 1 2.5.4c0 1.1-2.5 1.7-2.5 2.9h2.6', 'M3.4 15.8a1.2 1.2 0 0 1 2.3.5 1 1 0 0 1-1.1 1 1 1 0 0 1 1.1 1 1.2 1.2 0 0 1-2.3.5'],
    'list-check': ['M10.4 6.4h10.2', 'M10.4 12h10.2', 'M10.4 17.6h10.2', 'm3.2 6.4 1.6 1.6 3-3.2', 'm3.2 12 1.6 1.6 3-3.2', 'm3.2 17.6 1.6 1.6 3-3.2'],
    table: ['M3.6 4.4h16.8v15.2H3.6z', 'M3.6 9.5h16.8', 'M3.6 14.6h16.8', 'M9.2 4.4v15.2', 'M14.8 4.4v15.2'],
    heading: ['M6.2 4.8v14.4', 'M17.8 4.8v14.4', 'M6.2 12h11.6'],
    user: ['M19.6 20.6v-1.9a4 4 0 0 0-4-4H8.4a4 4 0 0 0-4 4v1.9', 'M12 10.8a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z'],
    bot: ['M5.2 8.8h13.6a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5.2a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Z', 'M12 5.6v3.2', 'M12 2.6a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3Z', '*M9.6 13.8a1.1 1.1 0 1 1-2.2 0 1.1 1.1 0 0 1 2.2 0Z', '*M16.6 13.8a1.1 1.1 0 1 1-2.2 0 1.1 1.1 0 0 1 2.2 0Z', 'M9.6 17h4.8'],
    'arrow-up': ['M12 19.4V5', 'm6.2 10.8 5.8-5.8 5.8 5.8'],
    command: ['M17.5 3.2a3 3 0 0 0-3 3v11.6a3 3 0 1 0 3-3H6.5a3 3 0 1 0 3 3V6.2a3 3 0 1 0-3 3h11a3 3 0 0 0 0-6Z'],
    chip: ['M6.4 6.4h11.2v11.2H6.4z', 'M9.8 9.8h4.4v4.4H9.8z', 'M9.6 3.2v3.2', 'M14.4 3.2v3.2', 'M9.6 17.6v3.2', 'M14.4 17.6v3.2', 'M3.2 9.6h3.2', 'M3.2 14.4h3.2', 'M17.6 9.6h3.2', 'M17.6 14.4h3.2'],
    dot: ['*M12 17.2a5.2 5.2 0 1 1 0-10.4 5.2 5.2 0 0 1 0 10.4Z'],
    swap: ['M3.6 8.6h16.8', 'm16.6 4.8 3.8 3.8-3.8 3.8', 'M20.4 15.4H3.6', 'm7.4 11.6-3.8 3.8 3.8 3.8'],
};

/** Builds a standalone <svg> node for `name`; unknown names fall back to a dot. */
export function icon(name, className) {
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '1.75');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    svg.setAttribute('class', className ? `rune-icon ${className}` : 'rune-icon');
    (PATHS[name] || PATHS.dot).forEach(entry => {
        const filled = entry.startsWith('*');
        const path = document.createElementNS(NS, 'path');
        path.setAttribute('d', filled ? entry.slice(1) : entry);
        if (filled) {
            path.setAttribute('fill', 'currentColor');
            path.setAttribute('stroke', 'none');
        }
        svg.appendChild(path);
    });
    return svg;
}

/** Replaces an element's contents with a single icon. */
export function setIcon(element, name) {
    if (element) element.replaceChildren(icon(name));
}

/** Prepends an icon to every [data-icon] element that has none yet. */
export function hydrateIcons(root = document) {
    root.querySelectorAll('[data-icon]').forEach(element => {
        if (element.querySelector('svg')) return;
        element.prepend(icon(element.dataset.icon));
    });
}

export function hasIcon(name) {
    return Object.hasOwn(PATHS, name);
}

globalThis.runeIcon = icon;
globalThis.setRuneIcon = setIcon;
globalThis.hydrateIcons = hydrateIcons;
