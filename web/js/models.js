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

globalThis.updateThinkingButton = function updateThinkingButton() {
    const btn = document.getElementById('thinking-btn');
    const label = document.getElementById('thinking-btn-label');
    if (!btn) return;

    // Find current model's reasoning_efforts
    const currentModelObj = availableModels.find(m => (m.id || m) === activeModel);
    let efforts = (currentModelObj && currentModelObj.reasoning_efforts) || [];

    const isOpenRouterAuto = activeModel && activeModel.startsWith('openrouter/auto');

    if (isOpenRouterAuto && efforts.length === 0) {
        efforts = ['low', 'medium', 'high', 'xhigh', 'max'];
    }

    if (!isAdmin || efforts.length === 0) {
        btn.style.display = 'none';
        return;
    }

    const isGemini3 = activeModel && activeModel.startsWith('gemini-3.');
    let val = currentThinking || (isOpenRouterAuto ? 'low' : 'off');
    if (isGemini3 && (val === 'off' || val === 'none')) {
        val = efforts[0] || 'medium';
    }

    if (label) {
        label.textContent = val;
    }
    const titlePrefix = isOpenRouterAuto ? 'Cost tier' : 'Thinking level';
    btn.title = `${titlePrefix}: ${val}`;
    btn.setAttribute('aria-label', `${titlePrefix}: ${val}`);
    btn.style.display = 'inline-flex';
};

globalThis.updateThinkingSelect = globalThis.updateThinkingButton;

globalThis.showThinkingDialog = function showThinkingDialog() {
    if (!isAdmin) return;

    const currentModelObj = availableModels.find(m => (m.id || m) === activeModel);
    let efforts = (currentModelObj && currentModelObj.reasoning_efforts) || [];
    const isOpenRouterAuto = activeModel && activeModel.startsWith('openrouter/auto');
    if (isOpenRouterAuto && efforts.length === 0) {
        efforts = ['low', 'medium', 'high', 'xhigh', 'max'];
    }
    if (efforts.length === 0) return;

    const isGemini3 = activeModel && activeModel.startsWith('gemini-3.');
    const titleEl = document.getElementById('thinking-modal-title');
    const descEl = document.getElementById('thinking-modal-desc');
    if (titleEl) {
        titleEl.textContent = isOpenRouterAuto ? 'Select Cost Tier' : 'Adjust Thinking Level';
    }
    if (descEl) {
        descEl.textContent = isOpenRouterAuto
            ? 'Select reasoning effort and cost tier for OpenRouter Auto.'
            : 'Configure reasoning effort for AI responses.';
    }

    const listEl = document.getElementById('thinking-list');
    if (!listEl) return;
    listEl.innerHTML = '';

    const levels = [];
    if (!efforts.includes('none') && !isGemini3) {
        levels.push('off');
    }
    efforts.forEach(lvl => {
        if (!levels.includes(lvl)) levels.push(lvl);
    });

    let activeVal = currentThinking || (isOpenRouterAuto ? 'low' : 'off');
    if (isGemini3 && (activeVal === 'off' || activeVal === 'none')) {
        activeVal = efforts[0] || 'medium';
    }

    const descriptions = {
        off: 'No reasoning tokens; fastest responses and lowest latency',
        none: 'Reasoning disabled; standard responses',
        low: 'Light reasoning; fast responses with basic chain-of-thought',
        medium: 'Balanced thinking; recommended for general problem solving',
        high: 'Deep reasoning; comprehensive reasoning for complex tasks',
        xhigh: 'Extended reasoning; intensive multi-step problem solving',
        max: 'Maximum thinking; largest reasoning budget available'
    };

    levels.forEach(lvl => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'thinking-option' + (lvl === activeVal ? ' active' : '');
        btn.dataset.action = 'select-thinking-level';
        btn.dataset.level = lvl;

        const leftDiv = document.createElement('div');
        leftDiv.className = 'thinking-option-info';

        const nameSpan = document.createElement('span');
        nameSpan.className = 'thinking-option-name';
        nameSpan.textContent = lvl;
        leftDiv.appendChild(nameSpan);

        if (descriptions[lvl]) {
            const descSpan = document.createElement('span');
            descSpan.className = 'thinking-option-desc';
            descSpan.textContent = descriptions[lvl];
            leftDiv.appendChild(descSpan);
        }

        btn.appendChild(leftDiv);

        if (lvl === activeVal) {
            const checkSpan = document.createElement('span');
            checkSpan.className = 'thinking-option-badge';
            checkSpan.textContent = 'Active';
            btn.appendChild(checkSpan);
        }

        listEl.appendChild(btn);
    });

    document.getElementById('thinking-modal')?.classList.remove('hidden');
};

globalThis.hideThinkingDialog = function hideThinkingDialog() {
    document.getElementById('thinking-modal')?.classList.add('hidden');
};

