import './state.js';

let activeAdminTab = 'provider';
let adminCopilotPollTimer = null;
let adminCopilotActive = false;
let adminDetectedModels = [];

function splitCsv(val) {
    return String(val || '').split(',').map(s => s.trim()).filter(Boolean);
}

function joinCsv(arr) {
    return Array.isArray(arr) ? arr.join(', ') : '';
}

function setAdminStatus(message, kind = '') {
    const el = document.getElementById('admin-settings-status');
    if (!el) return;
    el.textContent = message || '';
    el.className = 'admin-settings-status' + (kind ? ` ${kind}` : '');
    el.classList.toggle('hidden', !message);
}

globalThis.updateAdminSettingsButton = function updateAdminSettingsButton() {
    const btn = document.getElementById('btn-admin-settings');
    if (btn) btn.classList.toggle('hidden', !isAdmin);
};

globalThis.switchAdminSettingsTab = function switchAdminSettingsTab(tab) {
    activeAdminTab = tab || 'provider';
    document.querySelectorAll('#admin-settings-modal [data-admin-tab]').forEach(btn => {
        const active = btn.dataset.adminTab === activeAdminTab;
        btn.classList.toggle('active', active);
        btn.setAttribute('aria-selected', String(active));
    });
    document.querySelectorAll('#admin-settings-modal [data-admin-panel]').forEach(panel => {
        panel.classList.toggle('hidden', panel.dataset.adminPanel !== activeAdminTab);
    });
};

globalThis.toggleAdminAuthSections = function toggleAdminAuthSections() {
    const localOn = !!document.getElementById('admin-cfg-local-enabled')?.checked;
    const ghOn = !!document.getElementById('admin-cfg-github-enabled')?.checked;
    document.getElementById('admin-cfg-local-fields')?.classList.toggle('hidden', !localOn);
    document.getElementById('admin-cfg-github-fields')?.classList.toggle('hidden', !ghOn);
};

function populateAdminForm(data) {
    const cfg = data.config || {};

    const setVal = (id, v) => {
        const el = document.getElementById(id);
        if (el) el.value = v == null ? '' : String(v);
    };
    const setChk = (id, v) => {
        const el = document.getElementById(id);
        if (el) el.checked = !!v;
    };

    setVal('admin-cfg-provider', cfg.provider || '');
    const apiKeyInput = document.getElementById('admin-cfg-api-key');
    if (apiKeyInput) {
        apiKeyInput.value = '';
        apiKeyInput.placeholder = data.has_api_key
            ? '•••••••• (leave blank to keep existing)'
            : 'sk-or-... / AIza... / ghu_... / sk-...';
    }
    setVal('admin-cfg-base-url', cfg.base_url || '');
    setVal('admin-cfg-model', cfg.model || '');
    setVal('admin-cfg-notes-model', cfg.notes_model || '');
    setVal('admin-cfg-thinking', cfg.thinking || '');
    setVal('admin-cfg-skills-dir', cfg.skills_dir || '');
    setChk('admin-cfg-openrouter-zdr', cfg.openrouter_zdr);
    setChk('admin-cfg-embedding-enabled', cfg.embedding_enabled);
    setVal('admin-cfg-embedding-model', cfg.embedding_model || '');

    setVal('admin-cfg-notes-title', cfg.notes_title || '');
    setVal('admin-cfg-notes-desc', cfg.notes_desc || '');
    setVal('admin-cfg-notes-bind', cfg.notes_bind || '');
    setVal('admin-cfg-notes-port', cfg.notes_port || '');
    setChk('admin-cfg-agent-skills', cfg.agent_skills);
    setChk('admin-cfg-persona-files', cfg.persona_files);
    setChk('admin-cfg-cron-jobs', cfg.cron_jobs);

    setChk('admin-cfg-local-enabled', cfg.local_enabled);
    setVal('admin-cfg-local-admins', joinCsv(cfg.local_admins));
    setVal('admin-cfg-local-users', joinCsv(cfg.local_users));
    setVal('admin-cfg-local-guests', joinCsv(cfg.local_guests));

    setChk('admin-cfg-github-enabled', cfg.github_enabled);
    setVal('admin-cfg-github-client-id', cfg.github_client_id || '');
    const ghSecretInput = document.getElementById('admin-cfg-github-client-secret');
    if (ghSecretInput) {
        ghSecretInput.value = '';
        ghSecretInput.placeholder = data.has_github_client_secret
            ? '•••••••• (leave blank to keep existing)'
            : 'GitHub OAuth Client Secret';
    }
    setVal('admin-cfg-github-admins', joinCsv(cfg.github_admins));
    setVal('admin-cfg-github-users', joinCsv(cfg.github_users));
    setVal('admin-cfg-github-guests', joinCsv(cfg.github_guests));

    toggleAdminAuthSections();
    updateAdminCopilotVisibility();
}

