import './state.js';
import { api } from './api.js';

let cachedSkills = [];
let activeInput = null;
let activeAutocompleteEl = null;
let activeTrigger = null; // '+' or '@'
let activeRange = null; // { start, end, query }
let selectedIndex = 0;
let filteredItems = [];

export async function fetchSkills() {
    try {
        const res = await api('skills', undefined, 'GET');
        if (res && res.ok && res.data && res.data.skills) {
            cachedSkills = res.data.skills;
        } else if (res && res.skills) {
            cachedSkills = res.skills;
        }
    } catch (e) {
        console.warn('Failed to fetch skills:', e);
    }
}

export function getSkills() {
    return cachedSkills;
}

function detectTrigger(input) {
    if (!input || input.selectionStart == null) return null;
    const cursor = input.selectionStart;
    const text = input.value.slice(0, cursor);

    // Look backwards from cursor to find a trigger (+ or @)
    // Must be preceded by start of string, newline, or whitespace
    const match = /(?:^|\s)([+@])([a-zA-Z0-9_\-\.]*)$/.exec(text);
    if (!match) return null;

    const trigger = match[1];
    const query = match[2];
    const matchIndex = match.index + (match[0].length - trigger.length - query.length);

    return {
        trigger,
        query,
        start: matchIndex,
        end: cursor,
    };
}

export function updateAutocomplete(inputEl = null, autocompleteEl = null) {
    const input = inputEl || activeInput || document.getElementById('chat-input');
    const el = autocompleteEl || activeAutocompleteEl || document.getElementById('chat-autocomplete');
    if (!input || !el) return;

    activeInput = input;
    activeAutocompleteEl = el;

    const range = detectTrigger(input);
    if (!range) {
        hideAutocomplete();
        return;
    }

    activeTrigger = range.trigger;
    activeRange = range;
    const queryLower = range.query.toLowerCase();

    if (activeTrigger === '+') {
        filteredItems = cachedSkills
            .filter(s => (s.name || '').toLowerCase().includes(queryLower))
            .map(s => ({
                type: 'skill',
                name: s.name,
                description: s.description || '',
                insertText: `+${s.name} `,
            }));
    } else if (activeTrigger === '@') {
        const files = Array.isArray(globalThis.fileList)
            ? globalThis.fileList
            : (Array.isArray(fileList) ? fileList : []);
        filteredItems = files
            .filter(f => f.toLowerCase().includes(queryLower))
            .map(f => ({
                type: 'file',
                name: f,
                description: '',
                insertText: `@${f} `,
            }));
    } else {
        filteredItems = [];
    }

    if (filteredItems.length === 0) {
        hideAutocomplete();
        return;
    }

    if (selectedIndex >= filteredItems.length) {
        selectedIndex = 0;
    }

    renderAutocomplete();
}

function renderAutocomplete() {
    const el = activeAutocompleteEl || document.getElementById('chat-autocomplete');
    if (!el) return;

    el.replaceChildren();
    filteredItems.forEach((item, idx) => {
        const row = document.createElement('div');
        row.className = 'chat-autocomplete-item' + (idx === selectedIndex ? ' selected' : '');
        row.setAttribute('role', 'option');
        row.setAttribute('aria-selected', idx === selectedIndex ? 'true' : 'false');

        const prefix = document.createElement('span');
        prefix.className = 'chat-autocomplete-prefix';
        prefix.textContent = item.type === 'skill' ? '+' : '@';
        row.appendChild(prefix);

        const name = document.createElement('span');
        name.className = 'chat-autocomplete-name';
        name.textContent = item.name;
        row.appendChild(name);

        if (item.description) {
            const desc = document.createElement('span');
            desc.className = 'chat-autocomplete-desc';
            desc.textContent = item.description;
            row.appendChild(desc);
        }

        row.addEventListener('mousedown', (e) => {
            e.preventDefault(); // Prevent blur
            applyItem(idx);
        });

        el.appendChild(row);
    });

    el.classList.remove('hidden');
    const selectedEl = el.children[selectedIndex];
    if (selectedEl && typeof selectedEl.scrollIntoView === 'function') {
        selectedEl.scrollIntoView({ block: 'nearest' });
    }
}