globalThis.switchThinking = function switchThinking(level) {
    if (isConnected) {
        api('notes/' + encodeURIComponent(currentNoteId), { thinking: level }, 'PATCH');
    }
    if (typeof updateThinkingButton === 'function') {
        currentThinking = level;
        updateThinkingButton();
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
    const batteryIcon = document.getElementById('quota-battery-icon');
    if (!indicator) return;

    if (!providerUsage) {
        indicator.classList.add('hidden');
        return;
    }

    const isOpenRouter = providerUsage.provider === 'openrouter';
    const hasRemaining = typeof providerUsage.quota_remaining === 'number';
    const hasPercent = typeof providerUsage.quota_percent_remaining === 'number';
    const details = providerUsage.details || {};
    const hasUsdBalance = typeof details.balance === 'number' && typeof details.limit === 'number';

    let percent = hasPercent ? providerUsage.quota_percent_remaining : null;
    if (percent === null && hasRemaining && providerUsage.quota_entitlement) {
        percent = (providerUsage.quota_remaining / providerUsage.quota_entitlement) * 100;
    }

    let tooltip = 'AI Credits';
    let iconName = 'battery';

    if (isOpenRouter) {
        if (hasUsdBalance && percent !== null) {
            tooltip = `OpenRouter Budget: $${details.balance.toFixed(2)} / $${details.limit.toFixed(2)} (${Math.round(percent)}%)`;
        } else if (hasRemaining && providerUsage.quota_entitlement && percent !== null) {
            tooltip = `OpenRouter Budget: $${(providerUsage.quota_remaining / 100).toFixed(2)} / $${(providerUsage.quota_entitlement / 100).toFixed(2)} (${Math.round(percent)}%)`;
        } else if (hasRemaining && percent !== null) {
            tooltip = `OpenRouter Budget: $${(providerUsage.quota_remaining / 100).toFixed(2)} (${Math.round(percent)}%)`;
        } else if (typeof details.usage === 'number') {
            if (typeof details.usage_monthly === 'number') {
                tooltip = `OpenRouter Usage: $${details.usage.toFixed(2)} (Month: $${details.usage_monthly.toFixed(2)})`;
            } else {
                tooltip = `OpenRouter Usage: $${details.usage.toFixed(2)}`;
            }
        } else if (providerUsage.plan_name) {
            tooltip = `OpenRouter: ${providerUsage.plan_name}`;
        } else {
            tooltip = 'OpenRouter Budgets';
        }
    } else if (hasRemaining && percent !== null) {
        tooltip = `AI Credits: ${formatCredits(providerUsage.quota_remaining)} (${Math.round(percent)}%)`;
    } else if (hasRemaining) {
        tooltip = `AI Credits: ${formatCredits(providerUsage.quota_remaining)}`;
    } else if (percent !== null) {
        tooltip = `AI Credits: ${Math.round(percent)}% remaining`;
    } else if (providerUsage.plan_name) {
        tooltip = `AI Credits: ${providerUsage.plan_name}`;
    }

    if (percent !== null) {
        if (percent <= 20) {
            iconName = 'battery-low';
        } else if (percent <= 60) {
            iconName = 'battery-medium';
        } else {
            iconName = 'battery-full';
        }
    }

    indicator.title = tooltip;
    indicator.setAttribute('aria-label', tooltip);

    if (batteryIcon && typeof setRuneIcon === 'function') {
        setRuneIcon(batteryIcon, iconName);
    }

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
    const popoverTitle = document.querySelector('.quota-popover-title');
    const popoverRemaining = document.getElementById('quota-popover-remaining');
    const popoverPlan = document.getElementById('quota-popover-plan');
    const popoverTokens = document.getElementById('quota-popover-tokens');
    const popoverPercent = document.getElementById('quota-popover-percent');
    const progressFill = document.getElementById('quota-progress-fill');

    if (popoverTitle) {
        popoverTitle.textContent = isOpenRouter ? 'OpenRouter Budgets' : 'AI Credits';
    }

    if (popoverRemaining) {
        if (isOpenRouter) {
            if (hasUsdBalance) {
                popoverRemaining.textContent = `$${details.balance.toFixed(2)} / $${details.limit.toFixed(2)}`;
            } else if (hasRemaining && providerUsage.quota_entitlement) {
                popoverRemaining.textContent = `$${(providerUsage.quota_remaining / 100).toFixed(2)} / $${(providerUsage.quota_entitlement / 100).toFixed(2)}`;
            } else if (hasRemaining) {
                popoverRemaining.textContent = `$${(providerUsage.quota_remaining / 100).toFixed(2)}`;
            } else if (typeof details.usage === 'number') {
                if (typeof details.usage_monthly === 'number') {
                    popoverRemaining.textContent = `Used $${details.usage.toFixed(2)} (Month: $${details.usage_monthly.toFixed(2)})`;
                } else {
                    popoverRemaining.textContent = `Used $${details.usage.toFixed(2)}`;
                }
            } else {
                popoverRemaining.textContent = 'Active';
            }
        } else if (hasRemaining && providerUsage.quota_entitlement) {
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
