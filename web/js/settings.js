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

    // Default to General tab
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

    // Hide delete button for default session
    const delBtn = document.getElementById('btn-delete-note');
    if (delBtn) delBtn.style.display = sessionId === 'default' ? 'none' : '';
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

    const saveBtn = document.getElementById('btn-save-note-settings');
    if (saveBtn) {
        saveBtn.style.display = tabName === 'general' ? '' : 'none';
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
    } else {
        alert('Failed to initialize persona files: ' + ((r && r.error) || 'Unknown error'));
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
        const card = document.createElement('div');
        card.className = `cron-job-card ${job.enabled ? 'active' : 'paused'}`;

        const header = document.createElement('div');
        header.className = 'cron-card-header';

        const titleGroup = document.createElement('div');
        titleGroup.className = 'cron-title-group';

        const dot = document.createElement('span');
        dot.className = `status-dot ${job.enabled ? 'dot-active' : 'dot-paused'}`;

        const nameEl = document.createElement('span');
        nameEl.className = 'cron-name';
        nameEl.textContent = job.name;

        const stateBadge = document.createElement('span');
        stateBadge.className = `job-badge ${job.enabled ? 'badge-active' : 'badge-paused'}`;
        stateBadge.textContent = job.enabled ? 'Active' : 'Paused';

        const silentBadge = document.createElement('span');
        silentBadge.className = 'job-badge badge-muted';
        silentBadge.textContent = job.silent_if_no_action ? 'Silent OK: ON' : 'Silent OK: OFF';

        const modelBadge = document.createElement('span');
        modelBadge.className = 'job-badge badge-muted';
        modelBadge.textContent = `Model: ${job.model || 'inherit'}`;

        titleGroup.append(dot, nameEl, stateBadge, silentBadge, modelBadge);

        // Actions
        const actionsGroup = document.createElement('div');
        actionsGroup.className = 'cron-card-actions';

        const runBtn = document.createElement('button');
        runBtn.type = 'button';
        runBtn.className = 'btn-secondary btn-chip';
        runBtn.dataset.action = 'run-cron-job';
        runBtn.dataset.id = job.id;
        runBtn.textContent = '▶ Run Now';

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

globalThis.showCronJobModal = function showCronJobModal(job = null) {
    const modal = document.getElementById('cron-job-modal');
    if (!modal) return;

    // Populate model options
    const modelSelect = document.getElementById('cron-job-model');
    if (modelSelect && typeof models !== 'undefined') {
        modelSelect.innerHTML = '<option value="">Inherit Note Default</option>';
        models.forEach(m => {
            const opt = document.createElement('option');
            opt.value = m.id;
            opt.textContent = m.id;
            modelSelect.appendChild(opt);
        });
    }

    if (job) {
        document.getElementById('cron-job-modal-title').textContent = 'Edit Scheduled Job';
        document.getElementById('cron-job-id').value = job.id || '';
        document.getElementById('cron-job-name').value = job.name || '';
        document.getElementById('cron-job-prompt').value = job.prompt || '';
        if (modelSelect) modelSelect.value = job.model || '';
        document.getElementById('cron-job-silent').checked = !!job.silent_if_no_action;
        document.getElementById('cron-job-enabled').checked = job.enabled !== false;
        
        setCronScheduleType(job.schedule_type || 'interval', job.schedule_value || (job.schedule_type === 'cron' ? '*/30 * * * *' : '30m'));
    } else {
        document.getElementById('cron-job-modal-title').textContent = 'New Scheduled Job';
        document.getElementById('cron-job-id').value = '';
        document.getElementById('cron-job-name').value = '';
        document.getElementById('cron-job-prompt').value = '';
        if (modelSelect) modelSelect.value = '';
        document.getElementById('cron-job-silent').checked = true;
        document.getElementById('cron-job-enabled').checked = true;
        
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
            silent_if_no_action: true,
            enabled: true,
        });
    } else if (presetType === 'memory') {
        showCronJobModal({
            id: '',
            name: 'Daily Memory Distillation',
            schedule_type: 'cron',
            schedule_value: '0 0 * * *',
            prompt: 'Summarize highlights and key notes from memory-YYYY-MM-DD.md and distill updates into MEMORY.md.',
            model: null,
            silent_if_no_action: false,
            enabled: true,
        });
    } else if (presetType === 'weekly') {
        showCronJobModal({
            id: '',
            name: 'Weekly Summary Report',
            schedule_type: 'cron',
            schedule_value: '0 9 * * 1',
            prompt: 'Summarize notes and progress from the past week, and generate a weekly report saved to weekly-report.md.',
            model: null,
            silent_if_no_action: false,
            enabled: true,
        });
    }
};