globalThis.cancelAdminCopilotFlow = function cancelAdminCopilotFlow() {
    adminCopilotActive = false;
    if (adminCopilotPollTimer) {
        clearTimeout(adminCopilotPollTimer);
        adminCopilotPollTimer = null;
    }
    const startBtn = document.getElementById('admin-copilot-start-btn');
    if (startBtn) startBtn.disabled = false;
    document.getElementById('admin-copilot-cancel-btn')?.classList.add('hidden');
    document.getElementById('admin-copilot-code-row')?.classList.add('hidden');
};

globalThis.updateAdminCopilotVisibility = function updateAdminCopilotVisibility() {
    const provider = document.getElementById('admin-cfg-provider')?.value || '';
    const isCopilot = provider === 'github-copilot';
    const isOpenRouter = provider === 'openrouter';
    const showBaseUrl = !['openrouter', 'github-copilot', 'gemini'].includes(provider);

    document.getElementById('admin-copilot-box')?.classList.toggle('hidden', !isCopilot);
    if (!isCopilot) cancelAdminCopilotFlow();

    document.getElementById('admin-cfg-zdr-field')?.classList.toggle('hidden', !isOpenRouter);
    if (!isOpenRouter) {
        const zdrChk = document.getElementById('admin-cfg-openrouter-zdr');
        if (zdrChk) zdrChk.checked = false;
    }

    document.getElementById('admin-cfg-base-url-field')?.classList.toggle('hidden', !showBaseUrl);
};

function formatAdminCtx(tokens) {
    if (!tokens) return '';
    if (tokens >= 1000000) return `${Math.round(tokens / 100000) / 10}M`;
    return `${Math.round(tokens / 1000)}k`;
}

globalThis.toggleAdminDetectedModel = function toggleAdminDetectedModel(modelId) {
    const notesModelInput = document.getElementById('admin-cfg-notes-model');
    const defaultModelInput = document.getElementById('admin-cfg-model');
    const selected = splitCsv(notesModelInput?.value || '');
    const idx = selected.indexOf(modelId);
    if (idx >= 0) selected.splice(idx, 1);
    else selected.push(modelId);
    if (notesModelInput) notesModelInput.value = selected.join(', ');
    if (defaultModelInput && selected.length > 0 && !defaultModelInput.value.trim()) {
        defaultModelInput.value = selected[0];
    }
    renderAdminModelList();
};

globalThis.renderAdminModelList = function renderAdminModelList() {
    const host = document.getElementById('admin-model-list');
    if (!host) return;
    if (adminDetectedModels.length === 0) {
        host.classList.add('hidden');
        return;
    }
    host.classList.remove('hidden');
    const q = (document.getElementById('admin-model-search')?.value || '').trim().toLowerCase();
    const selectedSet = new Set(splitCsv(document.getElementById('admin-cfg-notes-model')?.value || ''));
    const filtered = adminDetectedModels.filter(m => !q || m.id.toLowerCase().includes(q));

    if (typeof host.replaceChildren === 'function') host.replaceChildren();
    else host.children = [];

    for (const m of filtered.slice(0, 150)) {
        const row = document.createElement('label');
        row.className = 'admin-model-item';
        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.checked = selectedSet.has(m.id);
        cb.onchange = () => toggleAdminDetectedModel(m.id);
        row.appendChild(cb);
        const span = document.createElement('span');
        span.className = 'admin-model-id';
        span.textContent = m.id;
        row.appendChild(span);
        const ctx = formatAdminCtx(m.context_window);
        if (ctx) {
            const badge = document.createElement('span');
            badge.className = 'admin-model-badge';
            badge.textContent = ctx;
            row.appendChild(badge);
        }
        host.appendChild(row);
    }
};

