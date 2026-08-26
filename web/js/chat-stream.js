import './state.js';
globalThis.appendToLastAssistant = function appendToLastAssistant(token) {
    if (!currentAssistantEl) {
        const div = document.createElement('div');
        div.className = 'chat-msg assistant';

        const sender = document.createElement('div');
        sender.className = 'sender';
        const nameSpan = document.createElement('span');
        nameSpan.textContent = 'ᚱ';
        const timeSpan = document.createElement('span');
        timeSpan.className = 'msg-time';
        timeSpan.textContent = fmtTime(null);
        sender.appendChild(nameSpan);
        sender.appendChild(timeSpan);

        const body = document.createElement('div');
        body.className = 'body';

        div.appendChild(sender);
        div.appendChild(body);
        chatMessages.appendChild(div);
        currentAssistantEl = body;
        currentAssistantDiv = div;
        currentAssistantText = '';
    }
    currentAssistantText += token;
    if (typeof marked !== 'undefined') {
        currentAssistantEl.replaceChildren(markdownFragment(currentAssistantText));
    } else {
        currentAssistantEl.textContent = currentAssistantText;
    }
    chatMessages.scrollTop = chatMessages.scrollHeight;
};

globalThis.finalizeAssistantMessage = function finalizeAssistantMessage() {
    if (currentAssistantEl && typeof marked !== 'undefined') {
        currentAssistantEl.replaceChildren(markdownFragment(currentAssistantText));
        renderChatMath(currentAssistantEl);
        if (typeof renderMermaidBlocks === 'function') renderMermaidBlocks(currentAssistantEl);
    }
    currentAssistantEl = null;
    currentAssistantText = '';
    currentAssistantDiv = null;
};

globalThis.attachMetaToLastAssistant = function attachMetaToLastAssistant(model, tokIn, tokOut, ctxTokens, ctxWindow, steps, toolCalls, thinking) {
    const target = currentAssistantDiv || chatMessages.querySelector('.chat-msg.assistant:last-child');
    if (!target) return;
    const sender = target.querySelector('.sender');
    if (!sender) return;
    // Remove old meta if any
    const oldMeta = sender.querySelector('.msg-meta');
    if (oldMeta) oldMeta.remove();
    // Model stays in header
    if (model) {
        const meta = document.createElement('span');
        meta.className = 'msg-meta';
        meta.textContent = (thinking && thinking !== 'off') ? `${model} ${thinking}` : model;
        const timeEl = sender.querySelector('.msg-time');
        if (timeEl) sender.insertBefore(meta, timeEl);
        else sender.appendChild(meta);
    }
    // Run stats go at the tail of the message body
    const totalTok = (tokIn||0) + (tokOut||0);
    if (steps || totalTok || toolCalls) {
        const body = target.querySelector('.body');
        if (body) {
            // Remove old stats footer if any
            const oldStats = body.querySelector('.run-stats');
            if (oldStats) oldStats.remove();
            const stats = document.createElement('div');
            stats.className = 'run-stats';
            stats.textContent = `${steps||0} steps · ${totalTok} tokens · ${toolCalls||0} tool calls`;
            body.appendChild(stats);
            // Auto-scroll to show the stats line
            chatMessages.scrollTop = chatMessages.scrollHeight;
        }
    }
    // Update context overlay
    if (ctxWindow && ctxWindow > 0) updateContextOverlay(ctxTokens || 0, ctxWindow);
};

globalThis.updateContextOverlay = function updateContextOverlay(ctxTokens, ctxWindow) {
    const overlay = document.getElementById('context-overlay');
    const pctEl   = document.getElementById('context-pct');
    const cntEl   = document.getElementById('context-counts');
    if (!overlay || !pctEl || !cntEl) return;
    lastContextTokens = ctxTokens;
    const pct = Math.round((ctxTokens / ctxWindow) * 100);
    pctEl.textContent = pct + '% context used';
    const fmt = n => n >= 1000 ? (n / 1000).toFixed(1) + 'k' : String(n);
    cntEl.textContent = fmt(ctxTokens) + ' / ' + fmt(ctxWindow);
    overlay.classList.remove('hidden', 'warn', 'danger');
    if (pct >= 80) overlay.classList.add('danger');
    else if (pct >= 60) overlay.classList.add('warn');
};

globalThis.showApprovalRequest = function showApprovalRequest(id, detail) {
    if (!isAdmin) return; // only admin sees approval requests
    const div = document.createElement('div');
    div.className = 'chat-msg assistant approval';
    const sender = document.createElement('div');
    sender.className = 'sender';
    sender.append(runeIcon('lock'), document.createTextNode('Approval Required'));
    const body = document.createElement('div');
    body.className = 'body';
    const code = document.createElement('code');
    code.textContent = detail;
    body.appendChild(code);
    const buttons = document.createElement('div');
    buttons.id = 'approval-btns-' + id;
    buttons.style.cssText = 'margin-top:8px;display:flex;gap:8px';
    [['Allow', 'check', true, 'btn-approve'], ['Deny', 'x', false, 'btn-deny']].forEach(([label, iconName, approved, className]) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.append(runeIcon(iconName), document.createTextNode(label));
        button.className = className;
        button.dataset.action = 'respond-approval';
        button.dataset.id = id;
        button.dataset.approved = String(approved);
        buttons.appendChild(button);
    });
    div.append(sender, body, buttons);
    chatMessages.appendChild(div);
    chatMessages.scrollTop = chatMessages.scrollHeight;
};

globalThis.removeApprovalButtons = function removeApprovalButtons(id) {
    const btns = document.getElementById('approval-btns-' + id);
    if (btns) btns.remove();
};

globalThis.removeAllApprovalButtons = function removeAllApprovalButtons() {
    document.querySelectorAll('[id^="approval-btns-"]').forEach(el => el.remove());
};

globalThis.respondApproval = function respondApproval(id, approved) {
    api('approval', { id, approved });
    addSystemMessage(approved ? `Approved: ${id}` : `Denied: ${id}`);
    removeApprovalButtons(id);
}
