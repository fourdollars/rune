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
    paint(state, STATUS_EMOJI[state] || 'dot', state);
};

globalThis.setToolStatus = function setToolStatus(toolName) {
    paint('tool', STATUS_EMOJI.tool, `tool: ${toolName}`);
};

globalThis.clearToolStatus = function clearToolStatus() {
    // Revert to thinking (tool ended, waiting for next LLM response)
    setStatus('thinking');
};
