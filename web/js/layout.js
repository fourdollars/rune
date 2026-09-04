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
                showEdit    = se !== null ? se === '1' : false;
                showPreview = sp !== null ? sp === '1' : true;
            } catch {}
        }
    }
    applyPanelLayout();
};

// showEdit/showPreview are the user's intent and survive every breakpoint;
// the tablet tier renders only one document pane, so what is on screen is
// derived here rather than by mutating the intent on resize or rotation.
globalThis.documentPanesExclusive = function documentPanesExclusive() {
    return isTabletViewport();
};

globalThis.resolveDocumentPanes = function resolveDocumentPanes() {
    if (documentPanesExclusive() && showEdit && showPreview) {
        return paneFocus === 'preview'
            ? { edit: false, preview: true }
            : { edit: true, preview: false };
    }
    return { edit: showEdit, preview: showPreview };
};

globalThis.applyPanelLayout = function applyPanelLayout() {
    const center     = document.getElementById('panel-center');
    const centerBody = document.getElementById('center-body');
    const view       = resolveDocumentPanes();

    // Editor visibility
    const editorWasHidden = editorContainer.classList.contains('hidden');
    editorContainer.classList.toggle('hidden', !view.edit);
    setToggleState(btnEdit, view.edit);

    // Preview visibility
    previewContainer.classList.toggle('hidden', !view.preview);
    setToggleState(btnPreview, view.preview);
    if (view.preview) renderPreview();

    // Split layout: side-by-side when both on
    centerBody.classList.toggle('split-view', view.edit && view.preview);
    editorContainer.style.width  = '';
    previewContainer.style.width = '';

    // Show split-title-bar whenever any panel is visible (editor or preview)
    const splitTitle = document.getElementById('split-title-bar');
    if (splitTitle) {
        splitTitle.style.display = (view.edit || view.preview) ? 'flex' : 'none';
    }

    // Both off → hide center so chat expands to fill
    const centerHidden = !view.edit && !view.preview;
    center.classList.toggle('hidden', centerHidden);
    document.body.classList.toggle('center-hidden', centerHidden);

    // When center is hidden, force chat panel open
    const panelRight = document.getElementById('panel-right');
    if (centerHidden) {
        if (panelRight && panelRight.classList.contains('collapsed')) {
            panelRight.classList.remove('collapsed');
            updateToggleIcon(panelRight, 'right');
            applyPanelSize(panelRight, 'right');
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
    setToggleState(btnChat, panelRight && !panelRight.classList.contains('collapsed'));

    // Persist (only after initial state has been restored from localStorage)
    if (_editorStateRestored) {
        try {
            localStorage.setItem('rune_show_edit',    showEdit    ? '1' : '0');
            localStorage.setItem('rune_show_preview', showPreview ? '1' : '0');
        } catch {}
    }
    // Update split-title bar and current filename
    updateDocTitle(currentFilename);
    // CodeMirror cannot lay out inside a display:none container, so any value
    // set while the editor was hidden is still on screen as the previous file.
    if (view.edit && editorWasHidden && editorInstance) editorInstance.refresh();
    syncPaneSwitcher();
    layoutEditorToolbar();
};

// In the exclusive tier the two buttons are a radio pair: picking a pane only
// moves the focus, so the other pane's intent survives for the wider tiers.
function selectDocumentPane(pane) {
    if (pane === 'edit') showEdit = true; else showPreview = true;
    paneFocus = pane;
    applyPanelLayout();
}

globalThis.toggleEdit = function toggleEdit() {
    if (documentPanesExclusive()) { selectDocumentPane('edit'); return; }
    showEdit = !showEdit;
    if (showEdit) paneFocus = 'edit';
    applyPanelLayout();
};

globalThis.togglePreview = function togglePreview() {
    if (documentPanesExclusive()) { selectDocumentPane('preview'); return; }
    showPreview = !showPreview;
    if (showPreview) paneFocus = 'preview';
    applyPanelLayout();
}

globalThis.swapEditorPreview = function swapEditorPreview() {
    if (documentPanesExclusive()) {
        selectDocumentPane(resolveDocumentPanes().preview ? 'edit' : 'preview');
        return;
    }
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
