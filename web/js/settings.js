import './state.js';
// --- Note Settings Dialog ---

const PERSONA_DESCRIPTIONS = {
    'AGENTS.md': 'Agent run loop, sandbox boundaries & limits',
    'SOUL.md': 'Core personality, tone & philosophy',
    'IDENTITY.md': 'Name, role, background & expertise',
    'USER.md': 'User profile, preferences & constraints',
    'TOOLS.md': 'Available capabilities & usage rules',
    'BOOTSTRAP.md': 'First-turn initialization instructions',
    'HEARTBEAT.md': 'Autonomous periodic checklist & tasks',
};

let currentCronJobs = [];

globalThis.showNoteSettings = function showNoteSettings(sessionId) {
    const s = notes.find(x => x.id === sessionId);
    if (!s) return;
    settingsNoteId = sessionId;
    document.getElementById('note-settings-title').textContent = 'Note: ' + s.name;
    document.getElementById('note-settings-name').value = s.name;
    
    selectedNoteIcon = s.icon || null;
    renderNoteIconTrigger();
    markSelectedEmoji();
    closeEmojiPicker();

    // Default to General tab (configures delete/save buttons)
    switchSettingsTab('general');

    // Toggle tab buttons visibility based on opt-in configuration flags
    const personaTabBtn = document.querySelector('.settings-tab[data-tab="persona"]');
    if (personaTabBtn) {
        personaTabBtn.style.display = (typeof personaFilesEnabled !== 'undefined' && personaFilesEnabled) ? '' : 'none';
    }
    const cronTabBtn = document.querySelector('.settings-tab[data-tab="cron"]');
    if (cronTabBtn) {
        cronTabBtn.style.display = (typeof cronJobsEnabled !== 'undefined' && cronJobsEnabled) ? '' : 'none';
    }

    // Toggle persona-dependent preset buttons (Heartbeat, Memory Digest)
    document.querySelectorAll('[data-require-persona="true"]').forEach(btn => {
        btn.style.display = (typeof personaFilesEnabled !== 'undefined' && personaFilesEnabled) ? '' : 'none';
    });

    document.getElementById('note-settings-modal').classList.remove('hidden');
};

globalThis.hideNoteSettings = function hideNoteSettings() {
    document.getElementById('note-settings-modal').classList.add('hidden');
    settingsNoteId = null;
};

globalThis.switchSettingsTab = function switchSettingsTab(tabName) {
    document.querySelectorAll('.settings-tab').forEach(t => {
        const active = t.dataset.tab === tabName;
        t.classList.toggle('active', active);
        t.setAttribute('aria-selected', active ? 'true' : 'false');
    });

    document.querySelectorAll('.settings-tab-panel').forEach(p => {
        p.classList.add('hidden');
    });

    const activePanel = document.getElementById(`settings-tab-${tabName}`);
    if (activePanel) {
        activePanel.classList.remove('hidden');
    }

    // Only show Delete button on General tab (and never for 'default' note)
    const delBtn = document.getElementById('btn-delete-note');
    if (delBtn) {
        delBtn.style.display = (tabName === 'general' && settingsNoteId !== 'default') ? '' : 'none';
    }

    const saveBtn = document.getElementById('btn-save-note-settings');
    if (saveBtn) {
        saveBtn.style.display = tabName === 'general' ? '' : 'none';
    }

    const modalActions = document.querySelector('#note-settings-modal .modal-actions');
    if (modalActions) {
        modalActions.style.display = tabName === 'general' ? '' : 'none';
    }

    if (tabName === 'persona') {
        loadPersonaStatus();
    } else if (tabName === 'cron') {
        loadCronJobs();
    }
};