export function hideAutocomplete() {
    const el = activeAutocompleteEl || document.getElementById('chat-autocomplete');
    if (el) {
        el.classList.add('hidden');
        el.replaceChildren();
    }
    const cronEl = document.getElementById('cron-prompt-autocomplete');
    if (cronEl && cronEl !== el) {
        cronEl.classList.add('hidden');
        cronEl.replaceChildren();
    }
    activeTrigger = null;
    activeRange = null;
    filteredItems = [];
    selectedIndex = 0;
}

export function isAutocompleteOpen() {
    const el = activeAutocompleteEl || document.getElementById('chat-autocomplete');
    return el && !el.classList.contains('hidden') && filteredItems.length > 0;
}

export function applyItem(idx) {
    if (idx < 0 || idx >= filteredItems.length) return;
    const item = filteredItems[idx];
    const input = activeInput || document.getElementById('chat-input');
    if (!input || !activeRange) return;

    const before = input.value.slice(0, activeRange.start);
    const after = input.value.slice(activeRange.end);
    input.value = before + item.insertText + after;

    const newCursor = before.length + item.insertText.length;
    if (typeof input.setSelectionRange === 'function') {
        input.setSelectionRange(newCursor, newCursor);
    }
    if (typeof input.focus === 'function') {
        input.focus();
    }

    hideAutocomplete();
}

export function handleAutocompleteKeydown(e) {
    if (!isAutocompleteOpen()) return false;

    if (e.key === 'ArrowDown') {
        selectedIndex = (selectedIndex + 1) % filteredItems.length;
        renderAutocomplete();
        e.preventDefault();
        e.stopPropagation();
        return true;
    }
    if (e.key === 'ArrowUp') {
        selectedIndex = (selectedIndex - 1 + filteredItems.length) % filteredItems.length;
        renderAutocomplete();
        e.preventDefault();
        e.stopPropagation();
        return true;
    }
    if (e.key === 'Enter' || e.key === 'Tab') {
        applyItem(selectedIndex);
        e.preventDefault();
        e.stopPropagation();
        return true;
    }
    if (e.key === 'Escape') {
        hideAutocomplete();
        e.preventDefault();
        e.stopPropagation();
        return true;
    }
    return false;
}

export function attachAutocomplete(inputEl, autocompleteEl) {
    if (!inputEl || !autocompleteEl) return;

    inputEl.addEventListener('keydown', (e) => {
        if (activeInput === inputEl && isAutocompleteOpen()) {
            handleAutocompleteKeydown(e);
        }
    }, true);

    const onUpdate = () => {
        activeInput = inputEl;
        activeAutocompleteEl = autocompleteEl;
        updateAutocomplete(inputEl, autocompleteEl);
    };

    inputEl.addEventListener('input', onUpdate);
    inputEl.addEventListener('keyup', (e) => {
        if (['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(e.key)) return;
        onUpdate();
    });
    inputEl.addEventListener('click', onUpdate);
    inputEl.addEventListener('blur', () => {
        setTimeout(() => {
            if (activeInput === inputEl) {
                hideAutocomplete();
            }
        }, 150);
    });
}

export function initAutocomplete() {
    const chatInput = document.getElementById('chat-input');
    const chatAutocomplete = document.getElementById('chat-autocomplete');
    if (chatInput && chatAutocomplete) {
        attachAutocomplete(chatInput, chatAutocomplete);
    }

    const cronPrompt = document.getElementById('cron-job-prompt');
    const cronAutocomplete = document.getElementById('cron-prompt-autocomplete');
    if (cronPrompt && cronAutocomplete) {
        attachAutocomplete(cronPrompt, cronAutocomplete);
    }

    fetchSkills();
}

globalThis.initAutocomplete = initAutocomplete;
globalThis.attachAutocomplete = attachAutocomplete;
globalThis.fetchSkills = fetchSkills;
globalThis.updateAutocomplete = updateAutocomplete;
globalThis.hideAutocomplete = hideAutocomplete;
globalThis.handleAutocompleteKeydown = handleAutocompleteKeydown;
globalThis.applyItem = applyItem;