globalThis.detectAdminModels = async function detectAdminModels() {
    if (!isAdmin) return;
    const status = document.getElementById('admin-model-detect-status');
    const btn = document.getElementById('admin-detect-models-btn');
    const search = document.getElementById('admin-model-search');
    if (btn) btn.disabled = true;
    if (status) status.textContent = 'Detecting models…';

    const provider = document.getElementById('admin-cfg-provider')?.value || null;
    const apiKey = document.getElementById('admin-cfg-api-key')?.value.trim() || null;
    const baseUrl = document.getElementById('admin-cfg-base-url')?.value.trim() || null;
    const zdr = provider === 'openrouter' && !!document.getElementById('admin-cfg-openrouter-zdr')?.checked;

    const data = await api('admin/models', {
        provider,
        api_key: apiKey,
        base_url: baseUrl,
        openrouter_zdr: zdr,
    }, 'POST');

    if (btn) btn.disabled = false;
    if (!data || !data.ok) {
        adminDetectedModels = [];
        if (status) status.textContent = (data && data.error) || 'Failed to detect models';
        renderAdminModelList();
        return;
    }

    adminDetectedModels = Array.isArray(data.models) ? data.models : [];
    if (search) search.classList.toggle('hidden', adminDetectedModels.length === 0);
    if (status) {
        status.textContent = adminDetectedModels.length > 0
            ? `${adminDetectedModels.length} model(s) detected`
            : 'No models returned';
    }
    renderAdminModelList();
};

globalThis.copyAdminCopilotCode = async function copyAdminCopilotCode() {
    const code = document.getElementById('admin-copilot-user-code')?.textContent || '';
    if (!code) return;
    try { await navigator.clipboard.writeText(code); } catch {}
    const status = document.getElementById('admin-copilot-status');
    if (status) status.textContent = 'Code copied! Paste it on GitHub to authorize.';
};

globalThis.startAdminCopilotFlow = async function startAdminCopilotFlow() {
    if (!isAdmin) return;
    cancelAdminCopilotFlow();
    const status = document.getElementById('admin-copilot-status');
    const startBtn = document.getElementById('admin-copilot-start-btn');
    const setSt = (msg, cls = '') => {
        if (!status) return;
        status.textContent = msg;
        status.className = 'admin-copilot-status' + (cls ? ` ${cls}` : '');
    };
    setSt('Requesting device code from GitHub…');
    if (startBtn) startBtn.disabled = true;

    const data = await api('admin/copilot/start', {}, 'POST');
    if (!data || !data.ok) {
        setSt((data && data.error) || 'Failed to start GitHub device flow', 'error');
        if (startBtn) startBtn.disabled = false;
        return;
    }

    const codeEl = document.getElementById('admin-copilot-user-code');
    if (codeEl) codeEl.textContent = data.user_code || '';
    const linkEl = document.getElementById('admin-copilot-verify-link');
    if (linkEl && data.verification_uri) linkEl.href = data.verification_uri;
    document.getElementById('admin-copilot-code-row')?.classList.remove('hidden');
    document.getElementById('admin-copilot-cancel-btn')?.classList.remove('hidden');
    setSt('Open GitHub, enter the code above, and authorize. Waiting…');

    adminCopilotActive = true;
    let intervalMs = Math.max(5, data.interval || 5) * 1000;
    const deviceCode = data.device_code;

    const poll = async () => {
        if (!adminCopilotActive) return;
        const pData = await api('admin/copilot/poll', { device_code: deviceCode }, 'POST');
        if (!adminCopilotActive) return;
        if (!pData || !pData.ok) {
            cancelAdminCopilotFlow();
            setSt((pData && pData.error) || 'Authorization failed', 'error');
            return;
        }
        if (pData.status === 'authorized' && pData.access_token) {
            const apiKeyInput = document.getElementById('admin-cfg-api-key');
            if (apiKeyInput) apiKeyInput.value = pData.access_token;
            cancelAdminCopilotFlow();
            setSt('✓ Authorized! Detecting Copilot models…', 'success');
            await detectAdminModels();
            return;
        }
        if (pData.status === 'expired') {
            cancelAdminCopilotFlow();
            setSt('Device code expired. Please try again.', 'error');
            return;
        }
        if (pData.status === 'denied') {
            cancelAdminCopilotFlow();
            setSt('Authorization denied on GitHub.', 'error');
            return;
        }
        if (pData.status === 'slow_down') {
            intervalMs = Math.max(intervalMs + 5000, (pData.interval || 10) * 1000);
        }
        if (adminCopilotActive) {
            adminCopilotPollTimer = setTimeout(poll, intervalMs);
        }
    };
    adminCopilotPollTimer = setTimeout(poll, intervalMs);
};

