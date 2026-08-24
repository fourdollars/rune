import './state.js';
import { setIcon } from './icons.js';

// Kept as STATUS_EMOJI for the asset contract; the values are icon names now.
globalThis.STATUS_EMOJI = {
    idle: 'dot',
    typing: 'pencil',
    thinking: 'command',
    tool: 'settings',
    disconnected: 'dot',
};

const MIN_TOOL_DISPLAY_MS = 600;
let toolStartTime = 0;
let clearToolTimer = null;

function paint(state, name, label) {
    ['status-indicator', 'mobile-status'].forEach(id => {
        const node = document.getElementById(id);
        if (!node) return;
        node.className = `status ${state}`;
        node.title = label;
        node.setAttribute('aria-label', `Status: ${label}`);
        setIcon(node, name);
    });
}

globalThis.setStatus = function setStatus(state) {
    if (state && typeof state === 'string' && state.startsWith('tool:')) {
        setToolStatus(state.slice(5));
        return;
    }
    if (clearToolTimer) {
        clearTimeout(clearToolTimer);
        clearToolTimer = null;
    }
    currentStatus = state;
    paint(state, STATUS_EMOJI[state] || 'dot', state);
};

globalThis.setToolStatus = function setToolStatus(toolName) {
    if (clearToolTimer) {
        clearTimeout(clearToolTimer);
        clearToolTimer = null;
    }
    currentStatus = 'tool';
    toolStartTime = Date.now();
    paint('tool', STATUS_EMOJI.tool, `tool: ${toolName}`);
};

globalThis.clearToolStatus = function clearToolStatus() {
    if (currentStatus !== 'tool') {
        setStatus('thinking');
        return;
    }
    const elapsed = Date.now() - toolStartTime;
    if (elapsed < MIN_TOOL_DISPLAY_MS) {
        if (clearToolTimer) clearTimeout(clearToolTimer);
        clearToolTimer = setTimeout(() => {
            clearToolTimer = null;
            if (currentStatus === 'tool') {
                setStatus('thinking');
            }
        }, MIN_TOOL_DISPLAY_MS - elapsed);
    } else {
        setStatus('thinking');
    }
};
