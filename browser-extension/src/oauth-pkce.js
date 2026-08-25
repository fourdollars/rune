// oauth-pkce.js — RFC 7636 (PKCE) + RFC 7591 (Dynamic Client Registration)
// helpers, used by background.js to drive the login flow against a Rune
// Notes server's /oauth/register, /oauth/authorize, /oauth/token endpoints.

/** Base64url-encode an ArrayBuffer/Uint8Array without padding. */
function base64UrlEncode(bytes) {
  let str = '';
  for (const b of new Uint8Array(bytes)) str += String.fromCharCode(b);
  return btoa(str).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** RFC 7636 code_verifier: 43-128 char unreserved-charset random string. */
export function generateCodeVerifier() {
  const bytes = new Uint8Array(64);
  crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

/** RFC 7636 S256 code_challenge = BASE64URL(SHA256(code_verifier)). */
export async function generateCodeChallenge(codeVerifier) {
  const data = new TextEncoder().encode(codeVerifier);
  const digest = await crypto.subtle.digest('SHA-256', data);
  return base64UrlEncode(digest);
}

export function generateState() {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

/**
 * RFC 7591 Dynamic Client Registration against the Rune server.
 * Rune's /oauth/register accepts any registration (open-client model) and
 * always returns a fresh client_id when none is supplied.
 */
export async function registerClient(serverUrl, redirectUri) {
  const resp = await fetch(`${serverUrl}/oauth/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      client_name: 'Rune Notes Browser Extension',
      redirect_uris: [redirectUri],
      grant_types: ['authorization_code'],
      response_types: ['code'],
    }),
  });
  if (!resp.ok) {
    throw new Error(`/oauth/register failed: HTTP ${resp.status}`);
  }
  const body = await resp.json();
  if (!body.client_id) {
    throw new Error('/oauth/register response missing client_id');
  }
  return body.client_id;
}

/** Build the /oauth/authorize URL for launchWebAuthFlow. */
export function buildAuthorizeUrl(serverUrl, { clientId, redirectUri, codeChallenge, state }) {
  const url = new URL(`${serverUrl}/oauth/authorize`);
  url.searchParams.set('response_type', 'code');
  url.searchParams.set('client_id', clientId);
  url.searchParams.set('redirect_uri', redirectUri);
  url.searchParams.set('code_challenge', codeChallenge);
  url.searchParams.set('code_challenge_method', 'S256');
  url.searchParams.set('state', state);
  return url.toString();
}

/** Extract `code`/`state`/`error` from the redirect URL launchWebAuthFlow resolves with. */
export function parseAuthorizeRedirect(redirectUrl) {
  const url = new URL(redirectUrl);
  const error = url.searchParams.get('error');
  if (error) {
    const desc = url.searchParams.get('error_description') || error;
    throw new Error(`Authorization failed: ${desc}`);
  }
  return {
    code: url.searchParams.get('code'),
    state: url.searchParams.get('state'),
  };
}

/** Exchange an authorization code for tokens via POST /oauth/token (form-encoded). */
export async function exchangeCodeForToken(serverUrl, { code, codeVerifier, clientId, redirectUri }) {
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    code_verifier: codeVerifier,
    client_id: clientId,
    redirect_uri: redirectUri,
  });
  const resp = await fetch(`${serverUrl}/oauth/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: body.toString(),
  });
  const payload = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new Error(`/oauth/token failed: ${payload.error_description || payload.error || resp.status}`);
  }
  return payload; // { access_token, token_type, expires_in }
}
