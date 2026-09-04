// background.js — service worker (Chrome) / event page (Firefox).
// Owns: OAuth PKCE flow, token storage, context menu registration,
// message relay between content-script <-> side panel <-> Rune Notes server.

import {
  getSyncSettings,
  getActiveServerUrl,
  getLocalAuth,
  setLocalAuth,
  clearLocalAuth,
  isLoggedIn,
  apiFetch,
} from './common.js';

import {
  generateCodeVerifier,
  generateCodeChallenge,
  generateState,
  registerClient,
  buildAuthorizeUrl,
  parseAuthorizeRedirect,
  exchangeCodeForToken,
} from './oauth-pkce.js';

const CONTEXT_MENU_ID = 'rune-notes-send-to-chat';

// Chrome only: make a left-click on the toolbar icon open the side panel
// directly, instead of requiring right-click -> "Open side panel". Run this
// at top level (not just onInstalled) so it's reliably re-applied whenever
// the service worker restarts, per Chrome's own recommendation. This API
// doesn't exist in Firefox (whose sidebar_action already opens on icon
// click by default), so guard for its presence.
if (globalThis.browser?.sidePanel?.setPanelBehavior) {
  browser.sidePanel
    .setPanelBehavior({ openPanelOnActionClick: true })
    .catch((e) => console.warn('[rune-notes] setPanelBehavior failed:', e));
}

async function updateExtensionTitle(serverUrl, serverName) {
  const displayName = (serverName && serverName.trim()) ? serverName.trim() : (serverUrl || '');
  const title = displayName ? `ᚱᚢᚾᛖ Chat @ ${displayName}` : 'ᚱᚢᚾᛖ Chat';
  if (browser.action?.setTitle) {
    try { await browser.action.setTitle({ title }); } catch (_) {}
  }
  if (browser.sidebarAction?.setTitle) {
    try { await browser.sidebarAction.setTitle({ title }); } catch (_) {}
  }
}

async function refreshExtensionTitle() {
  const syncSettings = await getSyncSettings();
  const serverUrl = syncSettings.serverUrl;
  const servers = Array.isArray(syncSettings.servers) ? syncSettings.servers : [];
  const found = servers.find((s) => s.url === serverUrl);
  const serverName = found?.name || '';
  await updateExtensionTitle(serverUrl, serverName);
}

refreshExtensionTitle().catch(() => {});

if (browser.storage?.onChanged) {
  browser.storage.onChanged.addListener((changes, area) => {
    if (area === 'sync' && (changes.serverUrl || changes.servers)) {
      refreshExtensionTitle().catch(() => {});
    }
  });
}

browser.runtime.onInstalled.addListener(() => {
  browser.contextMenus.create({
    id: CONTEXT_MENU_ID,
    title: 'Send to Rune Chat',
    contexts: ['selection', 'page'],
  });
});

browser.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId !== CONTEXT_MENU_ID) return;
  // TODO: not yet wired up — should open the side panel (it currently only
  // opens via the toolbar-icon click behavior below) and forward
  // info.selectionText / a content-script extraction request into the chat
  // composer.
  console.log('[rune-chat] context menu clicked', info, tab);
});

/**
 * Full OAuth 2.1 Authorization Code + PKCE flow against the user's own
 * Rune Notes server:
 *   1. Dynamic Client Registration (POST /oauth/register) -> client_id.
 *      Re-registers every login for simplicity (the server is a stateless
 *      "open client" model, so this is cheap and avoids stale-client-id
 *      edge cases across server reinstalls).
 *   2. Generate PKCE code_verifier/code_challenge (S256) + anti-CSRF state.
 *   3. browser.identity.launchWebAuthFlow() against /oauth/authorize.
 *   4. Parse the redirected `code`, verify `state` matches.
 *   5. POST /oauth/token (code + code_verifier) -> access_token.
 *
 * NOTE: Rune's current /oauth/token response only returns
 * { access_token, token_type, expires_in } — there is NO refresh_token.
 * So there is no silent refresh; once the access token expires the user
 * must log in again via this same flow.
 */
async function startLogin(targetServerUrl) {
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  if (!serverUrl) {
    throw new Error('Rune Notes URL is not set yet — please configure and authorize it on the settings page first');
  }

  const redirectUri = browser.identity.getRedirectURL();

  const clientId = await registerClient(serverUrl, redirectUri);

  const codeVerifier = generateCodeVerifier();
  const codeChallenge = await generateCodeChallenge(codeVerifier);
  const state = generateState();

  const authorizeUrl = buildAuthorizeUrl(serverUrl, {
    clientId,
    redirectUri,
    codeChallenge,
    state,
  });

  const redirectedTo = await browser.identity.launchWebAuthFlow({
    url: authorizeUrl,
    interactive: true,
  });

  const { code, state: returnedState } = parseAuthorizeRedirect(redirectedTo);
  if (!code) {
    throw new Error('Authorization server did not return an authorization code');
  }
  if (returnedState !== state) {
    throw new Error('state mismatch — possible CSRF attack, login aborted');
  }

  const tokenResp = await exchangeCodeForToken(serverUrl, {
    code,
    codeVerifier,
    clientId,
    redirectUri,
  });

  await setLocalAuth({
    accessToken: tokenResp.access_token,
    refreshToken: null, // Rune Notes server does not currently issue refresh tokens.
    tokenExpiresAt: Date.now() + (tokenResp.expires_in ?? 3600) * 1000,
    clientId,
  }, serverUrl);
}

