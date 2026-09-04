// common.js — shared helpers for Rune Notes browser extension.
// Loaded (as a classic script or ES module, per context) by background.js,
// options.js, and sidepanel.js.
//
// TODO(i18n): all user-facing strings across this extension are English-only
// for now. Add browser.i18n / _locales support later if multi-language UI is
// needed; out of scope for this pass.

/* global browser */

if (typeof globalThis.browser === 'undefined' && typeof globalThis.chrome !== 'undefined') {
  globalThis.browser = globalThis.chrome;
}

/** Storage keys */
export const SYNC_KEYS = {
  serverUrl: 'serverUrl',
  servers: 'servers',
  defaultNotebook: 'defaultNotebook',
  uiPrefs: 'uiPrefs',
};

export const LOCAL_KEYS = {
  authByServer: 'authByServer',
  accessToken: 'accessToken',
  refreshToken: 'refreshToken',
  tokenExpiresAt: 'tokenExpiresAt',
  clientId: 'clientId',
};

/** Read all sync-scoped settings. Returns {} for unset keys. */
export async function getSyncSettings() {
  return browser.storage.sync.get(Object.values(SYNC_KEYS));
}

export async function setSyncSettings(partial) {
  return browser.storage.sync.set(partial);
}

/**
 * Validate a user-entered Rune Notes server URL.
 * Must be absolute http(s) URL with no path/query (we normalize to origin).
 */
export function normalizeServerUrl(input) {
  let url;
  try {
    url = new URL(input);
  } catch (e) {
    throw new Error('Please enter a full URL, e.g. https://rune.example.com');
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('Only http:// or https:// URLs are supported');
  }
  // Normalize: strip trailing slash, drop path/query/hash — we only need the origin.
  return url.origin;
}

/** Get list of configured servers. Migrates single serverUrl if needed. */
export async function getServers() {
  const data = await browser.storage.sync.get(['servers', 'serverUrl']);
  let servers = Array.isArray(data.servers) ? data.servers : [];
  if (servers.length === 0 && data.serverUrl) {
    servers = [
      {
        id: 'server_default',
        name: 'Default',
        url: data.serverUrl,
        createdAt: Date.now(),
      },
    ];
    await browser.storage.sync.set({ servers });
  }
  return servers;
}

/** Get active server URL */
export async function getActiveServerUrl() {
  const data = await browser.storage.sync.get(['serverUrl', 'servers']);
  if (data.serverUrl) return data.serverUrl;
  if (Array.isArray(data.servers) && data.servers.length > 0) {
    const activeUrl = data.servers[0].url;
    await browser.storage.sync.set({ serverUrl: activeUrl });
    return activeUrl;
  }
  return '';
}

/** Save or update a server in the servers list */
export async function saveServer({ id, name, url, setActive = false }) {
  const normUrl = normalizeServerUrl(url);
  const servers = await getServers();
  const existingIdx = servers.findIndex((s) => s.url === normUrl);
  const serverObj = {
    id: id || (existingIdx !== -1 ? servers[existingIdx].id : `server_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`),
    name: (name && name.trim()) ? name.trim() : (existingIdx !== -1 && servers[existingIdx].name ? servers[existingIdx].name : normUrl),
    url: normUrl,
    createdAt: existingIdx !== -1 ? servers[existingIdx].createdAt : Date.now(),
  };

  if (existingIdx !== -1) {
    servers[existingIdx] = serverObj;
  } else {
    servers.push(serverObj);
  }

  const updates = { servers };
  const currentActive = await getActiveServerUrl();
  if (setActive || !currentActive) {
    updates.serverUrl = normUrl;
  }

  await browser.storage.sync.set(updates);
  return serverObj;
}

/** Get active server object { id, name, url } */
export async function getActiveServer() {
  const activeUrl = await getActiveServerUrl();
  if (!activeUrl) return null;
  const servers = await getServers();
  const found = servers.find((s) => s.url === activeUrl);
  return found || { id: 'default', name: activeUrl, url: activeUrl };
}

/** Update server name */
export async function updateServerName(url, newName) {
  const normUrl = normalizeServerUrl(url);
  const servers = await getServers();
  const existingIdx = servers.findIndex((s) => s.url === normUrl);
  if (existingIdx !== -1) {
    servers[existingIdx].name = (newName && newName.trim()) ? newName.trim() : normUrl;
    await browser.storage.sync.set({ servers });
    return servers[existingIdx];
  }
  return null;
}

