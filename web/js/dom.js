export function clear(element) {
    element.replaceChildren();
    return element;
}

export function text(value) {
    return document.createTextNode(value == null ? '' : String(value));
}

export function element(tag, options = {}, children = []) {
    const node = document.createElement(tag);
    if (options.className) node.className = options.className;
    if (options.textContent != null) node.textContent = options.textContent;
    if (options.title) node.title = options.title;
    if (options.id) node.id = options.id;
    if (options.type) node.type = options.type;
    if (options.action) node.dataset.action = options.action;
    Object.entries(options.dataset || {}).forEach(([key, value]) => {
        node.dataset[key] = String(value);
    });
    Object.entries(options.attributes || {}).forEach(([key, value]) => {
        node.setAttribute(key, value);
    });
    children.forEach(child => node.append(child));
    return node;
}

export function setTrustedHtml(element, html) {
    element.innerHTML = html;
}

export function decodeHtml(value) {
    const textarea = document.createElement('textarea');
    textarea.innerHTML = value;
    return textarea.value;
}

// Forbid blocking browser window dialogs (window.alert, window.confirm, window.prompt)
if (typeof window !== 'undefined') {
    window.alert = function forbiddenAlert(msg) {
        console.warn('[Forbidden Dialog] window.alert() is forbidden:', msg);
        if (typeof globalThis.addSystemMessage === 'function') {
            globalThis.addSystemMessage('Notice: ' + msg);
        }
    };
    window.confirm = function forbiddenConfirm(msg) {
        console.warn('[Forbidden Dialog] window.confirm() is forbidden:', msg);
        return false;
    };
    window.prompt = function forbiddenPrompt(msg) {
        console.warn('[Forbidden Dialog] window.prompt() is forbidden:', msg);
        return null;
    };
}
