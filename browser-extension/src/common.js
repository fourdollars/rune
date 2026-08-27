// common.js — shared helpers for Rune Notes browser extension.
// Loaded (as a classic script or ES module, per context) by background.js,
// options.js, and sidepanel.js.
//
// TODO(i18n): all user-facing strings across this extension are English-only
// for now. Add browser.i18n / _locales support later if multi-language UI is
// needed; out of scope for this pass.

/* global browser */

/** Storage keys */
export const SYNC_KEYS = {
  serverUrl: 'serverUrl',
  defaultNotebook: 'defaultNotebook',
  uiPrefs: 'uiPrefs',
};

export const LOCAL_KEYS = {
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

/** Read OAuth/token state from local (per-device) storage. */
export async function getLocalAuth() {
  return browser.storage.local.get(Object.values(LOCAL_KEYS));
}

export async function setLocalAuth(partial) {
  return browser.storage.local.set(partial);
}

export async function clearLocalAuth() {
  return browser.storage.local.remove(Object.values(LOCAL_KEYS));
}

/**
 * Validate a user-entered Rune Notes server URL.
 * Must be absolute http(s) URL with no path/query (we normalize to origin).
 * TODO: consider allowing a path prefix if rune is ever served under a subpath.
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

/** True access token is present and not (yet) expired, with a small safety margin. */
export async function isLoggedIn() {
  const { accessToken, tokenExpiresAt } = await getLocalAuth();
  if (!accessToken) return false;
  if (tokenExpiresAt && Date.now() > tokenExpiresAt - 5000) return false;
  return true;
}

/**
 * Fetch wrapper that attaches the Authorization header. There is no
 * refresh-on-401 handling: the server's OAuth token response carries no
 * refresh_token, so an expired/rejected token requires a full interactive
 * re-login (see background.js's startLogin()) rather than a silent refresh.
 */
export async function apiFetch(path, options = {}) {
  const { serverUrl } = await getSyncSettings();
  if (!serverUrl) {
    throw new Error('Rune Server URL is not set yet');
  }
  const { accessToken } = await getLocalAuth();
  const headers = new Headers(options.headers || {});
  if (accessToken) {
    headers.set('Authorization', `Bearer ${accessToken}`);
  }
  const resp = await fetch(`${serverUrl}${path}`, { ...options, headers });
  return resp;
}
