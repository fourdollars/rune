// options.js — settings page: multi-server management, switching, and OAuth PKCE logins.

import {
  getServers,
  getActiveServerUrl,
  saveServer,
  setActiveServer,
  updateServerName,
  saveServersOrder,
  removeServer,
  normalizeServerUrl,
} from './common.js';

const $serverList = document.getElementById('server-list');
const $emptyState = document.getElementById('empty-state');
const $addForm = document.getElementById('add-server-form');
const $serverName = document.getElementById('serverName');
const $serverUrl = document.getElementById('serverUrl');
const $setAsActive = document.getElementById('setAsActive');
const $btnAddAndLogin = document.getElementById('btnAddAndLogin');
const $btnAddOnly = document.getElementById('btnAddOnly');
const $statusBanner = document.getElementById('status-banner');

let statusTimer = null;

function showStatus(text, type = 'info', autoHide = true) {
  if (statusTimer) clearTimeout(statusTimer);
  $statusBanner.textContent = text;
  $statusBanner.className = `show ${type}`;

  if (autoHide) {
    statusTimer = setTimeout(() => {
      $statusBanner.className = '';
    }, 6000);
  }
}

function escapeHtml(str) {
  return String(str ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// Helper to determine the drag insertion target element
function getDragAfterElement(container, y) {
  const draggableElements = [...container.querySelectorAll('.server-card:not(.dragging)')];
  return draggableElements.reduce((closest, child) => {
    const box = child.getBoundingClientRect();
    const offset = y - box.top - box.height / 2;
    if (offset < 0 && offset > closest.offset) {
      return { offset: offset, element: child };
    } else {
      return closest;
    }
  }, { offset: Number.NEGATIVE_INFINITY }).element;
}

// Container-level dragover to reorder cards in real time as the user drags
if ($serverList) {
  $serverList.addEventListener('dragover', (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    const dragging = $serverList.querySelector('.dragging');
    if (!dragging) return;
    const afterElement = getDragAfterElement($serverList, e.clientY);
    if (afterElement == null) {
      $serverList.appendChild(dragging);
    } else {
      $serverList.insertBefore(dragging, afterElement);
    }
  });
}

// SVG Icons
const ICONS = {
  star: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>`,
  starFilled: `<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>`,
  login: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><polyline points="10 17 15 12 10 7"/><line x1="15" y1="12" x2="3" y2="12"/></svg>`,
  refresh: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67"/></svg>`,
  logout: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>`,
  trash: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>`,
  pencil: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z"/></svg>`,
};

async function renderServers() {
  const servers = await getServers();
  const activeUrl = await getActiveServerUrl();

  if (!servers || servers.length === 0) {
    $serverList.innerHTML = '';
    $emptyState.style.display = 'block';
    return;
  }

  $emptyState.style.display = 'none';
  $serverList.innerHTML = '';

  const serverAuthList = await Promise.all(
    servers.map(async (server) => {
      const authStatus = await browser.runtime.sendMessage({
        type: 'rune:authStatus',
        serverUrl: server.url,
      }).catch(() => ({ loggedIn: false }));
      return { server, isLoggedIn: Boolean(authStatus?.loggedIn) };
    })
  );

  serverAuthList.forEach(({ server, isLoggedIn }, index) => {
    const isActive = server.url === activeUrl;

    const card = document.createElement('div');
    card.className = `server-card ${isActive ? 'active' : ''}`;
    card.dataset.url = server.url;
    card.dataset.index = String(index);
    card.draggable = true;

    card.innerHTML = `
      <div class="server-card-top">
        <div class="server-name-row">
          <span class="drag-handle" title="Drag to reorder" aria-label="Drag to reorder">⠿</span>
          <span class="server-name" title="Click to rename">${escapeHtml(server.name || server.url)}</span>
          <button type="button" class="btn-icon btn-rename" title="Rename server" aria-label="Rename server">${ICONS.pencil}</button>
          ${isActive ? '<span class="badge badge-active">Active</span>' : ''}
        </div>
        <div class="server-status">
          ${isLoggedIn
            ? '<span class="status-pill logged-in">🟢 Logged in</span>'
            : '<span class="status-pill logged-out">⚪ Not logged in</span>'}
        </div>
      </div>
      <div class="server-card-bottom">
        <div class="server-url" title="${escapeHtml(server.url)}">${escapeHtml(server.url)}</div>
        <div class="server-actions">
          ${!isActive
            ? `<button type="button" class="btn-icon-action btn-activate" title="Set as active server" aria-label="Set as active server">${ICONS.star}</button>`
            : `<button type="button" class="btn-icon-action btn-activate-active" title="Currently active" aria-label="Currently active" disabled>${ICONS.starFilled}</button>`}
          ${isLoggedIn
            ? `<button type="button" class="btn-icon-action btn-reauth" title="Re-authorize OAuth" aria-label="Re-authorize OAuth">${ICONS.refresh}</button>
               <button type="button" class="btn-icon-action btn-logout" title="Log Out" aria-label="Log Out">${ICONS.logout}</button>`
            : `<button type="button" class="btn-icon-action btn-login" title="Log In" aria-label="Log In">${ICONS.login}</button>`}
          <button type="button" class="btn-icon-action btn-delete" title="Remove server" aria-label="Remove server">${ICONS.trash}</button>
        </div>
      </div>
    `;

    // Drag-and-drop reordering
    card.addEventListener('dragstart', (e) => {
      if (e.target.closest('input, button, .edit-name-box, select, a')) {
        e.preventDefault();
        return;
      }
      card.classList.add('dragging');
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', server.url);
    });

    card.addEventListener('dragend', async () => {
      card.classList.remove('dragging');
      // Read final visual order from the DOM and persist
      const currentOrderUrls = [...$serverList.querySelectorAll('.server-card')].map((c) => c.dataset.url);
      const allServers = await getServers();
      const reordered = [];
      for (const url of currentOrderUrls) {
        const found = allServers.find((s) => s.url === url);
        if (found) reordered.push(found);
      }
      allServers.forEach((s) => {
        if (!reordered.some((r) => r.url === s.url)) reordered.push(s);
      });
      await saveServersOrder(reordered);
      showStatus('Updated server order', 'info');
      await renderServers();
    });

    // Inline rename handling
    const setupRename = () => {
      card.draggable = false;
      const nameRow = card.querySelector('.server-name-row');
      if (!nameRow) return;
      nameRow.innerHTML = `
        <div class="edit-name-box">
          <input type="text" class="input-rename" value="${escapeHtml(server.name || '')}" placeholder="Server Name" />
          <button type="button" class="btn btn-primary btn-sm btn-save-name">Save</button>
          <button type="button" class="btn btn-secondary btn-sm btn-cancel-name">Cancel</button>
        </div>
      `;
      const input = nameRow.querySelector('.input-rename');
      const btnSave = nameRow.querySelector('.btn-save-name');
      const btnCancel = nameRow.querySelector('.btn-cancel-name');
      input.focus();
      input.select();

      const save = async () => {
        const newName = input.value.trim();
        await updateServerName(server.url, newName);
        showStatus(`✅ Updated server name to: ${newName || server.url}`, 'success');
        await renderServers();
      };

      btnSave.addEventListener('click', (e) => {
        e.stopPropagation();
        save();
      });
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          save();
        } else if (e.key === 'Escape') {
          renderServers();
        }
      });
      btnCancel.addEventListener('click', (e) => {
        e.stopPropagation();
        renderServers();
      });
    };

    const btnRename = card.querySelector('.btn-rename');
    if (btnRename) {
      btnRename.addEventListener('click', (e) => {
        e.stopPropagation();
        setupRename();
      });
    }

    const serverNameEl = card.querySelector('.server-name');
    if (serverNameEl) {
      serverNameEl.addEventListener('click', (e) => {
        e.stopPropagation();
        setupRename();
      });
    }

    // Button event listeners
    const btnActivate = card.querySelector('.btn-activate');
    if (btnActivate) {
      btnActivate.addEventListener('click', async () => {
        await setActiveServer(server.url);
        showStatus(`Switched active server to: ${server.name || server.url}`, 'success');
        await renderServers();
      });
    }

    const btnLogin = card.querySelector('.btn-login');
    if (btnLogin) {
      btnLogin.addEventListener('click', async () => {
        showStatus(`Logging in to ${server.url}… (a browser window will open)`, 'info', false);
        const res = await browser.runtime.sendMessage({
          type: 'rune:login',
          serverUrl: server.url,
        });
        if (res?.ok) {
          showStatus(`✅ Successfully logged in to ${server.name || server.url}`, 'success');
        } else {
          showStatus(`⚠️ Login failed: ${res?.error ?? 'unknown error'}`, 'error');
        }
        await renderServers();
      });
    }

    const btnReauth = card.querySelector('.btn-reauth');
    if (btnReauth) {
      btnReauth.addEventListener('click', async () => {
        showStatus(`Re-authorizing with ${server.url}…`, 'info', false);
        const res = await browser.runtime.sendMessage({
          type: 'rune:login',
          serverUrl: server.url,
        });
        if (res?.ok) {
          showStatus(`✅ Re-authorized with ${server.name || server.url}`, 'success');
        } else {
          showStatus(`⚠️ Re-authorization failed: ${res?.error ?? 'unknown error'}`, 'error');
        }
        await renderServers();
      });
    }

    const btnLogout = card.querySelector('.btn-logout');
    if (btnLogout) {
      btnLogout.addEventListener('click', async () => {
        showStatus(`Logging out from ${server.url}…`, 'info', false);
        await browser.runtime.sendMessage({
          type: 'rune:logout',
          serverUrl: server.url,
        });
        showStatus(`Logged out from ${server.name || server.url}`, 'info');
        await renderServers();
      });
    }

    const btnDelete = card.querySelector('.btn-delete');
    if (btnDelete) {
      btnDelete.addEventListener('click', async () => {
        const confirmed = confirm(`Are you sure you want to remove "${server.name || server.url}"?`);
        if (!confirmed) return;
        await removeServer(server.url);
        showStatus(`Removed server: ${server.name || server.url}`, 'info');
        await renderServers();
      });
    }

    $serverList.appendChild(card);
  });
}

