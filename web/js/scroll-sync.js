import './state.js';
// --- Edit ↔ Preview scroll sync ---
globalThis.toggleSyncScroll = function toggleSyncScroll() {
    syncScrollEnabled = !syncScrollEnabled;
    setToggleState(document.getElementById('btn-sync-scroll'), syncScrollEnabled);
    try {
        localStorage.setItem('rune_sync_scroll', syncScrollEnabled ? '1' : '0');
    } catch {}
}


globalThis.handleEditorScroll = function handleEditorScroll() {
    if (!syncScrollEnabled || !showPreview || !showEdit || !editorInstance) return;
    if (activeScrollSource === 'preview') return;
    
    activeScrollSource = 'editor';
    clearTimeout(scrollTimeout);

    const scrollInfo = editorInstance.getScrollInfo();
    const topLine = editorInstance.lineAtHeight(scrollInfo.top, 'local');

    const elements = Array.from(previewContainer.querySelectorAll('[data-line]'));
    if (elements.length === 0) {
        activeScrollSource = null;
        return;
    }

    let low = 0;
    let high = elements.length - 1;
    let targetIdx = 0;

    while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        const line = parseInt(elements[mid].dataset.line, 10);
        if (line <= topLine) {
            targetIdx = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }

    const elLow = elements[targetIdx];
    const elHigh = elements[targetIdx + 1];
    
    const lineLow = parseInt(elLow.dataset.line, 10);
    const offsetLow = elLow.offsetTop;
    let targetScrollTop = offsetLow;

    if (elHigh) {
        const lineHigh = parseInt(elHigh.dataset.line, 10);
        const offsetHigh = elHigh.offsetTop;
        const ratio = (topLine - lineLow) / (lineHigh - lineLow || 1);
        targetScrollTop = offsetLow + ratio * (offsetHigh - offsetLow);
    }

    previewContainer.scrollTop = targetScrollTop;

    scrollTimeout = setTimeout(() => { activeScrollSource = null; }, 100);
};

globalThis.handlePreviewScroll = function handlePreviewScroll() {
    if (!syncScrollEnabled || !showPreview || !showEdit || !editorInstance) return;
    if (activeScrollSource === 'editor') return;
    
    activeScrollSource = 'preview';
    clearTimeout(scrollTimeout);

    const scrollTop = previewContainer.scrollTop;
    const elements = Array.from(previewContainer.querySelectorAll('[data-line]'));
    if (elements.length === 0) {
        activeScrollSource = null;
        return;
    }

    let low = 0;
    let high = elements.length - 1;
    let targetIdx = 0;

    while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        const offset = elements[mid].offsetTop;
        if (offset <= scrollTop) {
            targetIdx = mid;
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }

    const elLow = elements[targetIdx];
    const elHigh = elements[targetIdx + 1];
    
    const lineLow = parseInt(elLow.dataset.line, 10);
    const offsetLow = elLow.offsetTop;
    let targetEditorLine = lineLow;

    if (elHigh) {
        const lineHigh = parseInt(elHigh.dataset.line, 10);
        const offsetHigh = elHigh.offsetTop;
        const ratio = (scrollTop - offsetLow) / (offsetHigh - offsetLow || 1);
        targetEditorLine = lineLow + ratio * (lineHigh - lineLow);
    }
    targetEditorLine = Math.round(targetEditorLine);
    targetEditorLine = Math.max(0, Math.min(editorInstance.lineCount() - 1, targetEditorLine));

    const editorScrollTop = editorInstance.heightAtLine(targetEditorLine, 'local');
    editorInstance.scrollTo(null, editorScrollTop);

    scrollTimeout = setTimeout(() => { activeScrollSource = null; }, 100);
};

globalThis.initPreviewScrollSync = function initPreviewScrollSync() {
    previewContainer.addEventListener('scroll', handlePreviewScroll);
}
