// sidepanel.js — chat UI. Subscribes to /api/events over a fetch()-based SSE
// reader (native EventSource can't set Authorization headers, so we can't
// use it here), sends user prompts via background -> POST /api/chat, and
// renders streamed AI replies as they arrive.
//
// Message markup/classes intentionally mirror Rune Notes' own WebUI chat
// panel (web/js/chat.js + web/style.css: .chat-msg / .sender / .body) so the
// extension's chat looks and feels consistent with the main app, styled via
// sidepanel.css (a ported subset of web/style.css).

import { getSyncSettings, getLocalAuth } from './common.js';

const $messages = document.getElementById('messages');
const $form = document.getElementById('composer');
const $input = document.getElementById('input');
const $notice = document.getElementById('disabled-notice');
const $statusIndicator = document.getElementById('status-indicator');

// Each browser gets its own isolated chat "room" (Rune note_id), so
// concurrent Chrome/Firefox sessions logged into the same server don't see
// each other's messages. We detect the running browser via its user agent
// and use that as a distinct default note. TODO: eventually let the user
// pick/rename the target notebook explicitly instead of relying on this
// browser-name heuristic.
function detectDefaultNoteId() {
  const ua = navigator.userAgent;
  if (ua.includes('Firefox/')) return 'Mozilla Firefox';
  if (ua.includes('Edg/')) return 'Microsoft Edge';
  if (ua.includes('Chrome/')) return 'Google Chrome';
  return 'Rune';
}
const DEFAULT_NOTE_ID = detectDefaultNoteId();

let currentAssistantEl = null;
let sseAbortController = null;

function fmtTime() {
  const d = new Date();
  const hh = String(d.getHours()).padStart(2, '0');
  const min = String(d.getMinutes()).padStart(2, '0');
  return `${hh}:${min}`;
}

/** Renders a chat bubble using the same DOM shape as the WebUI's addChatMessage(). */
function appendMessage(role, text, { senderLabel } = {}) {
  const div = document.createElement('div');
  div.className = `chat-msg ${role}`;

  if (role !== 'system') {
    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.textContent = senderLabel ?? (role === 'user' ? 'You' : 'Rune Notes');
    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    timeSpan.textContent = fmtTime();
    sender.appendChild(nameSpan);
    sender.appendChild(timeSpan);
    div.appendChild(sender);
  }

  const body = document.createElement('div');
  body.className = 'body';
  body.textContent = text;
  div.appendChild(body);

  $messages.appendChild(div);
  $messages.scrollTop = $messages.scrollHeight;
  return body; // callers append/stream into the .body element
}

function appendSystem(text) {
  appendMessage('system', text);
}

function setConnected(connected) {
  $statusIndicator.classList.toggle('connected', connected);
  $statusIndicator.classList.toggle('disconnected', !connected);
  $statusIndicator.title = connected ? 'connected' : 'disconnected';
}

async function checkConfigured() {
  const { serverUrl } = await getSyncSettings();
  const configured = Boolean(serverUrl);
  $notice.hidden = configured;
  $input.disabled = !configured;
  $form.querySelector('button').disabled = !configured;
  return configured;
}

async function getActiveTabPageContext() {
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  if (!tab) return null;
  try {
    return await browser.tabs.sendMessage(tab.id, { type: 'rune:getPageContext' });
  } catch (e) {
    // content script may not be injected on this page (e.g. chrome:// pages)
    return null;
  }
}

/**
 * Parse a single SSE "chunk" (one or more lines ending in \n\n) into
 * { event, data } records. Rune's server emits standard `event:`/`data:`
 * lines per the SSE spec (see src/serve/api.rs events_handler).
 */
function parseSseChunk(chunk) {
  const records = [];
  for (const block of chunk.split('\n\n')) {
    if (!block.trim()) continue;
    let event = 'message';
    const dataLines = [];
    for (const line of block.split('\n')) {
      if (line.startsWith('event:')) event = line.slice(6).trim();
      else if (line.startsWith('data:')) dataLines.push(line.slice(5).trim());
    }
    if (dataLines.length) records.push({ event, data: dataLines.join('\n') });
  }
  return records;
}

const BEARER_PREFIX = 'Bearer ';

/**
 * fetch()-based SSE client. We can't use EventSource because it does not
 * support custom headers, and we need an Authorization header carrying the
 * OAuth access token. Runs until the response body closes or `signal` aborts.
 */
