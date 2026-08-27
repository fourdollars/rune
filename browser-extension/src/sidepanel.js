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
const $currentNoteName = document.getElementById('current-note-name');
const $noteSelect = document.getElementById('note-select');

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

// --- Configure marked (loaded by vendor-bundle.js before this module runs) ---
function escapeHtml(str) {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

if (typeof mermaid !== 'undefined') {
  try {
    mermaid.initialize({
      startOnLoad: false,
      theme: window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'default',
    });
  } catch (e) {
    console.error('[rune] mermaid.initialize failed:', e);
  }
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
    try {
      mermaid.initialize({ startOnLoad: false, theme: e.matches ? 'dark' : 'default' });
    } catch {}
  });
}

if (typeof marked !== 'undefined') {
  const renderer = new marked.Renderer();

  renderer.code = function(token) {
    const { text, lang } = token;
    if (lang && lang.toLowerCase() === 'mermaid') {
      const id = 'mermaid-' + Math.random().toString(36).slice(2);
      return `<div class="mermaid-block" id="${id}" data-src="${text.replace(/"/g, '&quot;')}"></div>`;
    }
    const raw = text.replace(/"/g, '&quot;');
    if (typeof hljs !== 'undefined') {
      const language = lang && hljs.getLanguage(lang) ? lang : null;
      const highlighted = language
        ? hljs.highlight(text, { language }).value
        : hljs.highlightAuto(text).value;
      const langClass = language ? ` class="language-${language}"` : '';
      return `<pre class="hljs-pre" data-raw="${raw}"><code class="hljs${langClass}">${highlighted}</code></pre>`;
    }
    const safe = escapeHtml(text);
    return `<pre class="hljs-pre" data-raw="${raw}"><code>${safe}</code></pre>`;
  };

  const hooks = {
    postprocess(html) {
      return html.replace(/<p>(\s*<svg[\s\S]*?<\/svg>\s*)<\/p>/gi, '$1');
    }
  };

  // --- Math extensions: intercept $$ and $ before marked mangles the content ---
  // Block math: $$...$$ (registered before inline to take priority)
  const blockMathExtension = {
    name: 'blockMath',
    level: 'block',
    start(src) { return src.indexOf('$$'); },
    tokenizer(src) {
      const match = src.match(/^\$\$([\s\S]+?)\$\$/);
      if (match) return { type: 'blockMath', raw: match[0], text: match[1].trim() };
    },
    renderer(token) {
      if (typeof katex !== 'undefined') {
        try {
          return '<div class="math-block">' + katex.renderToString(token.text, { displayMode: true, throwOnError: false }) + '</div>';
        } catch (e) {
          return '<div class="math-block math-error">' + escapeHtml(token.text) + '</div>';
        }
      }
      return '<div class="math-block">$$' + escapeHtml(token.text) + '$$</div>';
    }
  };

  // Inline math: $...$
  const inlineMathExtension = {
    name: 'inlineMath',
    level: 'inline',
    start(src) { return src.indexOf('$'); },
    tokenizer(src) {
      // Avoid matching $$ (already handled by block extension)
      const match = src.match(/^\$(?!\$)((?:[^$\\]|\\[\s\S])+?)\$/);
      if (match) return { type: 'inlineMath', raw: match[0], text: match[1] };
    },
    renderer(token) {
      if (typeof katex !== 'undefined') {
        try {
          return '<span class="math-inline">' + katex.renderToString(token.text, { displayMode: false, throwOnError: false }) + '</span>';
        } catch (e) {
          return '<span class="math-inline math-error">$' + escapeHtml(token.text) + '$</span>';
        }
      }
      return '<span class="math-inline">$' + escapeHtml(token.text) + '$</span>';
    }
  };

  marked.use({ renderer, hooks, breaks: true, gfm: true, extensions: [blockMathExtension, inlineMathExtension] });
}

// --- Markdown rendering helper ---
function markdownFragment(source) {
  if (typeof marked === 'undefined') return document.createTextNode(source);
  try {
    const html = marked.parse(source);
    const parser = new DOMParser();
    const doc = parser.parseFromString(html, 'text/html');
    const frag = document.createDocumentFragment();
    while (doc.body.firstChild) frag.appendChild(doc.body.firstChild);
    return frag;
  } catch (e) {
    console.error('[rune] markdownFragment error:', e);
    return document.createTextNode(source);
  }
}

function renderMermaidBlocks(container) {
  if (!container || typeof mermaid === 'undefined') return;
  container.querySelectorAll('.mermaid-block').forEach(el => {
    const src = el.dataset.src ? el.dataset.src.replace(/&quot;/g, '"') : '';
    if (!src) return;
    const doRender = (retries) => {
      if (window.mermaid && typeof window.mermaid.render === 'function') {
        const uid = 'mermaid-' + Date.now() + '-' + Math.random().toString(36).slice(2);
        el.id = uid;
        window.mermaid.render(uid + '-svg', src)
          .then(({ svg }) => { el.innerHTML = svg; })
          .catch(err => {
            const error = document.createElement('pre');
            error.style.color = 'var(--error, #cb2431)';
            error.textContent = 'Mermaid error: ' + err.message;
            el.replaceChildren(error);
          });
      } else if (retries > 0) {
        setTimeout(() => doRender(retries - 1), 200);
      } else {
        el.innerHTML = '<pre style="color:var(--text-muted)">Mermaid not loaded</pre>';
      }
    };
    doRender(20);
  });
}

let currentAssistantEl = null;
let currentAssistantText = '';
let sseAbortController = null;
// The note the SSE stream is currently subscribed to.
let activeNoteId = DEFAULT_NOTE_ID;
// Whether we've already replayed history for the current activeNoteId session.
let historyLoaded = false;

function fmtTime() {
  const d = new Date();
  const hh = String(d.getHours()).padStart(2, '0');
  const min = String(d.getMinutes()).padStart(2, '0');
  return `${hh}:${min}`;
}

/** Renders a chat bubble using the same DOM shape as the WebUI's addChatMessage(). */
function appendMessage(role, text = '', { senderLabel } = {}) {
  const div = document.createElement('div');
  div.className = `chat-msg ${role}`;

  if (role !== 'system') {
    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.textContent = senderLabel ?? (role === 'user' ? 'You' : 'ᚱ');
    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    timeSpan.textContent = fmtTime();
    sender.appendChild(nameSpan);
    sender.appendChild(timeSpan);
    div.appendChild(sender);
  }

  const body = document.createElement('div');
  body.className = 'body';
  if (role === 'system') {
    body.textContent = text;
  } else if (text) {
    if (typeof marked !== 'undefined') {
      body.replaceChildren(markdownFragment(text));
      renderMermaidBlocks(body);
    } else {
      body.textContent = text;
    }
  }
  div.appendChild(body);

  $messages.appendChild(div);
  $messages.scrollTop = $messages.scrollHeight;
  return body; // callers append/stream into the .body element
}

function appendSystem(text) {
  appendMessage('system', text);
}

/** Update the displayed note name in the header badge. */
function setActiveNote(noteId, noteName) {
  activeNoteId = noteId;
  const label = noteName || noteId;
  $currentNoteName.textContent = label;
  $currentNoteName.title = label;
  // Sync the select element to the active note
  if ($noteSelect.value !== noteId) $noteSelect.value = noteId;
}

/** Populate the note selector dropdown from a note_list payload. */
function populateNoteList(notes, activeId) {
  $noteSelect.innerHTML = '';
  for (const note of notes) {
    const opt = document.createElement('option');
    opt.value = note.id;
    opt.textContent = note.name || note.id;
    $noteSelect.appendChild(opt);
  }
  const active = notes.find((n) => n.id === activeId) ?? notes[0];
  if (active) {
    $noteSelect.value = active.id;
    setActiveNote(active.id, active.name || active.id);
  }
}

/**
 * Replay chat history messages. Called once on SSE connect when a `history`
 * event arrives. Mirrors the web app's replayHistory() in chat-history.js.
 */
function replayHistory(messages) {
  if (!messages?.length) return;
  $messages.innerHTML = '';
  for (const msg of messages) {
    const role = msg.role === 'assistant' ? 'assistant' : msg.role === 'user' ? 'user' : 'system';
    const label = msg.nickname || (role === 'assistant' ? 'ᚱ' : 'You');
    if (role === 'system') {
      appendSystem(msg.content || '');
      continue;
    }
    // Build bubble manually to set a historical timestamp
    const div = document.createElement('div');
    div.className = `chat-msg ${role}`;
    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.textContent = label;
    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    if (msg.created_at) {
      const d = new Date(msg.created_at * 1000);
      timeSpan.textContent = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
    } else {
      timeSpan.textContent = fmtTime();
    }
    sender.appendChild(nameSpan);
    sender.appendChild(timeSpan);
    div.appendChild(sender);
    const body = document.createElement('div');
    body.className = 'body';
    if (typeof marked !== 'undefined') {
      body.replaceChildren(markdownFragment(msg.content || ''));
      renderMermaidBlocks(body);
    } else {
      body.textContent = msg.content || '';
    }
    div.appendChild(body);
    $messages.appendChild(div);
  }
  $messages.scrollTop = $messages.scrollHeight;
}

function setConnected(connected) {
  $statusIndicator.classList.toggle('connected', connected);
  $statusIndicator.classList.toggle('disconnected', !connected);
  $statusIndicator.title = connected ? 'connected' : 'disconnected';
}

/**
 * Load chat history for the active note via PUT /api/session (background relay).
 * The SSE stream does not send a `history` event on initial connect — only when
 * switching notes server-side. So we call /api/session directly at startup and
 * after note switches to get prior messages.
 */
async function loadNoteHistory(noteId) {
  if (historyLoaded) return;
  try {
    const resp = await browser.runtime.sendMessage({
      type: 'rune:loadSession',
      noteId,
    });
    if (resp?.ok && resp.data?.history?.length) {
      historyLoaded = true;
      replayHistory(resp.data.history);
    } else {
      historyLoaded = true; // no history — mark loaded so we don't retry
    }
  } catch (e) {
    console.warn('[rune] loadNoteHistory failed:', e);
  }
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
  // Load prior conversation history for this note via /api/session (the SSE
  // stream only sends `history` on note-switch broadcasts, not on connect).
  loadNoteHistory(noteId);
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
        currentAssistantText = '';
      }
      currentAssistantText += payload.content ?? '';
      if (typeof marked !== 'undefined') {
        currentAssistantEl.replaceChildren(markdownFragment(currentAssistantText));
      } else {
        currentAssistantEl.textContent = currentAssistantText;
      }
      $messages.scrollTop = $messages.scrollHeight;
      break;
    case 'chat_done':
      if (currentAssistantEl && typeof marked !== 'undefined') {
        currentAssistantEl.replaceChildren(markdownFragment(currentAssistantText));
        renderMermaidBlocks(currentAssistantEl);
      }
      currentAssistantEl = null;
      currentAssistantText = '';
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
      currentAssistantText = '';
      break;
    case 'auth_error':
      appendSystem(`❌ Authentication failed: ${payload.message ?? 'unauthorized'}`);
      break;
    case 'note_list':
      // Populate the note switcher dropdown and update the header badge.
      if (Array.isArray(payload.notes)) {
        populateNoteList(payload.notes, payload.active);
      }
      break;
    case 'note_switched':
      // Server confirmed a note switch — reconnect SSE to the new note.
      if (payload.note_id && payload.note_id !== activeNoteId) {
        setActiveNote(payload.note_id, $noteSelect.options[$noteSelect.selectedIndex]?.text ?? payload.note_id);
        switchToNote(payload.note_id);
      }
      break;
    case 'history':
      // Replay prior conversation — only once per connection to avoid
      // duplicating messages on reconnect after the initial load.
      if (!historyLoaded && Array.isArray(payload.messages)) {
        historyLoaded = true;
        replayHistory(payload.messages);
      }
      break;
    default:
      // users_update / file_list / model_list / etc. — ignore for chat UI.
      break;

  }
}