globalThis.loadPersonaStatus = async function loadPersonaStatus() {
    if (!settingsNoteId) return;
    const grid = document.getElementById('persona-files-grid');
    if (!grid) return;
    grid.innerHTML = '<div class="loading-spinner">Loading persona files...</div>';

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/persona/status`, undefined, 'GET');
    if (!r || !r.ok) {
        grid.innerHTML = `<div class="error-msg">Failed to load persona status: ${(r && r.error) || 'Network error'}</div>`;
        return;
    }

    const files = r.files || [];
    if (r.persona_files_enabled !== undefined && typeof personaFilesEnabled !== 'undefined') {
        personaFilesEnabled = !!r.persona_files_enabled;
        const personaTabBtn = document.querySelector('.settings-tab[data-tab="persona"]');
        if (personaTabBtn) personaTabBtn.style.display = personaFilesEnabled ? '' : 'none';
    }
    grid.replaceChildren();

    files.forEach(f => {
        const card = document.createElement('div');
        card.className = `persona-card ${f.present ? 'present' : 'missing'} ${f.name === 'HEARTBEAT.md' ? 'featured' : ''}`;
        
        const header = document.createElement('div');
        header.className = 'persona-card-header';

        const nameEl = document.createElement('span');
        nameEl.className = 'persona-card-name';
        nameEl.textContent = f.name;

        const badge = document.createElement('span');
        badge.className = `persona-badge ${f.present ? 'badge-present' : 'badge-missing'}`;
        badge.textContent = f.present ? '✓ Present' : '○ Missing';

        header.append(nameEl, badge);

        const desc = document.createElement('div');
        desc.className = 'persona-card-desc';
        desc.textContent = PERSONA_DESCRIPTIONS[f.name] || 'Persona context file';

        card.append(header, desc);
        grid.appendChild(card);
    });
};

globalThis.initPersonaFiles = async function initPersonaFiles() {
    if (!settingsNoteId) return;
    const btn = document.getElementById('btn-init-persona');
    if (btn) btn.disabled = true;

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/persona/init`, { overwrite: false }, 'POST');
    if (btn) btn.disabled = false;

    if (r && r.ok) {
        loadPersonaStatus();
        if (typeof fetchNoteListAndConnect === 'function') {
            fetchNoteListAndConnect();
        }
    } else if (typeof addSystemMessage === 'function') {
        addSystemMessage('Failed to initialize persona files: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.loadCronJobs = async function loadCronJobs() {
    if (!settingsNoteId) return;
    const list = document.getElementById('cron-jobs-list');
    if (!list) return;
    list.innerHTML = '<div class="loading-spinner">Loading scheduled jobs...</div>';

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs`, undefined, 'GET');
    if (!r || !r.ok) {
        list.innerHTML = `<div class="error-msg">Failed to load scheduled jobs: ${(r && r.error) || 'Network error'}</div>`;
        return;
    }

    currentCronJobs = r.jobs || [];

    if (r.running_job_ids && Array.isArray(r.running_job_ids)) {
        if (!runningCronJobIds || !(runningCronJobIds instanceof Set)) {
            runningCronJobIds = new Set();
        }
        runningCronJobIds.clear();
        r.running_job_ids.forEach(id => runningCronJobIds.add(id));
    }

    if (r.cron_jobs_enabled !== undefined && typeof cronJobsEnabled !== 'undefined') {
        cronJobsEnabled = !!r.cron_jobs_enabled;
        const cronTabBtn = document.querySelector('.settings-tab[data-tab="cron"]');
        if (cronTabBtn) cronTabBtn.style.display = cronJobsEnabled ? '' : 'none';
    }
    if (r.persona_files_enabled !== undefined && typeof personaFilesEnabled !== 'undefined') {
        personaFilesEnabled = !!r.persona_files_enabled;
    }

    // Toggle persona-dependent preset buttons (Heartbeat, Memory Digest)
    document.querySelectorAll('[data-require-persona="true"]').forEach(btn => {
        btn.style.display = (typeof personaFilesEnabled !== 'undefined' && personaFilesEnabled) ? '' : 'none';
    });

    renderCronJobsList();
};

function renderCronJobsList() {
    const list = document.getElementById('cron-jobs-list');
    if (!list) return;
    list.replaceChildren();

    if (currentCronJobs.length === 0) {
        const empty = document.createElement('div');
        empty.className = 'empty-jobs';
        empty.innerHTML = '<p>No scheduled jobs configured for this note yet.</p><p class="sub-text">Click "＋ New Scheduled Job" or select a Quick Preset to automate routine checks.</p>';
        list.appendChild(empty);
        return;
    }

    currentCronJobs.forEach(job => {
        const isRunning = typeof runningCronJobIds !== 'undefined' && runningCronJobIds && runningCronJobIds.has(job.id);
        const card = document.createElement('div');
        card.className = `cron-job-card ${isRunning ? 'running' : (job.enabled ? 'active' : 'paused')}`;

        const header = document.createElement('div');
        header.className = 'cron-card-header';

        const titleGroup = document.createElement('div');
        titleGroup.className = 'cron-title-group';

        const dot = document.createElement('span');
        dot.className = `status-dot ${isRunning ? 'dot-running' : (job.enabled ? 'dot-active' : 'dot-paused')}`;

        const nameEl = document.createElement('span');
        nameEl.className = 'cron-name';
        nameEl.textContent = job.name;

        const stateBadge = document.createElement('span');
        if (isRunning) {
            stateBadge.className = 'job-badge badge-running';
            stateBadge.textContent = '⏳ Running...';
        } else {
            stateBadge.className = `job-badge ${job.enabled ? 'badge-active' : 'badge-paused'}`;
            stateBadge.textContent = job.enabled ? 'Active' : 'Paused';
        }

        const silentBadge = document.createElement('span');
        silentBadge.className = 'job-badge badge-muted';
        silentBadge.textContent = job.silent_if_no_action ? 'Silent OK: ON' : 'Silent OK: OFF';

        const modelBadge = document.createElement('span');
        modelBadge.className = 'job-badge badge-muted';
        modelBadge.textContent = `Model: ${job.model || 'inherit'}`;

        const thinkingBadge = document.createElement('span');
        thinkingBadge.className = 'job-badge badge-muted';
        thinkingBadge.textContent = `Thinking: ${job.thinking || 'inherit'}`;

        const timeoutBadge = document.createElement('span');
        timeoutBadge.className = 'job-badge badge-muted';
        timeoutBadge.textContent = `Timeout: ${job.timeout_secs || 60}s`;

        titleGroup.append(dot, nameEl, stateBadge, silentBadge, modelBadge, thinkingBadge, timeoutBadge);

        // Actions
        const actionsGroup = document.createElement('div');
        actionsGroup.className = 'cron-card-actions';

        const runBtn = document.createElement('button');
        runBtn.type = 'button';
        runBtn.className = `btn-secondary btn-chip ${isRunning ? 'is-running' : ''}`;
        runBtn.dataset.action = 'run-cron-job';
        runBtn.dataset.id = job.id;
        if (isRunning) {
            runBtn.disabled = true;
            runBtn.textContent = '⏳ Running...';
        } else {
            runBtn.textContent = '▶ Run Now';
        }

        const logsBtn = document.createElement('button');
        logsBtn.type = 'button';
        logsBtn.className = 'btn-secondary btn-chip';
        logsBtn.dataset.action = 'view-cron-logs';
        logsBtn.dataset.id = job.id;
        logsBtn.textContent = '⚙ Logs';

        const editBtn = document.createElement('button');
        editBtn.type = 'button';
        editBtn.className = 'btn-secondary btn-chip';
        editBtn.dataset.action = 'edit-cron-job';
        editBtn.dataset.id = job.id;
        editBtn.textContent = '✏ Edit';

        const toggleLabel = document.createElement('label');
        toggleLabel.className = 'toggle-switch';
        const toggleInput = document.createElement('input');
        toggleInput.type = 'checkbox';
        toggleInput.checked = job.enabled;
        toggleInput.dataset.action = 'toggle-cron-job';
        toggleInput.dataset.id = job.id;
        const toggleSlider = document.createElement('span');
        toggleSlider.className = 'toggle-slider';
        toggleLabel.append(toggleInput, toggleSlider);

        const deleteBtn = document.createElement('button');
        deleteBtn.type = 'button';
        deleteBtn.className = 'btn-danger btn-chip';
        deleteBtn.dataset.action = 'delete-cron-job';
        deleteBtn.dataset.id = job.id;
        deleteBtn.textContent = '🗑';

        actionsGroup.append(runBtn, logsBtn, editBtn, toggleLabel, deleteBtn);
        header.append(titleGroup, actionsGroup);

        const meta = document.createElement('div');
        meta.className = 'cron-meta-row';
        const lastRunDesc = job.last_run_at ? `${job.last_run_at} (${job.last_status || 'done'})` : 'Never';
        meta.innerHTML = `<span>Schedule: <strong class="highlight-text">${job.schedule_type === 'interval' ? 'Every ' + job.schedule_value : job.schedule_value}</strong></span> &nbsp;•&nbsp; <span>Last Run: <strong>${lastRunDesc}</strong></span>`;

        const promptRow = document.createElement('div');
        promptRow.className = 'cron-prompt-preview';
        promptRow.textContent = `Prompt: ${job.prompt}`;

        card.append(header, meta, promptRow);
        list.appendChild(card);
    });
}

globalThis.setCronScheduleType = function setCronScheduleType(type, customVal = null) {
    const chipsContainer = document.getElementById('cron-interval-chips');
    const input = document.getElementById('cron-schedule-value');
    const typeRadios = document.getElementsByName('cron-schedule-type');

    typeRadios.forEach(r => {
        r.checked = r.value === type;
    });

    if (type === 'cron') {
        if (chipsContainer) chipsContainer.style.display = 'none';
        if (input) {
            input.placeholder = 'e.g. */30 * * * * or 0 9 * * 1';
            if (customVal !== null && customVal !== undefined) {
                input.value = customVal;
            } else {
                const curr = input.value.trim();
                if (!curr || /^\d+[smhd]$/i.test(curr)) {
                    if (curr === '15m') input.value = '*/15 * * * *';
                    else if (curr === '30m') input.value = '*/30 * * * *';
                    else if (curr === '1h') input.value = '0 * * * *';
                    else if (curr === '6h') input.value = '0 */6 * * *';
                    else if (curr === '1d') input.value = '0 0 * * *';
                    else input.value = '*/30 * * * *';
                }
            }
        }
    } else {
        if (chipsContainer) chipsContainer.style.display = '';
        if (input) {
            input.placeholder = 'e.g. 30m, 1h, 1d';
            let targetVal = '30m';
            if (customVal !== null && customVal !== undefined) {
                targetVal = customVal;
            } else {
                const curr = input.value.trim();
                if (/^\d+[smhd]$/i.test(curr)) {
                    targetVal = curr;
                } else if (curr === '*/15 * * * *') {
                    targetVal = '15m';
                } else if (curr === '*/30 * * * *') {
                    targetVal = '30m';
                } else if (curr === '0 * * * *') {
                    targetVal = '1h';
                } else if (curr === '0 */6 * * *') {
                    targetVal = '6h';
                } else if (curr === '0 0 * * *') {
                    targetVal = '1d';
                } else {
                    targetVal = '30m';
                }
            }
            input.value = targetVal;
            document.querySelectorAll('.interval-chips .chip-btn').forEach(btn => {
                btn.classList.toggle('active', btn.dataset.val === targetVal);
            });
        }
    }
};

globalThis.parseDurationSeconds = function parseDurationSeconds(val) {
    if (typeof val === 'number') return Math.round(val);
    if (!val) return 60;
    const s = String(val).trim();
    if (/^\d+$/.test(s)) return parseInt(s, 10);
    let totalSecs = 0, totalMs = 0, matched = false;
    const re = /(\d+(?:\.\d+)?)\s*(ms|s|m|h|d)/gi;
    let match;
    while ((match = re.exec(s)) !== null) {
        matched = true;
        const num = parseFloat(match[1]);
        const unit = match[2].toLowerCase();
        if (unit === 'ms') totalMs += num;
        else if (unit === 's') totalSecs += num;
        else if (unit === 'm') totalSecs += num * 60;
        else if (unit === 'h') totalSecs += num * 3600;
        else if (unit === 'd') totalSecs += num * 86400;
    }
    if (matched) {
        const total = totalSecs + (totalMs / 1000);
        return Math.round(total);
    }
    const parsed = parseInt(s, 10);
    return isNaN(parsed) ? 60 : parsed;
};

globalThis.showCronJobModal = function showCronJobModal(job = null) {
    const modal = document.getElementById('cron-job-modal');
    if (!modal) return;

    const errEl = document.getElementById('cron-job-form-error');
    if (errEl) {
        errEl.textContent = '';
        errEl.classList.add('hidden');
    }

    // Populate model options
    const modelSelect = document.getElementById('cron-job-model');
    const modelsList = (typeof availableModels !== 'undefined' && Array.isArray(availableModels))
        ? availableModels
        : ((typeof globalThis.availableModels !== 'undefined' && Array.isArray(globalThis.availableModels)) ? globalThis.availableModels : []);

    if (modelSelect) {
        modelSelect.innerHTML = '';
        const defaultOpt = document.createElement('option');
        defaultOpt.value = '';
        defaultOpt.textContent = 'Inherit Note Default';
        modelSelect.appendChild(defaultOpt);
        modelsList.forEach(m => {
            const mId = typeof m === 'string' ? m : (m.id || '');
            if (mId) {
                const opt = document.createElement('option');
                opt.value = mId;
                opt.textContent = mId;
                modelSelect.appendChild(opt);
            }
        });
        modelSelect.value = job ? (job.model || '') : '';
    }

    const thinkingSelect = document.getElementById('cron-job-thinking');
    if (thinkingSelect) {
        thinkingSelect.value = job ? (job.thinking || '') : '';
    }

    if (job) {
        document.getElementById('cron-job-modal-title').textContent = 'Edit Scheduled Job';
        document.getElementById('cron-job-id').value = job.id || '';
        document.getElementById('cron-job-name').value = job.name || '';
        document.getElementById('cron-job-prompt').value = job.prompt || '';
        if (modelSelect) modelSelect.value = job.model || '';
        if (thinkingSelect) thinkingSelect.value = job.thinking || '';
        document.getElementById('cron-job-silent').checked = !!job.silent_if_no_action;
        document.getElementById('cron-job-enabled').checked = job.enabled !== false;
        const timeoutInput = document.getElementById('cron-job-timeout');
        if (timeoutInput) timeoutInput.value = job.timeout_secs ? `${job.timeout_secs}s` : '60s';
        
        setCronScheduleType(job.schedule_type || 'interval', job.schedule_value || (job.schedule_type === 'cron' ? '*/30 * * * *' : '30m'));
    } else {
        document.getElementById('cron-job-modal-title').textContent = 'New Scheduled Job';
        document.getElementById('cron-job-id').value = '';
        document.getElementById('cron-job-name').value = '';
        document.getElementById('cron-job-prompt').value = '';
        if (modelSelect) modelSelect.value = '';
        if (thinkingSelect) thinkingSelect.value = '';
        document.getElementById('cron-job-silent').checked = true;
        document.getElementById('cron-job-enabled').checked = true;
        const timeoutInput = document.getElementById('cron-job-timeout');
        if (timeoutInput) timeoutInput.value = '60s';
        
        setCronScheduleType('interval', '30m');
    }

    modal.classList.remove('hidden');
};

globalThis.hideCronJobModal = function hideCronJobModal() {
    const modal = document.getElementById('cron-job-modal');
    if (modal) modal.classList.add('hidden');
};

globalThis.selectIntervalChip = function selectIntervalChip(val) {
    setCronScheduleType('interval', val);
};

globalThis.applyCronPreset = function applyCronPreset(presetType) {
    if (presetType === 'heartbeat') {
        showCronJobModal({
            id: '',
            name: 'Heartbeat Periodic Check',
            schedule_type: 'interval',
            schedule_value: '30m',
            prompt: 'Follow @HEARTBEAT.md to review pending tasks and system health. If no action is needed, reply with "HEARTBEAT_OK".',
            model: null,
            thinking: null,
            silent_if_no_action: true,
            enabled: true,
            timeout_secs: 60,
        });
    } else if (presetType === 'memory') {
        showCronJobModal({
            id: '',
            name: 'Daily Memory Distillation',
            schedule_type: 'cron',
            schedule_value: '0 0 * * *',
            prompt: 'Summarize highlights and key notes from memory-YYYY-MM-DD.md and distill updates into MEMORY.md.',
            model: null,
            thinking: null,
            silent_if_no_action: false,
            enabled: true,
            timeout_secs: 60,
        });
    } else if (presetType === 'weekly') {
        showCronJobModal({
            id: '',
            name: 'Weekly Summary Report',
            schedule_type: 'cron',
            schedule_value: '0 9 * * 1',
            prompt: 'Summarize notes and progress from the past week, and generate a weekly report saved to weekly-report.md.',
            model: null,
            thinking: null,
            silent_if_no_action: false,
            enabled: true,
            timeout_secs: 60,
        });
    }
};

function setCronFormError(msg, focusId = null) {
    const errEl = document.getElementById('cron-job-form-error');
    if (errEl) {
        errEl.textContent = msg;
        errEl.classList.remove('hidden');
    } else if (typeof addSystemMessage === 'function') {
        addSystemMessage('Error: ' + msg);
    }
    if (focusId) {
        const input = document.getElementById(focusId);
        if (input && typeof input.focus === 'function') input.focus();
    }
}

globalThis.saveCronJob = async function saveCronJob() {
    if (!settingsNoteId) return;

    const id = (document.getElementById('cron-job-id')?.value || '').trim();
    const name = (document.getElementById('cron-job-name')?.value || '').trim();
    const prompt = (document.getElementById('cron-job-prompt')?.value || '').trim();
    const schedule_value = (document.getElementById('cron-schedule-value')?.value || '').trim();
    
    let schedule_type = 'interval';
    document.getElementsByName('cron-schedule-type').forEach(r => {
        if (r.checked) schedule_type = r.value;
    });

    const modelSelect = document.getElementById('cron-job-model');
    const model = modelSelect && modelSelect.value ? modelSelect.value : null;
    const thinkingSelect = document.getElementById('cron-job-thinking');
    const thinking = thinkingSelect && thinkingSelect.value ? thinkingSelect.value : null;
    const silent_if_no_action = !!document.getElementById('cron-job-silent')?.checked;
    const enabled = document.getElementById('cron-job-enabled') ? document.getElementById('cron-job-enabled').checked : true;

    const timeoutInput = document.getElementById('cron-job-timeout');
    let timeout_secs = timeoutInput ? parseDurationSeconds(timeoutInput.value) : 60;
    if (isNaN(timeout_secs) || timeout_secs < 5) timeout_secs = 60;
    if (timeout_secs > 3600) timeout_secs = 3600;

    if (!name) {
        setCronFormError('Please enter a job name.', 'cron-job-name');
        return;
    }
    if (!prompt) {
        setCronFormError('Please enter an instruction prompt.', 'cron-job-prompt');
        return;
    }
    if (!schedule_value) {
        setCronFormError('Please enter a schedule value.', 'cron-schedule-value');
        return;
    }

    const payload = {
        name,
        schedule_type,
        schedule_value,
        prompt,
        model,
        thinking,
        silent_if_no_action,
        enabled,
        timeout_secs,
    };

    let r;
    if (id) {
        r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(id)}`, payload, 'PUT');
    } else {
        r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs`, payload, 'POST');
    }

    if (r && r.ok) {
        hideCronJobModal();
        loadCronJobs();
    } else {
        setCronFormError('Failed to save scheduled job: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.editCronJob = function editCronJob(jobId) {
    const job = currentCronJobs.find(j => j.id === jobId);
    if (job) {
        showCronJobModal(job);
    }
};

globalThis.runCronJob = async function runCronJob(jobId) {
    if (!settingsNoteId) return;
    if (typeof runningCronJobIds !== 'undefined' && runningCronJobIds && runningCronJobIds.has(jobId)) {
        return;
    }
    if (typeof runningCronJobIds !== 'undefined' && runningCronJobIds) {
        runningCronJobIds.add(jobId);
        renderCronJobsList();
    }
    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}/run`, {}, 'POST');
    if (typeof runningCronJobIds !== 'undefined' && runningCronJobIds) {
        runningCronJobIds.delete(jobId);
    }
    if (r && r.ok) {
        loadCronJobs();
    } else {
        if (typeof addSystemMessage === 'function') {
            addSystemMessage('Failed to run scheduled job: ' + ((r && r.error) || 'Unknown error'));
        }
        renderCronJobsList();
    }
};

