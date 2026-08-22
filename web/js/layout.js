import './state.js';
// --- Spec Editor ---
globalThis.setToggleState = function setToggleState(button, on) {
    if (!button) return;
    button.classList.toggle('active', on);
    button.setAttribute('aria-pressed', String(on));
};

globalThis.updateEditorVisibility = function updateEditorVisibility(fileCount) {
    const btnEdit = document.getElementById('btn-edit');
    const btnPreview = document.getElementById('btn-preview');
    if (fileCount === 0) {
        // No markdown files: hide buttons and collapse editor
        btnEdit.classList.add('hidden');
        btnPreview.classList.add('hidden');
        showEdit = false;
        showPreview = false;
    } else {
        // Has files: show buttons; restore from localStorage only on first call
        btnEdit.classList.remove('hidden');
        btnPreview.classList.remove('hidden');
        if (!_editorStateRestored) {
            _editorStateRestored = true;
            try {
                const se = localStorage.getItem('rune_show_edit');
                const sp = localStorage.getItem('rune_show_preview');
                showEdit    = se !== null ? se === '1' : true;
                showPreview = sp !== null ? sp === '1' : true;
            } catch {}
        }
    }
    applyPanelLayout();
};

globalThis.applyPanelLayout = function applyPanelLayout() {
    const center     = document.getElementById('panel-center');
    const centerBody = document.getElementById('center-body');

    // Editor visibility
    editorContainer.classList.toggle('hidden', !showEdit);
    setToggleState(btnEdit, showEdit);

    // Preview visibility
    previewContainer.classList.toggle('hidden', !showPreview);
    setToggleState(btnPreview, showPreview);
    if (showPreview) renderPreview();

    // Split layout: side-by-side when both on
    if (showEdit && showPreview) {
        centerBody.classList.add('split-view');
        editorContainer.style.width  = '';
        previewContainer.style.width = '';
    } else {
        centerBody.classList.remove('split-view');
        editorContainer.style.width  = '';
        previewContainer.style.width = '';
    }
    // Consumed only below 1280px, where one pane must win over the other.
    centerBody.classList.toggle('prefer-preview', paneFocus === 'preview');

    // Show split-title-bar whenever any panel is visible (editor or preview)
    const splitTitle = document.getElementById('split-title-bar');
    if (splitTitle) {
        splitTitle.style.display = (showEdit || showPreview) ? 'flex' : 'none';
    }

    // Both off → hide center so chat expands to fill
    if (!showEdit && !showPreview) {
        center.classList.add('hidden');
    } else {
        center.classList.remove('hidden');
    }

    // When center is hidden, force chat panel open
    const panelRight = document.getElementById('panel-right');
    if (!showEdit && !showPreview) {
        if (panelRight && panelRight.classList.contains('collapsed')) {
            panelRight.classList.remove('collapsed');
            updateToggleIcon(panelRight, 'right');
            try {
                const saved = localStorage.getItem('rune_panel_right');
                panelRight.style.width = saved ? saved + 'px' : '';
            } catch {}
        }
        // Hide the right panel resize handle arrow (no collapse allowed)
        const rh = document.getElementById('resize-right');
        if (rh) rh.style.pointerEvents = 'none';
        if (rh) rh.querySelector('.toggle-icon') && (rh.querySelector('.toggle-icon').style.display = 'none');
    } else {
        const rh = document.getElementById('resize-right');
        if (rh) rh.style.pointerEvents = '';
        const icon = rh && rh.querySelector('.toggle-icon');
        if (icon) icon.style.display = '';
    }

    // Persist (only after initial state has been restored from localStorage)
    if (_editorStateRestored) {
        try {
            localStorage.setItem('rune_show_edit',    showEdit    ? '1' : '0');
            localStorage.setItem('rune_show_preview', showPreview ? '1' : '0');
        } catch {}
    }
    // Update split-title bar and current filename
    updateDocTitle(currentFilename);
    syncPaneSwitcher();
    layoutEditorToolbar();
};

globalThis.toggleEdit = function toggleEdit() {
    showEdit = !showEdit;
    if (showEdit) paneFocus = 'edit';
    applyPanelLayout();
};

globalThis.togglePreview = function togglePreview() {
    showPreview = !showPreview;
    if (showPreview) paneFocus = 'preview';
    applyPanelLayout();
}

globalThis.swapEditorPreview = function swapEditorPreview() {
    const toPreview = showEdit && !showPreview;
    showEdit = !toPreview;
    showPreview = toPreview;
    paneFocus = toPreview ? 'preview' : 'edit';
    applyPanelLayout();
}

// Legacy alias (used internally for keyboard shortcut etc.)
globalThis.setMode = function setMode(mode) {
    if (mode === 'edit')    { showEdit = true;  showPreview = false; paneFocus = 'edit'; }
    else                    { showEdit = false; showPreview = true;  paneFocus = 'preview'; }
    applyPanelLayout();
}
