import './state.js';
import { setIcon } from './icons.js';

function popover() {
    return document.getElementById('emoji-picker-popover');
}

function trigger() {
    return document.getElementById('emoji-picker-trigger');
}

// Grid cells stay non-button on purpose: emoji are legitimate content here, and
// the control language elsewhere is icon-only.
function choice(char, tags) {
    const cell = document.createElement('span');
    cell.className = 'emoji-btn' + (char === selectedNoteIcon ? ' active' : '');
    cell.role = 'option';
    cell.tabIndex = 0;
    cell.textContent = char;
    cell.title = tags;
    cell.setAttribute('aria-label', tags);
    cell.setAttribute('aria-selected', String(char === selectedNoteIcon));
    cell.dataset.action = 'select-emoji';
    cell.dataset.emoji = char;
    return cell;
}

function section(titleText) {
    const wrapper = document.createElement('div');
    wrapper.className = 'emoji-category-section';
    const title = document.createElement('div');
    title.className = 'emoji-category-title';
    title.textContent = titleText;
    const grid = document.createElement('div');
    grid.className = 'emoji-grid';
    grid.role = 'listbox';
    grid.setAttribute('aria-label', titleText);
    wrapper.append(title, grid);
    return { wrapper, grid };
}

globalThis.initEmojiPicker = function initEmojiPicker() {
    const tabs = document.getElementById('emoji-picker-tabs');
    const container = document.getElementById('emoji-categories-container');
    if (!tabs || !container) return;

    tabs.replaceChildren();
    container.replaceChildren();

    Object.keys(EMOJI_CATEGORIES).forEach(key => {
        const category = EMOJI_CATEGORIES[key];

        const tab = document.createElement('span');
        tab.className = 'emoji-tab-btn';
        tab.role = 'tab';
        tab.tabIndex = 0;
        tab.textContent = category.icon;
        tab.title = category.title;
        tab.setAttribute('aria-label', category.title);
        tab.dataset.action = 'emoji-category';
        tab.dataset.category = key;
        tabs.appendChild(tab);

        const { wrapper, grid } = section(category.title);
        wrapper.id = `category-sec-${key}`;
        category.list.forEach(emoji => grid.appendChild(choice(emoji.char, emoji.tags)));
        container.appendChild(wrapper);
    });

    if (emojiPickerInitialized) return;
    emojiPickerInitialized = true;

    document.getElementById('emoji-clear-btn')?.addEventListener('click', event => {
        event.stopPropagation();
        selectEmoji(null);
    });
    document.addEventListener('click', event => {
        if (popover()?.classList.contains('hidden') !== false) return;
        if (!event.target.closest?.('.emoji-picker-wrapper')) closeEmojiPicker();
    });
};

globalThis.scrollEmojiCategory = function scrollEmojiCategory(key) {
    document.getElementById(`category-sec-${key}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
};

globalThis.closeEmojiPicker = function closeEmojiPicker(restoreFocus) {
    popover()?.classList.add('hidden');
    trigger()?.setAttribute('aria-expanded', 'false');
    if (restoreFocus) trigger()?.focus();
};

globalThis.toggleEmojiPicker = function toggleEmojiPicker() {
    const node = popover();
    if (!node) return;
    const open = node.classList.toggle('hidden') === false;
    trigger()?.setAttribute('aria-expanded', String(open));
    if (!open) return;
    const search = document.getElementById('emoji-search-input');
    if (search) {
        search.value = '';
        filterEmojis('');
        search.focus();
    }
};

globalThis.renderNoteIconTrigger = function renderNoteIconTrigger() {
    const node = trigger();
    if (!node) return;
    if (selectedNoteIcon) node.textContent = selectedNoteIcon;
    else setIcon(node, 'folder');
};

globalThis.markSelectedEmoji = function markSelectedEmoji() {
    document.querySelectorAll('.emoji-btn').forEach(cell => {
        const active = cell.dataset.emoji === selectedNoteIcon;
        cell.classList.toggle('active', active);
        cell.setAttribute('aria-selected', String(active));
    });
};

const EMOJI_REGEX = /(\p{Extended_Pictographic}(?:\u200D\p{Extended_Pictographic}|[\uFE0E\uFE0F]|\p{Emoji_Modifier})*|[\u{1F1E6}-\u{1F1FF}]{2})/u;

export function extractCustomEmoji(query) {
    if (!query) return null;
    const match = query.match(EMOJI_REGEX);
    if (match) return match[0];
    const chars = [...query];
    if (chars.length === 1 && !/^[a-zA-Z0-9\s]$/.test(query)) {
        return chars[0];
    }
    return null;
}

function customOption(char) {
    const item = document.createElement('div');
    item.className = 'emoji-custom-preview' + (char === selectedNoteIcon ? ' active' : '');
    item.role = 'option';
    item.tabIndex = 0;
    item.setAttribute('aria-selected', String(char === selectedNoteIcon));
    item.dataset.action = 'select-emoji';
    item.dataset.emoji = char;

    const iconSpan = document.createElement('span');
    iconSpan.className = 'emoji-custom-char';
    iconSpan.textContent = char;

    const textSpan = document.createElement('span');
    textSpan.className = 'emoji-custom-label';
    textSpan.textContent = `Use "${char}" as icon`;

    item.append(iconSpan, textSpan);
    return item;
}

globalThis.selectEmoji = function selectEmoji(emoji) {
    selectedNoteIcon = emoji;
    renderNoteIconTrigger();
    markSelectedEmoji();
    closeEmojiPicker();
};

globalThis.filterEmojis = function filterEmojis(rawQuery) {
    const container = document.getElementById('emoji-categories-container');
    const tabs = document.getElementById('emoji-picker-tabs');
    if (!container) return;

    const query = (rawQuery || '').trim();
    if (!query) {
        if (tabs) tabs.style.display = 'flex';
        initEmojiPicker();
        markSelectedEmoji();
        return;
    }

    if (tabs) tabs.style.display = 'none';
    container.replaceChildren();

    const queryLower = query.toLowerCase();
    const customEmoji = extractCustomEmoji(query);

    if (customEmoji) {
        container.appendChild(customOption(customEmoji));
    }

    const { wrapper, grid } = section('Search Results');

    let count = 0;
    const seen = new Set();
    if (customEmoji) {
        seen.add(customEmoji);
        grid.appendChild(choice(customEmoji, `Custom: ${customEmoji}`));
        count++;
    }

    Object.values(EMOJI_CATEGORIES).forEach(category => {
        category.list.forEach(emoji => {
            if (seen.has(emoji.char)) return;
            const matchesChar = emoji.char === query;
            const matchesTag = emoji.tags.toLowerCase().includes(queryLower);
            if (!matchesChar && !matchesTag) return;
            seen.add(emoji.char);
            count++;
            grid.appendChild(choice(emoji.char, emoji.tags));
        });
    });

    if (count === 0) {
        const noResults = document.createElement('div');
        noResults.className = 'emoji-no-results';
        noResults.textContent = 'No matching emojis';
        grid.appendChild(noResults);
    }
    container.appendChild(wrapper);
};

globalThis.commitEmojiSearch = function commitEmojiSearch(rawQuery) {
    const query = (rawQuery || '').trim();
    if (!query) return;

    const customEmoji = extractCustomEmoji(query);
    if (customEmoji) {
        selectEmoji(customEmoji);
        return;
    }

    const firstOption = document.querySelector('#emoji-categories-container [data-action="select-emoji"]');
    if (firstOption && firstOption.dataset.emoji) {
        selectEmoji(firstOption.dataset.emoji);
    }
};
