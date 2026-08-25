// background.js — service worker (Chrome) / event page (Firefox).
// Owns: OAuth PKCE flow, token storage, context menu registration,
// message relay between content-script <-> side panel <-> Rune server.

import {
  getSyncSettings,
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

// Mirrors sidepanel.js's detectDefaultNoteId() — each browser gets its own
// isolated chat note so concurrent Chrome/Firefox sessions on the same
// server don't collide in the same room. Kept in sync manually since the two
// files run in different contexts (service worker vs. side panel page) and
// don't share a module graph here.
function detectDefaultNoteId() {
  const ua = navigator.userAgent;
  if (ua.includes('Firefox/')) return 'Mozilla Firefox';
  if (ua.includes('Edg/')) return 'Microsoft Edge';
  if (ua.includes('Chrome/')) return 'Google Chrome';
  return 'Rune';
}

/**
 * Ensure this browser's default note exists on the server, creating it if
 * missing. Called right after a successful login so the user's note shows up
 * immediately (e.g. in the Rune WebUI's note list) instead of only coming
 * into existence lazily on first chat message via get_or_create_room().
 * Best-effort: a "duplicate name" response is expected and fine; any other
 * failure is swallowed (non-fatal — chat still works even if this fails,
 * since /api/chat auto-creates the room regardless).
 */
async function ensureDefaultNoteExists() {
  const noteId = detectDefaultNoteId();
  try {
    const resp = await apiFetch('/api/notes', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: noteId }),
    });
    await resp.json().catch(() => ({}));
  } catch (e) {
    console.warn('[rune-notes] ensureDefaultNoteExists failed (non-fatal):', e);
  }
  return noteId;
}

browser.runtime.onInstalled.addListener(() => {
  browser.contextMenus.create({
    id: CONTEXT_MENU_ID,
    title: 'Send to Rune Notes Chat',
    contexts: ['selection', 'page'],
  });
});

browser.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId !== CONTEXT_MENU_ID) return;
  // TODO: not yet wired up — should open the side panel (it currently only
  // opens via the toolbar-icon click behavior below) and forward
  // info.selectionText / a content-script extraction request into the chat
  // composer.
  console.log('[rune-notes] context menu clicked', info, tab);
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
async function startLogin() {
  const { serverUrl } = await getSyncSettings();
  if (!serverUrl) {
    throw new Error('Rune Notes Server URL is not set yet — please configure and authorize it on the settings page first');
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
    refreshToken: null, // Rune server does not currently issue refresh tokens.
    tokenExpiresAt: Date.now() + (tokenResp.expires_in ?? 3600) * 1000,
    clientId,
  });

  // Create this browser's default note right away so it's visible in the
  // Rune WebUI immediately, rather than waiting for the first chat message.
  await ensureDefaultNoteExists();
}

async function doLogout() {
  // Best-effort server-side revocation (RFC 7009 style /oauth/revoke) so a
  // "Logout" click actually invalidates the token immediately, rather than
  // leaving it valid server-side for its full remaining lifetime (up to 30
  // days) with only the local copy erased. Non-fatal if it fails (e.g. no
  // network, server down, or server predates this endpoint) — we still
  // clear local storage either way so the extension forgets the token.
  const { accessToken } = await getLocalAuth();
  if (accessToken) {
    try {
      await apiFetch('/oauth/revoke', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: `token=${encodeURIComponent(accessToken)}`,
      });
    } catch (e) {
      console.warn('[rune-notes] /oauth/revoke failed (continuing local logout):', e);
    }
  }
  await clearLocalAuth();
}

/** Message router: options.js / sidepanel.js talk to background via runtime messages. */
browser.runtime.onMessage.addListener((message, sender, sendResponse) => {
  (async () => {
    switch (message?.type) {
      case 'rune:login':
        try {
          await startLogin();
          sendResponse({ ok: true });
        } catch (e) {
          sendResponse({ ok: false, error: String(e?.message ?? e) });
        }
        break;
      case 'rune:logout':
        await doLogout();
        sendResponse({ ok: true });
        break;
      case 'rune:authStatus':
        sendResponse({ ok: true, loggedIn: await isLoggedIn() });
        break;
      case 'rune:checkServer': {
        // Used by options.js to validate a URL before saving (GET /api/auth/config).
        try {
          const resp = await apiFetch('/api/auth/config');
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
      default:
        sendResponse({ ok: false, error: `unknown message type: ${message?.type}` });
    }
  })();
  return true; // keep the message channel open for the async response
});