function collectStructuredConfig() {
    const val = id => (document.getElementById(id)?.value || '').trim();
    const chk = id => !!document.getElementById(id)?.checked;
    const portStr = val('admin-cfg-notes-port');
    const portNum = portStr ? parseInt(portStr, 10) : null;

    return {
        provider: val('admin-cfg-provider') || null,
        api_key: val('admin-cfg-api-key') || null,
        clear_api_key: false,
        base_url: val('admin-cfg-base-url') || null,
        model: val('admin-cfg-model') || null,
        thinking: val('admin-cfg-thinking') || null,
        openrouter_zdr: chk('admin-cfg-openrouter-zdr'),
        skills_dir: val('admin-cfg-skills-dir') || null,
        embedding_enabled: chk('admin-cfg-embedding-enabled'),
        embedding_model: val('admin-cfg-embedding-model') || null,
        notes_port: Number.isFinite(portNum) ? portNum : null,
        notes_bind: val('admin-cfg-notes-bind') || null,
        notes_model: val('admin-cfg-notes-model') || null,
        notes_title: val('admin-cfg-notes-title') || null,
        notes_desc: val('admin-cfg-notes-desc') || null,
        agent_skills: chk('admin-cfg-agent-skills'),
        persona_files: chk('admin-cfg-persona-files'),
        cron_jobs: chk('admin-cfg-cron-jobs'),
        local_enabled: chk('admin-cfg-local-enabled'),
        local_admins: splitCsv(val('admin-cfg-local-admins')),
        local_users: splitCsv(val('admin-cfg-local-users')),
        local_guests: splitCsv(val('admin-cfg-local-guests')),
        github_enabled: chk('admin-cfg-github-enabled'),
        github_client_id: val('admin-cfg-github-client-id') || null,
        github_client_secret: val('admin-cfg-github-client-secret') || null,
        github_admins: splitCsv(val('admin-cfg-github-admins')),
        github_users: splitCsv(val('admin-cfg-github-users')),
        github_guests: splitCsv(val('admin-cfg-github-guests')),
    };
}

globalThis.showAdminSettings = async function showAdminSettings() {
    if (!isAdmin) return;
    const modal = document.getElementById('admin-settings-modal');
    if (!modal) return;
    setAdminStatus('Loading configuration…');
    switchAdminSettingsTab('provider');
    modal.classList.remove('hidden');

    const data = await api('admin/config', undefined, 'GET');
    if (!data || !data.ok) {
        setAdminStatus((data && data.error) || 'Failed to load configuration', 'error');
        return;
    }
    populateAdminForm(data);
    setAdminStatus('');
};

globalThis.hideAdminSettings = function hideAdminSettings() {
    cancelAdminCopilotFlow();
    document.getElementById('admin-settings-modal')?.classList.add('hidden');
};

globalThis.saveAdminSettings = async function saveAdminSettings() {
    if (!isAdmin) return;
    setAdminStatus('Saving and reloading…');
    const body = { config: collectStructuredConfig() };

    const data = await api('admin/config', body, 'PUT');
    if (!data || !data.ok) {
        setAdminStatus((data && data.error) || 'Failed to save configuration', 'error');
        return;
    }

    // Refresh form from saved config
    const refreshed = await api('admin/config', undefined, 'GET');
    if (refreshed && refreshed.ok) {
        populateAdminForm(refreshed);
        personaFilesEnabled = !!refreshed.config?.persona_files;
        cronJobsEnabled = !!refreshed.config?.cron_jobs;
    }

    const msg = data.restart_required
        ? `Saved & reloaded (${data.models_count} models). Note: bind/port change requires a server restart.`
        : `Saved & reloaded (${data.models_count} models).`;
    setAdminStatus(msg, data.restart_required ? 'warning' : 'success');
};