async function doLogout(targetServerUrl) {
  // Best-effort server-side revocation (RFC 7009 style /oauth/revoke) so a
  // "Logout" click actually invalidates the token immediately, rather than
  // leaving it valid server-side for its full remaining lifetime (up to 30
  // days) with only the local copy erased. Non-fatal if it fails (e.g. no
  // network, server down, or server predates this endpoint) — we still
  // clear local storage either way so the extension forgets the token.
  const serverUrl = targetServerUrl || (await getActiveServerUrl());
  const { accessToken } = await getLocalAuth(serverUrl);
  if (accessToken && serverUrl) {
    try {
      await apiFetch('/oauth/revoke', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: `token=${encodeURIComponent(accessToken)}`,
      }, serverUrl);
    } catch (e) {
      console.warn('[rune-notes] /oauth/revoke failed (continuing local logout):', e);
    }
  }

  // Also hit /auth/logout via launchWebAuthFlow to clear the server-side Web
  // session cookie (rune_sid), synchronizing the logout between extension
  // and Rune Notes Server so that the next login prompt properly asks for credentials.
  if (serverUrl && browser?.identity?.launchWebAuthFlow && browser?.identity?.getRedirectURL) {
    try {
      const redirectUri = browser.identity.getRedirectURL();
      const logoutUrl = `${serverUrl}/auth/logout?redirect_uri=${encodeURIComponent(redirectUri)}`;
      await browser.identity.launchWebAuthFlow({
        url: logoutUrl,
        interactive: false,
      });
    } catch (e) {
      console.warn('[rune-notes] /auth/logout webAuthFlow failed (continuing local logout):', e);
    }
  }

  await clearLocalAuth(serverUrl);
}

/** Message router: options.js / sidepanel.js talk to background via runtime messages. */
browser.runtime.onMessage.addListener((message, sender, sendResponse) => {
  (async () => {
    switch (message?.type) {
      case 'rune:login':
        try {
          await startLogin(message?.serverUrl);
          sendResponse({ ok: true });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      case 'rune:logout':
        await doLogout(message?.serverUrl);
        sendResponse({ ok: true });
        break;
      case 'rune:authStatus':
        sendResponse({ ok: true, loggedIn: await isLoggedIn(message?.serverUrl) });
        break;
      case 'rune:checkServer': {
        // Used by options.js to validate a URL before saving (GET /api/auth/config).
        try {
          const resp = await apiFetch('/api/auth/config', {}, message?.serverUrl);
          sendResponse({ ok: resp.ok, status: resp.status });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      }
      case 'rune:sendChat': {
        // Forwards a chat message to POST /api/chat. The AI's streamed reply
        // arrives separately over /api/events (SSE), which sidepanel.js
        // subscribes to directly (fetch()-based, since it needs to keep a
        // live stream open — not a fit for the request/response messaging
        // pattern used here).
        try {
          const resp = await apiFetch('/api/chat', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              note_id: message.noteId,
              content: message.content,
              nickname: message.nickname ?? null,
            }),
          });
          if (!resp.ok) {
            const text = await resp.text().catch(() => '');
            sendResponse({ ok: false, error: `HTTP ${resp.status} ${text}`.trim() });
            break;
          }
          const body = await resp.json().catch(() => ({}));
          sendResponse({ ok: body.ok !== false, error: body.error });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      }
      case 'rune:loadSession': {
        // PUT /api/session — returns { ok, history, files, current_model, … }
        // Used by the side panel to load chat history when first connecting
        // to a note (the SSE stream does not send a history event on initial
        // connect; only note-switch broadcasts do).
        try {
          const resp = await apiFetch('/api/session', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ note: message.noteId }),
          });
          if (!resp.ok) {
            sendResponse({ ok: false, error: `HTTP ${resp.status}` });
            break;
          }
          const body = await resp.json().catch(() => ({}));
          sendResponse({ ok: true, data: body });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      }
      case 'rune:patchNote': {
        try {
          const resp = await apiFetch(`/api/notes/${encodeURIComponent(message.noteId)}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(message.patch),
          });
          if (!resp.ok) {
            sendResponse({ ok: false, error: `HTTP ${resp.status}` });
            break;
          }
          const body = await resp.json().catch(() => ({}));
          sendResponse({ ok: true, data: body });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      }
      case 'rune:archiveChat': {
        try {
          const resp = await apiFetch('/api/chat/archive', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ note_id: message.noteId }),
          });
          if (!resp.ok) {
            sendResponse({ ok: false, error: `HTTP ${resp.status}` });
            break;
          }
          const body = await resp.json().catch(() => ({}));
          sendResponse({ ok: true, data: body });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      }
      default:
        sendResponse({ ok: false, error: `unknown message type: ${message?.type}` });
    }
  })();
  return true; // keep the message channel open for the async response
});
