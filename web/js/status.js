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
const toolStartTimes = new Map();
const clearToolTimers = new Map();

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

function refreshSessionModalIfOpen() {
    const sessionModal = document.getElementById('session-modal');
    if (sessionModal && !sessionModal.classList.contains('hidden')) {
        const searchInput = document.getElementById('session-modal-search-input');
        if (typeof renderSessionModal === 'function') {
            renderSessionModal(searchInput?.value || '');
        }
    }
}

globalThis.getSessionStatus = function getSessionStatus(sessionId) {
    const sess = sessionId || currentSessionId || 'main';
    if (!sessionStatuses || !(sessionStatuses instanceof Map)) {
        sessionStatuses = new Map();
    }
    return sessionStatuses.get(sess) || 'idle';
};

globalThis.setSessionStatus = function setSessionStatus(sessionId, state) {
    const sess = sessionId || currentSessionId || 'main';
    if (state && typeof state === 'string' && state.startsWith('tool:')) {
        setSessionToolStatus(sess, state.slice(5));
        return;
    }

    if (clearToolTimers.has(sess)) {
        clearTimeout(clearToolTimers.get(sess));
        clearToolTimers.delete(sess);
    }

    if (!sessionStatuses || !(sessionStatuses instanceof Map)) {
        sessionStatuses = new Map();
    }

    if (!state || state === 'idle') {
        sessionStatuses.delete(sess);
    } else {
        sessionStatuses.set(sess, state);
    }

    if (sess === (currentSessionId || 'main')) {
        currentStatus = state || 'idle';
        paint(currentStatus, STATUS_EMOJI[currentStatus] || 'dot', currentStatus);
    }

    refreshSessionModalIfOpen();
};

globalThis.setSessionToolStatus = function setSessionToolStatus(sessionId, toolName) {
    const sess = sessionId || currentSessionId || 'main';
    if (clearToolTimers.has(sess)) {
        clearTimeout(clearToolTimers.get(sess));
        clearToolTimers.delete(sess);
    }

    if (!sessionStatuses || !(sessionStatuses instanceof Map)) {
        sessionStatuses = new Map();
    }

    const stateKey = `tool:${toolName}`;
    sessionStatuses.set(sess, stateKey);
    toolStartTimes.set(sess, Date.now());

    if (sess === (currentSessionId || 'main')) {
        currentStatus = 'tool';
        paint('tool', STATUS_EMOJI.tool, `tool: ${toolName}`);
    }

    refreshSessionModalIfOpen();
};

globalThis.clearSessionToolStatus = function clearSessionToolStatus(sessionId) {
    const sess = sessionId || currentSessionId || 'main';
    const current = getSessionStatus(sess);
    if (!current || !current.startsWith('tool')) {
        return;
    }

    const startTime = toolStartTimes.get(sess) || 0;
    const elapsed = Date.now() - startTime;

    if (elapsed < MIN_TOOL_DISPLAY_MS) {
        if (clearToolTimers.has(sess)) {
            clearTimeout(clearToolTimers.get(sess));
        }
        const timer = setTimeout(() => {
            clearToolTimers.delete(sess);
            if (getSessionStatus(sess).startsWith('tool')) {
                setSessionStatus(sess, 'thinking');
            }
        }, MIN_TOOL_DISPLAY_MS - elapsed);
        clearToolTimers.set(sess, timer);
    } else {
        setSessionStatus(sess, 'thinking');
    }
};

globalThis.updateCurrentSessionStatus = function updateCurrentSessionStatus() {
    const sess = currentSessionId || 'main';
    const status = getSessionStatus(sess);
    if (status && status.startsWith('tool:')) {
        const toolName = status.slice(5);
        currentStatus = 'tool';
        paint('tool', STATUS_EMOJI.tool, `tool: ${toolName}`);
    } else {
        currentStatus = status || 'idle';
        paint(currentStatus, STATUS_EMOJI[currentStatus] || 'dot', currentStatus);
    }
};

globalThis.setStatus = function setStatus(state, sessionId) {
    setSessionStatus(sessionId || currentSessionId || 'main', state);
};

globalThis.setToolStatus = function setToolStatus(toolName, sessionId) {
    setSessionToolStatus(sessionId || currentSessionId || 'main', toolName);
};

globalThis.clearToolStatus = function clearToolStatus(sessionId) {
    clearSessionToolStatus(sessionId || currentSessionId || 'main');
};