async function startSseSubscription(noteId) {
  const { serverUrl } = await getSyncSettings();
  const { accessToken } = await getLocalAuth();
  if (!serverUrl) return;

  sseAbortController?.abort();
  sseAbortController = new AbortController();
  historyLoaded = false; // Reset so history is replayed for this connection
  activeNoteId = noteId ?? activeNoteId;

  try {
    await subscribeEvents({
      serverUrl,
      accessToken,
      noteId: activeNoteId,
      signal: sseAbortController.signal,
      onEvent: handleSseEvent,
    });
  } catch (e) {
    setConnected(false);
    if (e?.name === 'AbortError') return;
    appendSystem(`❌ SSE connection dropped: ${String(e?.message ?? e)}`);
  }
}

/**
 * Switch to a different note: clear the chat, abort the current SSE stream,
 * and open a new one for the target note.
 */
function switchToNote(noteId) {
  if (!noteId || noteId === activeNoteId) return;
  $messages.innerHTML = '';
  currentAssistantEl = null;
  currentAssistantText = '';
  startSseSubscription(noteId);
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
    noteId: activeNoteId,
    content: outgoing,
  });
  if (!resp?.ok) {
    appendSystem(`❌ Failed to send: ${resp?.error ?? 'unknown error'}`);
  }
});

// Note switcher: when the user changes the dropdown, switch SSE to that note.
$noteSelect.addEventListener('change', () => {
  const selectedId = $noteSelect.value;
  if (selectedId && selectedId !== activeNoteId) {
    switchToNote(selectedId);
  }
});

checkConfigured().then((configured) => {
  if (configured) startSseSubscription(DEFAULT_NOTE_ID);
});
