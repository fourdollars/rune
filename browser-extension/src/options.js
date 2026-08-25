// options.js — settings page: server URL entry, permission request, login trigger.

import { getSyncSettings, setSyncSettings, normalizeServerUrl } from './common.js';

const $serverUrl = document.getElementById('serverUrl');
const $save = document.getElementById('save');
const $logout = document.getElementById('logout');
const $status = document.getElementById('status');

function setStatus(text) {
  $status.textContent = text;
}

async function refreshLoginButtonLabel() {
  const resp = await browser.runtime.sendMessage({ type: 'rune:authStatus' });
  if (resp?.loggedIn) {
    $save.textContent = 'Re-authorize & Log In Again';
    $logout.hidden = false;
    $logout.disabled = false;
  } else {
    $save.textContent = 'Save & Log In';
    $logout.hidden = true;
  }
}

async function restore() {
  const { serverUrl } = await getSyncSettings();
  if (serverUrl) {
    $serverUrl.value = serverUrl;
    setStatus(`Current setting: ${serverUrl}`);
    await refreshLoginButtonLabel();
  }
}

$save.addEventListener('click', async () => {
  let origin;
  try {
    origin = normalizeServerUrl($serverUrl.value.trim());
  } catch (e) {
    setStatus(`❌ ${e.message}`);
    return;
  }

  // Request host permission dynamically (optional_host_permissions), not <all_urls>.
  const granted = await browser.permissions.request({ origins: [`${origin}/*`] });
  if (!granted) {
    setStatus('❌ Permission denied — cannot connect to this URL');
    return;
  }

  await setSyncSettings({ serverUrl: origin });

  // Confirm this looks like a real Rune Notes server before declaring success.
  const check = await browser.runtime.sendMessage({ type: 'rune:checkServer' });
  if (!check?.ok) {
    setStatus(`⚠️ Saved, but could not verify the connection (${check?.error ?? check?.status}). Please double-check the URL.`);
    return;
  }

  // URL saved and verified — immediately continue into the OAuth login flow
  // rather than making the user click a second button.
  setStatus(`✅ Saved: ${origin}. Logging in… (a browser authorization window will open)`);
  const loginResp = await browser.runtime.sendMessage({ type: 'rune:login' });
  if (loginResp?.ok) {
    setStatus(`✅ Saved and logged in: ${origin}`);
  } else {
    setStatus(`⚠️ Saved: ${origin}, but login failed: ${loginResp?.error ?? 'unknown error'}`);
  }
  await refreshLoginButtonLabel();
});

$logout.addEventListener('click', async () => {
  setStatus('Logging out…');
  await browser.runtime.sendMessage({ type: 'rune:logout' });
  setStatus('Logged out');
  await refreshLoginButtonLabel();
});

restore();
