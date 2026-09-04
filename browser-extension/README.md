# Rune Chat Browser Extension

Chat with your own self-hosted Rune Notes server about the page you're
currently browsing — right from a Chrome or Firefox side panel.

## Directory layout

```
browser-extension/
├─ manifest.chrome.json     Chrome (MV3) manifest
├─ manifest.firefox.json    Firefox (MV3 / WebExtensions) manifest
├─ build.js                 Assembles dist/chrome/ and dist/firefox/
├─ vendor/
│   └─ browser-polyfill.min.js   webextension-polyfill (gives Chrome the browser.* API)
├─ icons/                   16/48/128 px toolbar icon (ᚱ rune, shared by both browsers)
└─ src/
    ├─ common.js            Shared helpers: storage (sync + local), URL normalization,
    │                       authenticated fetch wrapper
    ├─ oauth-pkce.js         OAuth 2.1 Authorization Code + PKCE helpers (code_verifier /
    │                       code_challenge generation, dynamic client registration,
    │                       authorize URL, token exchange)
    ├─ background.js         Service worker (Chrome) / event page (Firefox): owns the
    │                       OAuth login/logout flow, token storage, context menu,
    │                       side-panel-click behavior, and message relay between the
    │                       content script, side panel, and the Rune Notes server
    ├─ content-script.js     Extracts the current page's title/URL/selected-or-full text
    │                       so the AI can answer questions about the page being viewed
    ├─ options.html/.js      Settings page: server URL entry, dynamic host permission
    │                       request, OAuth login/logout
    └─ sidepanel.html/.js/.css   Chat side panel UI (fetch()-based SSE client, message
                            rendering, page-context-aware prompt composition)
```

## Quick start (development)

### Chrome
1. `node build.js chrome` to produce `dist/chrome/`
2. Open `chrome://extensions`
3. Click "Load unpacked" and select `dist/chrome/`

### Firefox
1. `node build.js firefox` to produce `dist/firefox/`
2. Open `about:debugging#/runtime/this-firefox`
3. Click "Load Temporary Add-on…" and select `dist/firefox/manifest.json`

After loading, open the extension's settings page and enter your Rune Notes
server URL, then click "Save & Log In".

## Design notes

- The server URL has **no default** — the extension is unusable until the
  user enters one on the settings page. Only a single server is supported
  (no multi-profile switching).
- `storage.sync` holds the server URL and other cross-device settings;
  `storage.local` holds the OAuth access token (each device logs in
  independently — tokens are not synced).
- Host access uses `optional_host_permissions` plus a runtime
  `permissions.request()` call when the user saves a URL — never a blanket
  `<all_urls>` permission.
- Authentication is OAuth 2.1 Authorization Code + PKCE via
  `browser.identity.launchWebAuthFlow()`, using the server's dynamic client
  registration, authorize, and token endpoints. The current token response
  carries no refresh token, so an expired session requires logging in again.
- Chat streaming uses a hand-rolled `fetch()`-based SSE reader rather than
  `EventSource`, because `EventSource` cannot send a custom `Authorization`
  header and the SSE endpoint needs the bearer token when no cookie session
  is available.
- Users can switch between available notebooks directly from the side
  panel header dropdown; the active selection is remembered in local storage.
- All user-facing strings are English-only for now; internationalization is
  a deliberately deferred follow-up.
