const initial = {
    pendingNoteId: null,
    pendingFile: null,
    showEdit: true,
    showPreview: true,
    paneFocus: 'edit',
    syncScrollEnabled: true,
    editorStateRestored: false,
    currentFilename: '',
    fileList: [],
    specContent: '',
    isConnected: false,
    evtSource: null,
    loggedOut: false,
    editorDirty: false,
    debounceTimer: null,
    specVersion: 0,
    myNickname: '',
    isAdmin: false,
    isGuest: false,
    availableModels: [],
    currentThinking: 'off',
    currentStatus: 'disconnected',
    notes: [],
    currentNoteId: '',
    dirBrowserTargetInput: null,
    settingsNoteId: null,
    selectedNoteIcon: null,
    activeModel: '',
    lastContextTokens: null,
    editorInstance: null,
    currentAssistantEl: null,
    currentAssistantDiv: null,
    currentAssistantText: '',
    activeScrollSource: null,
    scrollTimeout: null,
    emojiPickerInitialized: false,
};

const listeners = new Set();
export const store = new Proxy(initial, {
    set(target, key, value) {
        if (Object.is(target[key], value)) return true;
        const previous = target[key];
        target[key] = value;
        listeners.forEach(listener => listener(key, value, previous));
        return true;
    },
});

export function setState(key, value) {
    store[key] = value;
}

export function updateState(values) {
    Object.entries(values).forEach(([key, value]) => { store[key] = value; });
}

export function subscribe(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

const aliases = {
    _pendingNoteId: 'pendingNoteId',
    _pendingFile: 'pendingFile',
    _editorStateRestored: 'editorStateRestored',
};

Object.keys(initial).forEach(key => {
    Object.defineProperty(globalThis, key, {
        configurable: false,
        get: () => store[key],
        set: value => { store[key] = value; },
    });
});
Object.entries(aliases).forEach(([alias, key]) => {
    Object.defineProperty(globalThis, alias, {
        configurable: false,
        get: () => store[key],
        set: value => { store[key] = value; },
    });
});