globalThis.toggleCronJob = async function toggleCronJob(jobId, enabled) {
    if (!settingsNoteId) return;
    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}`, { enabled }, 'PUT');
    if (r && r.ok) {
        loadCronJobs();
    } else if (typeof addSystemMessage === 'function') {
        addSystemMessage('Failed to toggle scheduled job: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.deleteCronJob = async function deleteCronJob(jobId) {
    if (!settingsNoteId) return;
    const ok = await showDialog({ title: 'Delete Scheduled Job', message: 'Delete this scheduled job and its execution logs?', danger: true, okLabel: 'Delete Job' });
    if (!ok) return;

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}`, undefined, 'DELETE');
    if (r && r.ok) {
        loadCronJobs();
    } else if (typeof addSystemMessage === 'function') {
        addSystemMessage('Failed to delete scheduled job: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.viewCronLogs = async function viewCronLogs(jobId) {
    if (!settingsNoteId) return;
    const modal = document.getElementById('cron-logs-modal');
    const tbody = document.getElementById('cron-logs-tbody');
    const statsBar = document.getElementById('cron-logs-stats');
    if (!modal || !tbody) return;

    tbody.innerHTML = '<tr><td colspan="6" class="loading-td">Loading logs...</td></tr>';
    modal.classList.remove('hidden');

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}/logs?limit=50`, undefined, 'GET');
    if (!r || !r.ok) {
        tbody.innerHTML = `<tr><td colspan="6" class="error-td">Failed to load logs: ${(r && r.error) || 'Network error'}</td></tr>`;
        return;
    }

    const logs = r.logs || [];
    tbody.replaceChildren();

    if (logs.length === 0) {
        tbody.innerHTML = '<tr><td colspan="6" class="empty-td">No execution logs recorded yet.</td></tr>';
        if (statsBar) statsBar.textContent = 'Total Runs: 0';
        return;
    }

    // Stats
    const totalRuns = logs.length;
    const silentRuns = logs.filter(l => l.status === 'silent_ok').length;
    const successRuns = logs.filter(l => l.status === 'success').length;
    const errorRuns = logs.filter(l => l.status === 'error' || l.status === 'timeout').length;
    const avgDuration = Math.round(logs.reduce((acc, l) => acc + (l.duration_ms || 0), 0) / totalRuns);

    if (statsBar) {
        statsBar.innerHTML = `
            <span>Total Runs: <strong>${totalRuns}</strong></span>
            <span>Success: <strong class="badge-present">${successRuns + silentRuns}</strong></span>
            <span>Silent Skips: <strong class="badge-silent">${silentRuns}</strong></span>
            <span>Errors: <strong class="badge-missing">${errorRuns}</strong></span>
            <span>Avg Duration: <strong>${avgDuration}ms</strong></span>
        `;
    }

    logs.forEach((log, index) => {
        const tr = document.createElement('tr');
        tr.className = 'log-row';
        tr.dataset.action = 'toggle-log-detail';
        tr.dataset.index = String(index);
        tr.role = 'button';
        tr.tabIndex = 0;
        tr.setAttribute('aria-expanded', 'false');
        tr.setAttribute('aria-label', `View details for log at ${log.timestamp}`);

        const statusClass = log.status === 'silent_ok' ? 'status-silent' : (log.status === 'success' ? 'status-success' : 'status-error');

        const expandIcon = document.createElement('span');
        expandIcon.className = 'log-expand-icon';
        expandIcon.textContent = '▶';

        const timeTd = document.createElement('td');
        timeTd.append(expandIcon, ' ' + log.timestamp);

        const triggerTd = document.createElement('td');
        triggerTd.innerHTML = `<span class="trigger-badge">${log.trigger}</span>`;

        const statusTd = document.createElement('td');
        statusTd.innerHTML = `<span class="log-status-badge ${statusClass}">${log.status}</span>`;

        const durationTd = document.createElement('td');
        durationTd.textContent = `${log.duration_ms}ms`;

        const filesTd = document.createElement('td');
        filesTd.textContent = (log.files_modified && log.files_modified.length > 0) ? log.files_modified.join(', ') : '0 files';

        const snippetTd = document.createElement('td');
        snippetTd.className = 'snippet-cell';
        snippetTd.title = log.output_snippet || log.error || '';
        snippetTd.textContent = log.output_snippet || (log.error ? 'Error: ' + log.error : '—');

        tr.append(timeTd, triggerTd, statusTd, durationTd, filesTd, snippetTd);
        tbody.appendChild(tr);

        // Detail row (hidden by default)
        const detailTr = document.createElement('tr');
        detailTr.className = 'log-detail-row hidden';
        detailTr.id = `log-detail-${index}`;

        const detailTd = document.createElement('td');
        detailTd.colSpan = 6;

        const detailContent = document.createElement('div');
        detailContent.className = 'log-detail-content';

        const metaBox = document.createElement('div');
        metaBox.className = 'log-detail-meta';

        let tokensStr = '—';
        if (log.tokens_in != null || log.tokens_out != null || log.tokens_used != null) {
            const tIn = log.tokens_in != null ? `${log.tokens_in} in` : null;
            const tOut = log.tokens_out != null ? `${log.tokens_out} out` : null;
            const tTot = log.tokens_used != null ? `${log.tokens_used} total` : null;
            if (tIn && tOut) {
                tokensStr = `${tIn} / ${tOut} (${tTot || (log.tokens_in + log.tokens_out) + ' total'})`;
            } else if (tTot) {
                tokensStr = tTot;
            }
        }

        const toolsStr = (log.tools && log.tools.length > 0) ? log.tools.join(', ') : 'None';
        const filesStr = (log.files_modified && log.files_modified.length > 0) ? log.files_modified.join(', ') : 'None';

        metaBox.innerHTML = `
            <div><strong>Timestamp (UTC):</strong> <span>${log.timestamp}</span></div>
            <div><strong>Trigger Type:</strong> <span>${log.trigger}</span></div>
            <div><strong>Status:</strong> <span class="log-status-badge ${statusClass}">${log.status}</span></div>
            <div><strong>Duration:</strong> <span>${log.duration_ms}ms</span></div>
            <div><strong>Model:</strong> <span>${log.model || '—'}</span></div>
            <div><strong>Thinking:</strong> <span>${log.thinking || '—'}</span></div>
            <div><strong>Steps:</strong> <span>${log.steps != null ? log.steps : '—'}</span></div>
            <div><strong>Tokens:</strong> <span>${tokensStr}</span></div>
            <div><strong>Tools Used:</strong> <span>${toolsStr}</span></div>
            <div><strong>Files Modified:</strong> <span>${filesStr}</span></div>
        `;
        detailContent.appendChild(metaBox);

        if (log.error) {
            const errBox = document.createElement('div');
            errBox.className = 'log-detail-error';
            const errTitle = document.createElement('strong');
            errTitle.textContent = 'Error Details:';
            const errPre = document.createElement('pre');
            errPre.textContent = log.error;
            errBox.append(errTitle, errPre);
            detailContent.appendChild(errBox);
        }

        const outBox = document.createElement('div');
        outBox.className = 'log-detail-output';

        const outHeader = document.createElement('div');
        outHeader.className = 'log-detail-output-header';
        const outTitle = document.createElement('strong');
        outTitle.textContent = 'Execution Output / Response:';

        const copyBtn = document.createElement('button');
        copyBtn.type = 'button';
        copyBtn.className = 'btn-secondary btn-chip';
        copyBtn.dataset.action = 'copy-log-output';
        copyBtn.textContent = '📋 Copy Output';

        outHeader.append(outTitle, copyBtn);

        const outPre = document.createElement('pre');
        outPre.className = 'log-output-pre';
        outPre.textContent = log.output_snippet || '(No output recorded)';

        outBox.append(outHeader, outPre);
        detailContent.appendChild(outBox);

        detailTd.appendChild(detailContent);
        detailTr.appendChild(detailTd);
        tbody.appendChild(detailTr);
    });
};

globalThis.toggleLogDetail = function toggleLogDetail(rowElement) {
    const tr = rowElement.closest('.log-row');
    if (!tr) return;
    const index = tr.dataset.index;
    const detailRow = document.getElementById(`log-detail-${index}`);
    if (!detailRow) return;

    const isHidden = detailRow.classList.contains('hidden');
    detailRow.classList.toggle('hidden', !isHidden);
    tr.classList.toggle('is-expanded', isHidden);
    tr.setAttribute('aria-expanded', isHidden ? 'true' : 'false');
    const icon = tr.querySelector('.log-expand-icon');
    if (icon) icon.textContent = isHidden ? '▼' : '▶';
};

globalThis.copyLogOutput = function copyLogOutput(button) {
    const detailBox = button.closest('.log-detail-output');
    const pre = detailBox ? detailBox.querySelector('.log-output-pre') : null;
    const text = pre ? pre.textContent : '';
    if (!text) return;

    const done = () => {
        const orig = button.textContent;
        button.textContent = '✓ Copied!';
        setTimeout(() => { button.textContent = orig; }, 1500);
    };

    if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).then(done).catch(() => {
            if (typeof fallbackCopy === 'function') fallbackCopy(text, done);
            else done();
        });
    } else if (typeof fallbackCopy === 'function') {
        fallbackCopy(text, done);
    } else {
        done();
    }
};

globalThis.hideCronLogsModal = function hideCronLogsModal() {
    const modal = document.getElementById('cron-logs-modal');
    if (modal) modal.classList.add('hidden');
};

globalThis.saveNoteSettings = function saveNoteSettings() {
    if (!settingsNoteId) return;
    const name = document.getElementById('note-settings-name').value.trim();
    const s = notes.find(x => x.id === settingsNoteId);
    if (s && name) {
        if (name !== s.name || selectedNoteIcon !== (s.icon || null)) {
            api('notes/' + encodeURIComponent(settingsNoteId), { name, icon: selectedNoteIcon }, 'PATCH');
        }
    }
    hideNoteSettings();
};

globalThis.deleteCurrentNote = async function deleteCurrentNote() {
    if (!settingsNoteId) return;
    const ok = await showDialog({ title: 'Delete Note', message: 'Delete this note? Chat history will be preserved.', danger: true, okLabel: 'Delete Note' });
    if (!ok) return;
    const deletedId = settingsNoteId;
    api('notes/' + encodeURIComponent(deletedId), undefined, 'DELETE');
    hideNoteSettings();
    if (currentNoteId === deletedId) {
        if (evtSource) { evtSource.close(); evtSource = null; }
        isConnected = false;
        currentNoteId = '';
        localStorage.removeItem('rune_note');
        updateChatInputState();
        fetchNoteListAndConnect();
    }
};

// --- Directory Browser ---

globalThis.openDirBrowser = function openDirBrowser(targetInputId) {
    dirBrowserTargetInput = document.getElementById(targetInputId);
    const startPath = dirBrowserTargetInput ? (dirBrowserTargetInput.value || '/') : '/';
    document.getElementById('dir-browser-modal').classList.remove('hidden');
    navigateDir(startPath || '/');
};

globalThis.hideDirBrowser = function hideDirBrowser() {
    document.getElementById('dir-browser-modal').classList.add('hidden');
    dirBrowserTargetInput = null;
};

globalThis.navigateDir = function navigateDir(path) {
    document.getElementById('dir-browser-path').value = path;
    api('dirs?path=' + encodeURIComponent(path), undefined, 'GET').then(r => { if (r.ok && r.data) handleMessage(r.data); });
};

globalThis.renderDirBrowser = function renderDirBrowser(path, parent, entries) {
    document.getElementById('dir-browser-path').value = path;
    const list = document.getElementById('dir-browser-list');
    list.replaceChildren();
    if (parent) {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        appendDirEntry(el, 'arrow-up', '..', parent);
        list.appendChild(el);
    }
    entries.forEach(e => {
        const el = document.createElement('div');
        el.className = 'dir-entry';
        appendDirEntry(el, 'folder', e.name, path + (path.endsWith('/') ? '' : '/') + e.name);
        list.appendChild(el);
    });
};

globalThis.selectDir = function selectDir() {
    const path = document.getElementById('dir-browser-path').value;
    if (dirBrowserTargetInput) {
        dirBrowserTargetInput.value = path;
    }
    hideDirBrowser();
};

function appendDirEntry(row, iconName, nameText, targetPath) {
    const icon = document.createElement('span');
    icon.className = 'dir-entry-icon';
    icon.appendChild(runeIcon(iconName));
    const name = document.createElement('span');
    name.className = 'dir-entry-name';
    name.textContent = nameText;
    row.role = 'button';
    row.tabIndex = 0;
    row.setAttribute('aria-label', `Open ${nameText}`);
    row.dataset.action = 'navigate-entry';
    row.dataset.path = targetPath;
    row.append(icon, name);
}
