import './state.js';
// --- Panel Toggle ---

// The chat dock runs down the right edge on wide screens and across the bottom
// on portrait tablets. Sizes are stored per axis so rotating a device never
// feeds a saved width back in as a height.
function axisOf(side) {
    return side === 'right' ? chatAxis() : 'x';
}

function sizeKey(side) {
    return 'rune_panel_' + side + (axisOf(side) === 'y' ? '_h' : '');
}

function sizeProp(side) {
    return axisOf(side) === 'y' ? 'height' : 'width';
}

function measure(panel, side) {
    return axisOf(side) === 'y' ? panel.offsetHeight : panel.offsetWidth;
}

function clearPanelSize(panel) {
    panel.style.width = '';
    panel.style.height = '';
}

globalThis.applyPanelSize = function applyPanelSize(panel, side) {
    clearPanelSize(panel);
    if (panel.classList.contains('collapsed')) return;
    let saved = null;
    try { saved = localStorage.getItem(sizeKey(side)); } catch {}
    if (saved) panel.style[sizeProp(side)] = saved + 'px';
};

globalThis.savePanelSize = function savePanelSize(panel, side, value) {
    try { localStorage.setItem(sizeKey(side), value ?? measure(panel, side)); } catch {}
};

globalThis.togglePanel = function togglePanel(side) {
    const panel = document.getElementById('panel-' + side);
    // When center is hidden (both Edit+Preview off), chat panel cannot collapse
    if (side === 'right') {
        const center = document.getElementById('panel-center');
        if (center && center.classList.contains('hidden')) return;
    }
    const wasCollapsed = panel.classList.contains('collapsed');
    if (!wasCollapsed) savePanelSize(panel, side);
    panel.classList.toggle('collapsed');
    updateToggleIcon(panel, side);
    applyPanelSize(panel, side);

    // Persist collapsed state
    try { localStorage.setItem('rune_panel_' + side + '_collapsed', panel.classList.contains('collapsed') ? '1' : '0'); } catch {}
    if (side === 'right') setToggleState(btnChat, !panel.classList.contains('collapsed'));
};

globalThis.toggleChatPanel = function toggleChatPanel() {
    togglePanel('right');
};

globalThis.updateToggleIcon = function updateToggleIcon(panel, side) {
    const icon = panel.querySelector('.toggle-icon');
    const handle = panel.querySelector('.resize-handle');
    const collapsed = panel.classList.contains('collapsed');
    if (handle) handle.setAttribute('aria-expanded', String(!collapsed));
    if (!icon) return;
    if (axisOf(side) === 'y') icon.textContent = collapsed ? '⌃' : '⌄';
    else if (side === 'left')  icon.textContent = collapsed ? '›' : '‹';
    else                       icon.textContent = collapsed ? '‹' : '›';
}

// --- Utilities ---
globalThis.escapeHtml = function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
};

globalThis.fallbackCopy = function fallbackCopy(text, onSuccess) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    try {
        document.execCommand('copy');
        if (onSuccess) onSuccess();
    } catch (err) {
        console.error('Copy failed:', err);
    }
    document.body.removeChild(ta);
}

// --- Keyboard shortcuts ---
chatInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
    }
});

document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key === 'E') {
        e.preventDefault();
        swapEditorPreview();
    }
});

document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === 'Enter') {
        e.preventDefault();
        chatInput.focus();
        sendMessage();
    }
});

// --- Panel Resize ---
globalThis.initPanelResize = function initPanelResize() {
    ['left', 'right'].forEach(side => {
        const panel = document.getElementById('panel-' + side);
        // Restore collapsed state
        const wasCollapsed = localStorage.getItem('rune_panel_' + side + '_collapsed');
        if (wasCollapsed === '1' && !panel.classList.contains('collapsed')) {
            panel.classList.add('collapsed');
        } else if (wasCollapsed === '0' && panel.classList.contains('collapsed')) {
            panel.classList.remove('collapsed');
        }
        applyPanelSize(panel, side);
        updateToggleIcon(panel, side);
    });
    setupResizeHandle('resize-left',  'panel-left',  'left');
    setupResizeHandle('resize-right', 'panel-right', 'right');
};

// Re-seats the chat dock after a rotation: the inline size belongs to the axis
// it was measured on, so it is dropped and the new axis' own value applied.
globalThis.syncPanelAxes = function syncPanelAxes() {
    ['left', 'right'].forEach(side => {
        const panel = document.getElementById('panel-' + side);
        if (!panel) return;
        applyPanelSize(panel, side);
        updateToggleIcon(panel, side);
    });
};

globalThis.setupResizeHandle = function setupResizeHandle(handleId, panelId, side) {
    const handle = document.getElementById(handleId);
    const panel  = document.getElementById(panelId);
    if (!handle || !panel) return;

    let start, startSize, axis, target, moved = false;

    handle.addEventListener('keydown', event => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        togglePanel(side);
    });

    handle.addEventListener('mousedown', e => {
        e.preventDefault();
        axis = axisOf(side);
        start = axis === 'y' ? e.clientY : e.clientX;
        startSize = measure(panel, side);
        target = startSize;
        moved  = false;
        handle.classList.add('dragging');
        // The panel animates its size, so it must not lag the pointer mid-drag.
        panel.classList.add('resizing');
        document.body.style.userSelect = 'none';

        // Only set the resize cursor when not collapsed
        if (!panel.classList.contains('collapsed')) {
            document.body.style.cursor = axis === 'y' ? 'row-resize' : 'col-resize';
        }

        function onMove(e) {
            if (panel.classList.contains('collapsed')) return;
            const position = axis === 'y' ? e.clientY : e.clientX;
            const dist = Math.abs(position - start);
            if (dist < 4) return; // dead zone — too small to be a drag
            moved = true;
            // Left panel grows toward the pointer; the chat dock grows away from it.
            const delta = side === 'left' ? position - start : start - position;
            const style = getComputedStyle(panel);
            const min = parseInt(axis === 'y' ? style.minHeight : style.minWidth) || 160;
            const max = parseInt(axis === 'y' ? style.maxHeight : style.maxWidth) || 600;
            target = Math.max(min, Math.min(max, startSize + delta));
            panel.style[sizeProp(side)] = target + 'px';
        }

        function onUp() {
            handle.classList.remove('dragging');
            panel.classList.remove('resizing');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            document.removeEventListener('mousemove', onMove);
            document.removeEventListener('mouseup', onUp);

            if (!moved) {
                // Pure click → toggle collapse
                togglePanel(side);
            } else {
                // The drag target, not a measurement: a size mid-animation is short.
                savePanelSize(panel, side, target);
            }
        }

        document.addEventListener('mousemove', onMove);
        document.addEventListener('mouseup', onUp);
    });
}
