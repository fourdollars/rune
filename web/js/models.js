import './state.js';
// --- Archive ---
// --- Model switcher ---
globalThis.updateModelIndicator = function updateModelIndicator() {
    const indicator = document.getElementById('model-indicator');
    const nameEl = document.getElementById('model-name');
    if (!indicator || !nameEl) return;
    if (!activeModel) { indicator.style.display = 'none'; return; }
    nameEl.textContent = activeModel;
    // Sync mobile model name
    const mobileModelEl = document.getElementById("mobile-model-name");
    if (mobileModelEl) mobileModelEl.textContent = activeModel;
    indicator.style.display = 'flex';
    // Admin can click the name to switch; show pointer cursor
    nameEl.style.cursor = (isAdmin && availableModels.length > 1) ? 'pointer' : 'default';
};

globalThis.updateThinkingSelect = function updateThinkingSelect() {
    const selects = [
        document.getElementById('thinking-select'),
        document.getElementById('mobile-thinking-select')
    ].filter(Boolean);
    if (selects.length === 0) return;

    // Find current model's reasoning_efforts
    const currentModelObj = availableModels.find(m => (m.id || m) === activeModel);
    const efforts = (currentModelObj && currentModelObj.reasoning_efforts) || [];

    if (!isAdmin || efforts.length === 0) {
        selects.forEach(s => s.style.display = 'none');
        return;
    }

    const isGemini3 = activeModel && activeModel.startsWith('gemini-3.');

    // Build options: prepend "off" only when "none" is not already in the list, and not Gemini 3.x
    selects.forEach(select => {
        select.innerHTML = '';
        if (!efforts.includes('none') && !isGemini3) {
            const offOpt = document.createElement('option');
            offOpt.value = 'off';
            offOpt.textContent = 'off';
            select.appendChild(offOpt);
        }

        efforts.forEach(level => {
            const opt = document.createElement('option');
            opt.value = level;
            opt.textContent = level;
            select.appendChild(opt);
        });

        let val = currentThinking || 'off';
        if (isGemini3 && (val === 'off' || val === 'none')) {
            val = efforts[0] || 'medium';
        }
        select.value = val;
        select.style.display = '';
    });
};

globalThis.switchThinking = function switchThinking(level) {
    if (isConnected) {
        api('notes/' + encodeURIComponent(currentNoteId), { thinking: level }, 'PATCH');
    }
};

globalThis.showModelDialog = function showModelDialog() {
    if (!isAdmin || availableModels.length <= 1) return;

    // Set provider in title
    const titleEl = document.getElementById('model-modal-title');
    if (titleEl) {
        const firstModel = availableModels.find(m => m.provider);
        const providerName = firstModel ? firstModel.provider : '';
        if (providerName) {
            let friendlyProvider = providerName;
            const lower = providerName.toLowerCase();
            if (lower === 'gemini') {
                friendlyProvider = 'Google Gemini';
            } else if (lower === 'github-copilot') {
                friendlyProvider = 'GitHub Copilot';
            } else if (lower === 'openrouter') {
                friendlyProvider = 'OpenRouter';
            } else if (lower === 'openrouter-zdr') {
                friendlyProvider = 'OpenRouter w/ ZDR';
            } else if (lower === 'openai') {
                friendlyProvider = 'OpenAI';
            } else if (lower === 'openai-compatible') {
                friendlyProvider = 'OpenAI compatible';
            } else {
                friendlyProvider = providerName.charAt(0).toUpperCase() + providerName.slice(1);
            }
            titleEl.textContent = `Switch Model (${friendlyProvider})`;
        } else {
            titleEl.textContent = 'Switch Model';
        }
    }

    const listEl = document.getElementById('model-list');
    if (!listEl) return;
    listEl.innerHTML = '';
    availableModels.forEach(m => {
        const btn = document.createElement('button');
        const modelId = m.id || m;
        btn.className = 'model-option' + (modelId === activeModel ? ' active' : '');
        
        // Model name
        const nameSpan = document.createElement('span');
        nameSpan.className = 'model-option-name';
        nameSpan.textContent = modelId;
        btn.appendChild(nameSpan);
        
        // Metadata badges
        const badgeContainer = document.createElement('span');
        badgeContainer.className = 'model-badges';
        
        if (m.reasoning_efforts && m.reasoning_efforts.length > 0) {
            const reasonBadge = document.createElement('span');
            reasonBadge.className = 'model-reasoning-badge';
            reasonBadge.textContent = m.reasoning_efforts.join(' | ');
            badgeContainer.appendChild(reasonBadge);
        }
        
        if (m.context_window) {
            const ctxBadge = document.createElement('span');
            ctxBadge.className = 'model-ctx-badge';
            ctxBadge.textContent = formatContextWindow(m.context_window);
            badgeContainer.appendChild(ctxBadge);
        }
        
        btn.appendChild(badgeContainer);
        btn.dataset.action = 'switch-model';
        btn.dataset.model = modelId;
        listEl.appendChild(btn);
    });

    const searchInput = document.getElementById('model-search-input');
    if (searchInput) searchInput.value = '';

    document.getElementById('model-modal').classList.remove('hidden');
    if (searchInput) {
        setTimeout(() => searchInput.focus(), 50);
    }
};

globalThis.formatContextWindow = function formatContextWindow(tokens) {
    if (tokens >= 1000000) return (tokens / 1000000).toFixed(0) + 'M';
    if (tokens >= 1000) return (tokens / 1000).toFixed(0) + 'K';
    return tokens.toString();
}

globalThis.filterModels = function filterModels(value) {
    const query = value.toLowerCase().trim();
    document.querySelectorAll('#model-list .model-option').forEach(button => {
        const name = button.querySelector('.model-option-name')?.textContent.toLowerCase() || '';
        button.style.display = name.includes(query) ? 'flex' : 'none';
    });
};


globalThis.hideModelDialog = function hideModelDialog() {
    document.getElementById('model-modal').classList.add('hidden');
};

globalThis.switchModel = function switchModel(model) {
    if (isConnected) {
        api('notes/' + encodeURIComponent(currentNoteId), { model }, 'PATCH');
    }
}