async function handleAddServer(andLogin = false) {
  let origin;
  try {
    origin = normalizeServerUrl($serverUrl.value.trim());
  } catch (e) {
    showStatus(`❌ ${e.message}`, 'error');
    return;
  }

  const name = $serverName.value.trim();
  const setActive = $setAsActive.checked;

  // Request host permission dynamically (optional_host_permissions)
  const granted = await browser.permissions.request({ origins: [`${origin}/*`] });
  if (!granted) {
    showStatus('❌ Host permission denied — cannot connect to this URL', 'error');
    return;
  }

  // Verify server reachability
  showStatus(`Verifying connection to ${origin}…`, 'info', false);
  const check = await browser.runtime.sendMessage({
    type: 'rune:checkServer',
    serverUrl: origin,
  });

  if (!check?.ok) {
    showStatus(
      `⚠️ Could not verify connection (${check?.error ?? check?.status}). Please double check the URL.`,
      'warning'
    );
  }

  await saveServer({
    name,
    url: origin,
    setActive,
  });

  $serverName.value = '';
  $serverUrl.value = '';

  if (andLogin) {
    showStatus(`Saved server: ${origin}. Starting login… (a browser window will open)`, 'info', false);
    const loginResp = await browser.runtime.sendMessage({
      type: 'rune:login',
      serverUrl: origin,
    });
    if (loginResp?.ok) {
      showStatus(`✅ Server saved and logged in: ${name || origin}`, 'success');
    } else {
      showStatus(`⚠️ Server saved, but login failed: ${loginResp?.error ?? 'unknown error'}`, 'warning');
    }
  } else {
    showStatus(`✅ Server saved: ${name || origin}`, 'success');
  }

  await renderServers();
}

$addForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  await handleAddServer(true);
});

$btnAddOnly.addEventListener('click', async () => {
  await handleAddServer(false);
});

// Initial render
renderServers();