async function subscribeEvents({ serverUrl, accessToken, noteId, signal, onEvent }) {
  const resp = await fetch(`${serverUrl}/api/events?note_id=${encodeURIComponent(noteId)}`, {
    headers: accessToken ? { Authorization: BEARER_PREFIX + accessToken } : {},
    signal,
  });
  if (!resp.ok || !resp.body) {
    throw new Error(`SSE connection failed (HTTP ${resp.status})`);
  }
  setConnected(true);
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    // SSE records are separated by a blank line; keep any trailing partial
    // record in `buffer` for the next read.
    const lastBreak = buffer.lastIndexOf('\n\n');
    if (lastBreak === -1) continue;
    const complete = buffer.slice(0, lastBreak);
    buffer = buffer.slice(lastBreak + 2);
    for (const rec of parseSseChunk(complete)) {
      onEvent(rec);
    }
  }
}

function handleSseEvent(rec) {
  let payload;
  try {
    payload = JSON.parse(rec.data);
  } catch (e) {
    return;
  }
  switch (rec.event) {
    case 'chat_token':
      if (!currentAssistantEl) {
        currentAssistantEl = appendMessage('assistant', '');
      }
      currentAssistantEl.textContent += payload.content ?? '';
      $messages.scrollTop = $messages.scrollHeight;
      break;
    case 'chat_done':
      currentAssistantEl = null;
      break;
    case 'chat_message':
      // Another participant's message (multi-user room); skip our own echo.
      break;
    case 'status':
      if (payload.state === 'thinking') appendSystem('(AI is thinking…)');
      break;
    case 'error':
      appendSystem(`❌ Error: ${payload.message ?? 'unknown error'}`);
      currentAssistantEl = null;
      break;
    case 'auth_error':
      appendSystem(`❌ Authentication failed: ${payload.message ?? 'unauthorized'}`);
      break;
    default:
      // history / users_update / file_list / etc. — ignore for v1 chat UI.
      break;
  }
}

async function startSseSubscription() {
  const { serverUrl } = await getSyncSettings();
  const { accessToken } = await getLocalAuth();
  if (!serverUrl) return;

  sseAbortController?.abort();
  sseAbortController = new AbortController();

  try {
    await subscribeEvents({
      serverUrl,
      accessToken,
      noteId: DEFAULT_NOTE_ID,
      signal: sseAbortController.signal,
      onEvent: handleSseEvent,
    });
  } catch (e) {
    setConnected(false);
    if (e?.name === 'AbortError') return;
    appendSystem(`❌ SSE connection dropped: ${String(e?.message ?? e)}`);
  }
}

$form.addEventListener('submit', async (e) => {
  e.preventDefault();
  const text = $input.value.trim();
  if (!text) return;
  $input.value = '';
  appendMessage('user', text);

  const pageContext = await getActiveTabPageContext();

  // Fold the current page's content into the outgoing message so the AI can
  // actually "see" what the user is looking at. v1 approach: prepend a
  // clearly-delimited context block ahead of the user's own text. If the
  // content script couldn't run on this page (e.g. chrome://, about:,
  // or a page opened before the extension had permission), pageContext is
  // null and we just send the raw text with a note so the AI (and user)
  // know why context is missing.
  let outgoing = text;
  if (pageContext) {
    const body = (pageContext.selection || pageContext.content || '').slice(0, 20000);
    outgoing =
      `Below is the content of the web page the user is currently viewing. ` +
      `Use it to answer the user's question.\n\n` +
      `Page title: ${pageContext.title}\n` +
      `URL: ${pageContext.url}\n` +
      (pageContext.selection ? `(User has selected part of the text)\n` : `(No selection, full page text extracted)\n`) +
      `---\n${body}\n---\n\nUser's question: ${text}`;
  } else {
    appendSystem('(Could not read this page — sending your text only; the AI cannot see the page content)');
  }

  const resp = await browser.runtime.sendMessage({
    type: 'rune:sendChat',
    noteId: DEFAULT_NOTE_ID,
    content: outgoing,
  });
  if (!resp?.ok) {
    appendSystem(`❌ Failed to send: ${resp?.error ?? 'unknown error'}`);
  }
});

checkConfigured().then((configured) => {
  if (configured) startSseSubscription();
});
