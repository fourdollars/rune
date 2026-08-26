import './state.js';
globalThis.replayHistory = function replayHistory(messages) {
    if (!messages || messages.length === 0) return;
    addSystemMessage('── archived ──');
    for (const m of messages) {
        if (m.role === 'user') {
            // Render history user message with original timestamp
            const isMe = m.nickname === myNickname;
            const div = document.createElement('div');
            div.className = `chat-msg ${isMe ? 'user' : 'other'}`;
            const sender = document.createElement('div');
            sender.className = 'sender';
            const nameSpan = document.createElement('span');
            nameSpan.className = 'sender-name';
            nameSpan.append(runeIcon('user'), document.createTextNode(isMe ? `${m.nickname} (you)` : m.nickname));
            const timeSpan = document.createElement('span');
            timeSpan.className = 'msg-time';
            timeSpan.textContent = fmtTime(m.created_at || null);
            sender.appendChild(nameSpan);
            sender.appendChild(timeSpan);
            const body = document.createElement('div');
            body.className = 'body';
            body.textContent = m.content;
            div.appendChild(sender);
            div.appendChild(body);
            chatMessages.appendChild(div);
        } else if (m.role === 'assistant') {
            // Render as completed assistant message
            const div = document.createElement('div');
            div.className = 'chat-msg assistant';
            const sender = document.createElement('div');
            sender.className = 'sender';
            const nameSpan = document.createElement('span');
            nameSpan.textContent = 'ᚱ';
            sender.appendChild(nameSpan);
            // Model in header
            if (m.model) {
                const meta = document.createElement('span');
                meta.className = 'msg-meta';
                meta.textContent = (m.thinking && m.thinking !== 'off') ? `${m.model} ${m.thinking}` : m.model;
                sender.appendChild(meta);
            }
            const timeSpan = document.createElement('span');
            timeSpan.className = 'msg-time';
            timeSpan.textContent = fmtTime(m.created_at || null);
            sender.appendChild(timeSpan);
            const body = document.createElement('div');
            body.className = 'body';
            if (typeof marked !== 'undefined') {
                body.replaceChildren(markdownFragment(m.content));
                renderChatMath(body);
                if (typeof renderMermaidBlocks === 'function') renderMermaidBlocks(body);
            } else {
                body.textContent = m.content;
            }
            // Run stats at message tail
            const totalTok = (m.tokens_in||0) + (m.tokens_out||0);
            if (m.steps || totalTok || m.tool_calls) {
                const stats = document.createElement('div');
                stats.className = 'run-stats';
                stats.textContent = `${m.steps||0} steps · ${totalTok} tokens · ${m.tool_calls||0} tool calls`;
                body.appendChild(stats);
            }
            div.appendChild(sender);
            div.appendChild(body);
            chatMessages.appendChild(div);
        } else if (m.role === 'system') {
            addSystemMessage(m.content);
        }
    }
    addSystemMessage('── current ──');
    chatMessages.scrollTop = chatMessages.scrollHeight;

    // Restore context overlay from the last assistant message that has context_tokens
    const lastWithCtx = [...messages].reverse().find(
        m => m.role === 'assistant' && m.context_tokens != null && m.model
    );
    if (lastWithCtx) {
        const modelEntry = availableModels.find(m => m.id === lastWithCtx.model);
        if (modelEntry && modelEntry.context_window) {
            updateContextOverlay(lastWithCtx.context_tokens, modelEntry.context_window);
        }
    }
};

globalThis.renderChatMath = function renderChatMath(el) {
    if (typeof renderMathInElement !== 'undefined') {
        renderMathInElement(el, {
            delimiters: [
                {left: '$$', right: '$$', display: true},
                {left: '$', right: '$', display: false},
                {left: '\\(', right: '\\)', display: false},
                {left: '\\[', right: '\\]', display: true}
            ],
            throwOnError: false
        });
    }
};

globalThis.updateOnlineCount = function updateOnlineCount(count) {
    const el = document.getElementById('online-count');
    if (el) el.textContent = count;
}