/** Reorder servers by moving an item from fromIndex to toIndex */
export async function reorderServers(fromIndex, toIndex) {
  const servers = await getServers();
  if (fromIndex < 0 || fromIndex >= servers.length || toIndex < 0 || toIndex >= servers.length || fromIndex === toIndex) {
    return servers;
  }
  const [moved] = servers.splice(fromIndex, 1);
  servers.splice(toIndex, 0, moved);
  await browser.storage.sync.set({ servers });
  return servers;
}

/** Save updated servers list order */
export async function saveServersOrder(servers) {
  if (Array.isArray(servers)) {
    await browser.storage.sync.set({ servers });
  }
  return servers;
}

/** Set active server */
export async function setActiveServer(url) {
  const normUrl = normalizeServerUrl(url);
  await browser.storage.sync.set({ serverUrl: normUrl });
}

/** Remove a server by URL */
export async function removeServer(url) {
  const normUrl = normalizeServerUrl(url);
  let servers = await getServers();
  servers = servers.filter((s) => s.url !== normUrl);
  await browser.storage.sync.set({ servers });

  // Clear local auth for this server
  await clearLocalAuth(normUrl);

  const activeUrl = await getActiveServerUrl();
  if (activeUrl === normUrl) {
    const nextUrl = servers.length > 0 ? servers[0].url : '';
    await browser.storage.sync.set({ serverUrl: nextUrl });
  }
}

/** Read OAuth/token state from local (per-device) storage for a target server or active server. */
export async function getLocalAuth(targetServerUrl) {
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  const localData = await browser.storage.local.get([
    'authByServer',
    'accessToken',
    'refreshToken',
    'tokenExpiresAt',
    'clientId',
  ]);

  if (serverUrl && localData.authByServer && localData.authByServer[serverUrl]) {
    return localData.authByServer[serverUrl];
  }

  if (localData.accessToken) {
    return {
      accessToken: localData.accessToken,
      refreshToken: localData.refreshToken || null,
      tokenExpiresAt: localData.tokenExpiresAt || null,
      clientId: localData.clientId || null,
    };
  }

  return {
    accessToken: null,
    refreshToken: null,
    tokenExpiresAt: null,
    clientId: null,
  };
}

/** Write OAuth/token state to local storage for a target server or active server. */
export async function setLocalAuth(partial, targetServerUrl) {
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  const localData = await browser.storage.local.get(['authByServer']);
  const authByServer = localData.authByServer || {};

  if (serverUrl) {
    authByServer[serverUrl] = {
      ...(authByServer[serverUrl] || {}),
      ...partial,
    };
  }

  const payload = { authByServer };
  const activeUrl = await getActiveServerUrl();
  if (!serverUrl || serverUrl === activeUrl) {
    if ('accessToken' in partial) payload.accessToken = partial.accessToken;
    if ('refreshToken' in partial) payload.refreshToken = partial.refreshToken;
    if ('tokenExpiresAt' in partial) payload.tokenExpiresAt = partial.tokenExpiresAt;
    if ('clientId' in partial) payload.clientId = partial.clientId;
  }

  return browser.storage.local.set(payload);
}

/** Clear OAuth state for a target server or active server. */
export async function clearLocalAuth(targetServerUrl) {
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  const localData = await browser.storage.local.get(['authByServer']);
  const authByServer = localData.authByServer || {};

  if (serverUrl && authByServer[serverUrl]) {
    delete authByServer[serverUrl];
  }

  await browser.storage.local.set({ authByServer });

  const activeUrl = await getActiveServerUrl();
  if (!serverUrl || serverUrl === activeUrl) {
    await browser.storage.local.remove([
      'accessToken',
      'refreshToken',
      'tokenExpiresAt',
      'clientId',
    ]);
  }
}

/** True access token is present and not (yet) expired, with a small safety margin. */
export async function isLoggedIn(targetServerUrl) {
  const { accessToken, tokenExpiresAt } = await getLocalAuth(targetServerUrl);
  if (!accessToken) return false;
  if (tokenExpiresAt && Date.now() > tokenExpiresAt - 5000) return false;
  return true;
}

/**
 * Fetch wrapper that attaches the Authorization header.
 */
export async function apiFetch(path, options = {}, targetServerUrl) {
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  if (!serverUrl) {
    throw new Error('Rune Notes URL is not set yet');
  }
  const { accessToken } = await getLocalAuth(serverUrl);
  const headers = new Headers(options.headers || {});
  if (accessToken) {
    headers.set('Authorization', `Bearer ${accessToken}`);
  }
  const resp = await fetch(`${serverUrl}${path}`, { ...options, headers });
  return resp;
}