globalThis.saveCronJob = async function saveCronJob() {
    if (!settingsNoteId) return;

    const id = document.getElementById('cron-job-id').value.trim();
    const name = document.getElementById('cron-job-name').value.trim();
    const prompt = document.getElementById('cron-job-prompt').value.trim();
    const schedule_value = document.getElementById('cron-schedule-value').value.trim();
    
    let schedule_type = 'interval';
    document.getElementsByName('cron-schedule-type').forEach(r => {
        if (r.checked) schedule_type = r.value;
    });

    const modelSelect = document.getElementById('cron-job-model');
    const model = modelSelect && modelSelect.value ? modelSelect.value : null;
    const silent_if_no_action = document.getElementById('cron-job-silent').checked;
    const enabled = document.getElementById('cron-job-enabled').checked;

    if (!name) {
        alert('Please enter a job name.');
        return;
    }
    if (!prompt) {
        alert('Please enter an instruction prompt.');
        return;
    }
    if (!schedule_value) {
        alert('Please enter a schedule value.');
        return;
    }

    const payload = {
        name,
        schedule_type,
        schedule_value,
        prompt,
        model,
        silent_if_no_action,
        enabled,
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
        alert('Failed to save scheduled job: ' + ((r && r.error) || 'Unknown error'));
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
    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}/run`, {}, 'POST');
    if (r && r.ok) {
        const result = r.result || {};
        alert(`Job executed successfully!\nStatus: ${result.status || 'unknown'}\nDuration: ${result.duration_ms || 0}ms\nOutput: ${result.output_snippet || 'None'}`);
        loadCronJobs();
    } else {
        alert('Failed to run job: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.toggleCronJob = async function toggleCronJob(jobId, enabled) {
    if (!settingsNoteId) return;
    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}`, { enabled }, 'PUT');
    if (r && r.ok) {
        loadCronJobs();
    } else {
        alert('Failed to toggle job: ' + ((r && r.error) || 'Unknown error'));
    }
};

globalThis.deleteCronJob = async function deleteCronJob(jobId) {
    if (!settingsNoteId) return;
    const ok = await showDialog({ title: 'Delete Scheduled Job', message: 'Delete this scheduled job and its execution logs?', danger: true, okLabel: 'Delete Job' });
    if (!ok) return;

    const r = await api(`notes/${encodeURIComponent(settingsNoteId)}/jobs/${encodeURIComponent(jobId)}`, undefined, 'DELETE');
    if (r && r.ok) {
        loadCronJobs();
    } else {
        alert('Failed to delete job: ' + ((r && r.error) || 'Unknown error'));
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

    logs.forEach(log => {
        const tr = document.createElement('tr');
        const statusClass = log.status === 'silent_ok' ? 'status-silent' : (log.status === 'success' ? 'status-success' : 'status-error');

        tr.innerHTML = `
            <td>${log.timestamp}</td>
            <td><span class="trigger-badge">${log.trigger}</span></td>
            <td><span class="log-status-badge ${statusClass}">${log.status}</span></td>
            <td>${log.duration_ms}ms</td>
            <td>${(log.files_modified && log.files_modified.length > 0) ? log.files_modified.join(', ') : '0 files'}</td>
            <td class="snippet-cell" title="${log.output_snippet || ''}">${log.output_snippet || (log.error ? 'Error: ' + log.error : '—')}</td>
        `;
        tbody.appendChild(tr);
    });
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
