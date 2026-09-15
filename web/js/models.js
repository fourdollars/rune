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
    let efforts = (currentModelObj && currentModelObj.reasoning_efforts) || [];

    const isOpenRouterAuto = activeModel && activeModel.startsWith('openrouter/auto');

    if (isOpenRouterAuto && efforts.length === 0) {
        efforts = ['low', 'medium', 'high', 'xhigh', 'max'];
    }

    if (!isAdmin || efforts.length === 0) {
        selects.forEach(s => s.style.display = 'none');
        return;
    }

    const isGemini3 = activeModel && activeModel.startsWith('gemini-3.');
    const label = isOpenRouterAuto ? 'Cost tier' : 'Thinking level';

    // Build options: prepend "off" only when "none" is not already in the list, and not Gemini 3.x
    selects.forEach(select => {
        select.title = label;
        select.setAttribute('aria-label', label);
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

        let val = currentThinking || (isOpenRouterAuto ? 'low' : 'off');
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

globalThis.formatCredits = function formatCredits(num) {
    if (typeof num !== 'number') return '';
    if (num >= 1000000) return (num / 1000000).toFixed(1).replace(/\.0$/, '') + 'M';
    if (num >= 1000) return (num / 1000).toFixed(1).replace(/\.0$/, '') + 'k';
    return num.toLocaleString();
};

globalThis.updateUsageIndicator = function updateUsageIndicator() {
    const indicator = document.getElementById('quota-indicator');
    const textEl = document.getElementById('quota-text');
    if (!indicator || !textEl) return;

    if (!providerUsage) {
        indicator.classList.add('hidden');
        return;
    }

    const hasRemaining = typeof providerUsage.quota_remaining === 'number';
    const hasPercent = typeof providerUsage.quota_percent_remaining === 'number';

    let percent = hasPercent ? providerUsage.quota_percent_remaining : null;
    if (percent === null && hasRemaining && providerUsage.quota_entitlement) {
        percent = (providerUsage.quota_remaining / providerUsage.quota_entitlement) * 100;
    }

    let label = '';
    if (hasRemaining) {
        label = formatCredits(providerUsage.quota_remaining);
        if (percent !== null) {
            label += ` (${Math.round(percent)}%)`;
        }
    } else if (percent !== null) {
        label = `${Math.round(percent)}%`;
    } else if (providerUsage.plan_name) {
        label = providerUsage.plan_name.replace(/^GitHub /, '');
    } else if (providerUsage.provider === 'github-copilot') {
        label = 'Copilot';
    } else {
        indicator.classList.add('hidden');
        return;
    }

    textEl.textContent = label;
    indicator.classList.remove('hidden');

    // Warning styling based on remaining percentage
    indicator.classList.remove('warn', 'danger');
    if (percent !== null) {
        if (percent <= 20) {
            indicator.classList.add('danger');
        } else if (percent <= 50) {
            indicator.classList.add('warn');
        }
    }

    // Update Popover content
    const popoverRemaining = document.getElementById('quota-popover-remaining');
    const popoverPlan = document.getElementById('quota-popover-plan');
    const popoverTokens = document.getElementById('quota-popover-tokens');
    const popoverPercent = document.getElementById('quota-popover-percent');
    const progressFill = document.getElementById('quota-progress-fill');

    if (popoverRemaining) {
        if (hasRemaining && providerUsage.quota_entitlement) {
            popoverRemaining.textContent = `${providerUsage.quota_remaining.toLocaleString()} / ${providerUsage.quota_entitlement.toLocaleString()}`;
        } else if (hasRemaining) {
            popoverRemaining.textContent = providerUsage.quota_remaining.toLocaleString();
        } else {
            popoverRemaining.textContent = 'Active';
        }
    }

    if (popoverPercent) {
        popoverPercent.textContent = percent !== null ? `${Math.round(percent)}%` : '';
    }

    if (popoverPlan) {
        popoverPlan.textContent = providerUsage.plan_name || providerUsage.provider || '—';
    }

    if (popoverTokens) {
        popoverTokens.textContent = (providerUsage.session_tokens || 0).toLocaleString();
    }

    if (progressFill) {
        if (percent !== null) {
            progressFill.style.width = `${Math.min(Math.max(percent, 0), 100)}%`;
            progressFill.className = 'quota-progress-fill' + (percent <= 20 ? ' danger' : (percent <= 50 ? ' warn' : ''));
            progressFill.parentElement.style.display = 'block';
        } else {
            progressFill.parentElement.style.display = 'none';
        }
    }
};

globalThis.toggleQuotaPopover = function toggleQuotaPopover() {
    const popover = document.getElementById('quota-popover');
    const indicator = document.getElementById('quota-indicator');
    if (!popover || !indicator) return;
    const isHidden = popover.classList.contains('hidden');
    if (isHidden) {
        popover.classList.remove('hidden');
        indicator.setAttribute('aria-expanded', 'true');
    } else {
        popover.classList.add('hidden');
        indicator.setAttribute('aria-expanded', 'false');
    }
};

globalThis.closeQuotaPopover = function closeQuotaPopover() {
    const popover = document.getElementById('quota-popover');
    const indicator = document.getElementById('quota-indicator');
    if (popover) popover.classList.add('hidden');
    if (indicator) indicator.setAttribute('aria-expanded', 'false');
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

    // Update Quota banner in modal
    const modalQuota = document.getElementById('model-modal-quota');
    const modalQuotaText = document.getElementById('model-modal-quota-text');
    const modalQuotaFill = document.getElementById('model-modal-quota-fill');
    if (modalQuota && modalQuotaText && modalQuotaFill) {
        if (providerUsage && (typeof providerUsage.quota_remaining === 'number' || typeof providerUsage.quota_percent_remaining === 'number' || providerUsage.plan_name || providerUsage.provider === 'github-copilot')) {
            const hasRemaining = typeof providerUsage.quota_remaining === 'number';
            const hasPercent = typeof providerUsage.quota_percent_remaining === 'number';
            let percent = hasPercent ? providerUsage.quota_percent_remaining : null;
            if (percent === null && hasRemaining && providerUsage.quota_entitlement) {
                percent = (providerUsage.quota_remaining / providerUsage.quota_entitlement) * 100;
            }

            if (hasRemaining && providerUsage.quota_entitlement) {
                modalQuotaText.textContent = `${providerUsage.quota_remaining.toLocaleString()} / ${providerUsage.quota_entitlement.toLocaleString()}${percent !== null ? ` (${Math.round(percent)}%)` : ''}`;
            } else if (hasRemaining) {
                modalQuotaText.textContent = `${providerUsage.quota_remaining.toLocaleString()}${percent !== null ? ` (${Math.round(percent)}%)` : ''}`;
            } else if (percent !== null) {
                modalQuotaText.textContent = `${Math.round(percent)}% remaining`;
            } else {
                modalQuotaText.textContent = providerUsage.plan_name || 'Active';
            }

            if (percent !== null) {
                modalQuotaFill.style.width = `${Math.min(Math.max(percent, 0), 100)}%`;
                modalQuotaFill.className = 'quota-progress-fill' + (percent <= 20 ? ' danger' : (percent <= 50 ? ' warn' : ''));
                modalQuotaFill.parentElement.style.display = 'block';
            } else {
                modalQuotaFill.parentElement.style.display = 'none';
            }
            modalQuota.classList.remove('hidden');
        } else {
            modalQuota.classList.add('hidden');
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
