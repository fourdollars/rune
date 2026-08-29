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
const $runeTitle = document.getElementById('rune-title');
const $form = document.getElementById('composer');
const $input = document.getElementById('input');
const $notice = document.getElementById('disabled-notice');
const $statusIndicator = document.getElementById('status-indicator');
const $currentNoteName = document.getElementById('current-note-name');
const $noteSelect = document.getElementById('note-select');
const $modelIndicator = document.getElementById('model-indicator');
const $modelName = document.getElementById('model-name');
const $thinkingSelect = document.getElementById('thinking-select');
const $btnArchive = document.getElementById('btn-archive');
const $modelModal = document.getElementById('model-modal');
const $modelSearchInput = document.getElementById('model-search-input');
const $modelList = document.getElementById('model-list');
const $modelModalCancel = document.getElementById('model-modal-cancel');
const $archiveModal = document.getElementById('archive-modal');
const $archiveModalCancel = document.getElementById('archive-modal-cancel');
const $archiveModalConfirm = document.getElementById('archive-modal-confirm');

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

const COPY_ICON_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" width="13" height="13"><path d="M9.4 8.6h9a1.3 1.3 0 0 1 1.3 1.3v9a1.3 1.3 0 0 1-1.3 1.3h-9a1.3 1.3 0 0 1-1.3-1.3v-9A1.3 1.3 0 0 1 9.4 8.6Z"></path><path d="M5.2 15.4h-.6a1.3 1.3 0 0 1-1.3-1.3v-9a1.3 1.3 0 0 1 1.3-1.3h9a1.3 1.3 0 0 1 1.3 1.3v.6"></path></svg>';
const CHECK_ICON_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" width="13" height="13"><path d="m4.8 12.6 4.6 4.6L19.2 7.4"></path></svg>';

function decodeHtml(html) {
  const txt = document.createElement('textarea');
  txt.innerHTML = html;
  return txt.value;
}

function copyCodeBlock(button) {
  const pre = button.closest('pre');
  if (!pre) return;
  const raw = pre.dataset.raw !== undefined ? decodeHtml(pre.dataset.raw) : (pre.querySelector('code')?.textContent ?? '');
  const done = () => {
    button.innerHTML = CHECK_ICON_SVG;
    button.style.opacity = '1';
    setTimeout(() => {
      button.innerHTML = COPY_ICON_SVG;
      button.style.opacity = '';
    }, 1500);
  };
  if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(raw).then(done).catch(() => {
      const ta = document.createElement('textarea');
      ta.value = raw;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); done(); } catch (e) {}
      ta.remove();
    });
  } else {
    const ta = document.createElement('textarea');
    ta.value = raw;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand('copy'); done(); } catch (e) {}
    ta.remove();
  }
}

function attachCodeCopyButtons(container) {
  if (!container) return;
  container.querySelectorAll('pre.hljs-pre, pre').forEach((pre) => {
    if (pre.querySelector('.copy-btn')) return;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'copy-btn';
    btn.innerHTML = COPY_ICON_SVG;
    btn.title = 'Copy code';
    btn.setAttribute('aria-label', 'Copy code');
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      copyCodeBlock(btn);
    });
    pre.style.position = 'relative';
    pre.appendChild(btn);
  });
}

let currentAssistantDiv = null;
let currentAssistantEl = null;
let currentAssistantText = '';
let sseAbortController = null;
let availableModels = [];
let activeModel = '';
let currentThinking = 'off';
// The note the SSE stream is currently subscribed to.
let activeNoteId = DEFAULT_NOTE_ID;
// Whether we've already replayed history for the current activeNoteId session.
let historyLoaded = false;

function updateModelIndicator() {
  if (!$modelIndicator || !$modelName) return;
  if (!activeModel) {
    $modelIndicator.style.display = 'none';
    return;
  }
  $modelName.textContent = activeModel;
  $modelName.title = `Switch model (current: ${activeModel})`;
  $modelIndicator.style.display = 'flex';
}

function updateThinkingSelect() {
  if (!$thinkingSelect) return;
  if (!activeModel) {
    $thinkingSelect.style.display = 'none';
    return;
  }
  const currentModelObj = availableModels.find((m) => (m.id || m) === activeModel);
  const efforts = currentModelObj?.reasoning_efforts || [];
  if (efforts.length === 0) {
    $thinkingSelect.style.display = 'none';
    return;
  }
  const isGemini3 = activeModel.startsWith('gemini-3.');
  $thinkingSelect.innerHTML = '';
  if (!efforts.includes('none') && !isGemini3) {
    const offOpt = document.createElement('option');
    offOpt.value = 'off';
    offOpt.textContent = 'off';
    $thinkingSelect.appendChild(offOpt);
  }
  efforts.forEach((level) => {
    const opt = document.createElement('option');
    opt.value = level;
    opt.textContent = level;
    $thinkingSelect.appendChild(opt);
  });
  let val = currentThinking || 'off';
  if (isGemini3 && (val === 'off' || val === 'none')) {
    val = efforts[0] || 'medium';
  }
  $thinkingSelect.value = val;
  $thinkingSelect.style.display = '';
}

function formatContextWindow(tokens) {
  if (tokens >= 1000000) return (tokens / 1000000).toFixed(0) + 'M';
  if (tokens >= 1000) return (tokens / 1000).toFixed(0) + 'K';
  return String(tokens);
}

function renderModelList(filter = '') {
  if (!$modelList) return;
  $modelList.innerHTML = '';
  const lowerFilter = filter.toLowerCase();
  const filtered = availableModels.filter((m) => {
    const id = m.id || m;
    return !lowerFilter || id.toLowerCase().includes(lowerFilter);
  });
  if (filtered.length === 0) {
    const empty = document.createElement('div');
    empty.style.color = 'var(--text-muted)';
    empty.style.fontSize = '12px';
    empty.style.padding = '8px';
    empty.textContent = 'No matching models';
    $modelList.appendChild(empty);
    return;
  }
  filtered.forEach((m) => {
    const modelId = m.id || m;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'model-option' + (modelId === activeModel ? ' active' : '');
    const nameSpan = document.createElement('span');
    nameSpan.className = 'model-option-name';
    nameSpan.textContent = modelId;
    btn.appendChild(nameSpan);

    const badgeContainer = document.createElement('span');
    badgeContainer.className = 'model-badges';
    if (m.context_window) {
      const ctxBadge = document.createElement('span');
      ctxBadge.className = 'model-ctx-badge';
      ctxBadge.textContent = formatContextWindow(m.context_window);
      badgeContainer.appendChild(ctxBadge);
    }
    btn.appendChild(badgeContainer);

    btn.addEventListener('click', async () => {
      activeModel = modelId;
      updateModelIndicator();
      updateThinkingSelect();
      hideModelDialog();
      await browser.runtime.sendMessage({
        type: 'rune:patchNote',
        noteId: activeNoteId,
        patch: { model: modelId },
      });
    });
    $modelList.appendChild(btn);
  });
}

function showModelDialog() {
  if (!$modelModal) return;
  if (availableModels.length === 0) return;
  if ($modelSearchInput) $modelSearchInput.value = '';
  renderModelList('');
  $modelModal.classList.remove('hidden');
  if ($modelSearchInput) {
    setTimeout(() => $modelSearchInput.focus(), 50);
  }
}

function hideModelDialog() {
  if ($modelModal) $modelModal.classList.add('hidden');
}

function fmtTime(unixSec) {
  const d = unixSec ? new Date(unixSec * 1000) : new Date();
  const mm = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const min = String(d.getMinutes()).padStart(2, '0');
  return `${mm}-${dd} ${hh}:${min}`;
}

/** Renders a chat bubble using the same DOM shape as the WebUI's addChatMessage(). */
function appendMessage(role, text = '', { senderLabel, model, thinking, steps, tokensIn, tokensOut, toolCalls, createdAt } = {}) {
  const div = document.createElement('div');
  div.className = `chat-msg ${role}`;

  if (role !== 'system') {
    const sender = document.createElement('div');
    sender.className = 'sender';
    const nameSpan = document.createElement('span');
    nameSpan.textContent = senderLabel ?? (role === 'user' ? 'You' : 'ᚱ');
    sender.appendChild(nameSpan);

    if (role === 'assistant' && model) {
      const meta = document.createElement('span');
      meta.className = 'msg-meta';
      meta.textContent = (thinking && thinking !== 'off') ? `${model} ${thinking}` : model;
      sender.appendChild(meta);
    }

    const timeSpan = document.createElement('span');
    timeSpan.className = 'msg-time';
    timeSpan.textContent = fmtTime(createdAt || null);
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
      attachCodeCopyButtons(body);
    } else {
      body.textContent = text;
    }
  }

  // Run stats at message tail for assistant
  const totalTok = (tokensIn || 0) + (tokensOut || 0);
  if (role === 'assistant' && (steps || totalTok || toolCalls)) {
    const stats = document.createElement('div');
    stats.className = 'run-stats';
    stats.textContent = `${steps || 0} steps · ${totalTok} tokens · ${toolCalls || 0} tool calls`;
    body.appendChild(stats);
  }

  div.appendChild(body);

  $messages.appendChild(div);
  $messages.scrollTop = $messages.scrollHeight;
  return { div, body };
}

function appendSystem(text) {
  appendMessage('system', text);
}

function attachMetaToLastAssistant(model, tokIn, tokOut, ctxTokens, ctxWindow, steps, toolCalls, thinking) {
  const target = currentAssistantDiv || $messages.querySelector('.chat-msg.assistant:last-child');
  if (!target) return;
  const sender = target.querySelector('.sender');
  if (!sender) return;

  const oldMeta = sender.querySelector('.msg-meta');
  if (oldMeta) oldMeta.remove();

  if (model) {
    const meta = document.createElement('span');
    meta.className = 'msg-meta';
    meta.textContent = (thinking && thinking !== 'off') ? `${model} ${thinking}` : model;
    const timeEl = sender.querySelector('.msg-time');
    if (timeEl) sender.insertBefore(meta, timeEl);
    else sender.appendChild(meta);
  }

  const totalTok = (tokIn || 0) + (tokOut || 0);
  if (steps || totalTok || toolCalls) {
    const body = target.querySelector('.body');
    if (body) {
      const oldStats = body.querySelector('.run-stats');
      if (oldStats) oldStats.remove();
      const stats = document.createElement('div');
      stats.className = 'run-stats';
      stats.textContent = `${steps || 0} steps · ${totalTok} tokens · ${toolCalls || 0} tool calls`;
      body.appendChild(stats);
      $messages.scrollTop = $messages.scrollHeight;
    }
  }

  if (ctxWindow && ctxWindow > 0) {
    updateContextOverlay(ctxTokens || 0, ctxWindow);
  }
}

let lastContextTokens = null;

function updateContextOverlay(ctxTokens, ctxWindow) {
  lastContextTokens = ctxTokens;
  const overlay = document.getElementById('context-overlay');
  const pctEl = document.getElementById('context-pct');
  const cntEl = document.getElementById('context-counts');
  if (!overlay || !pctEl || !cntEl) return;
  const pct = Math.round((ctxTokens / ctxWindow) * 100);
  pctEl.textContent = pct + '% context used';
  const fmt = (n) => (n >= 1000 ? (n / 1000).toFixed(1) + 'k' : String(n));
  cntEl.textContent = fmt(ctxTokens) + ' / ' + fmt(ctxWindow);
  overlay.classList.remove('hidden', 'warn', 'danger');
  if (pct >= 80) overlay.classList.add('danger');
  else if (pct >= 60) overlay.classList.add('warn');
}

/** Update the displayed note name in the header badge. */
function setActiveNote(noteId, noteName) {
  activeNoteId = noteId;
  const label = noteName || noteId;
  $currentNoteName.textContent = label;
  $currentNoteName.title = label;
  // Sync the select element to the active note
  if ($noteSelect.value !== noteId) $noteSelect.value = noteId;
  browser.storage.local.set({ lastSelectedNote: noteId }).catch(() => {});
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
  const targetId = activeNoteId || activeId;
  const active = notes.find((n) => n.id === targetId) ?? notes.find((n) => n.id === activeId) ?? notes[0];
  if (active) {
    $noteSelect.value = active.id;
    setActiveNote(active.id, active.name || active.id);
  }
}

let currentLoadedHistoryRaw = null;

async function saveHistoryCache(noteId, messages) {
  if (!noteId || !Array.isArray(messages)) return;
  try {
    await browser.storage.local.set({ [`rune_cached_history_${noteId}`]: messages });
  } catch {}
}

async function restoreCachedState(noteId) {
  try {
    const keys = [
      `rune_cached_history_${noteId}`,
      'rune_cached_models',
      'rune_cached_notes',
      `rune_model_${noteId}`,
      `rune_thinking_${noteId}`,
    ];
    const cached = await browser.storage.local.get(keys);
    if (Array.isArray(cached.rune_cached_models) && cached.rune_cached_models.length) {
      availableModels = cached.rune_cached_models;
    }
    if (cached[`rune_model_${noteId}`]) {
      activeModel = cached[`rune_model_${noteId}`];
    }
    if (cached[`rune_thinking_${noteId}`]) {
      currentThinking = cached[`rune_thinking_${noteId}`];
    }
    if (Array.isArray(cached.rune_cached_notes) && cached.rune_cached_notes.length) {
      populateNoteList(cached.rune_cached_notes, noteId);
    }
    updateModelIndicator();
    updateThinkingSelect();

    const history = cached[`rune_cached_history_${noteId}`];
    if (Array.isArray(history) && history.length) {
      currentLoadedHistoryRaw = JSON.stringify(history);
      replayHistory(history, false);
    }
  } catch (e) {
    console.warn('[rune] restoreCachedState failed:', e);
  }
}

/**
 * Replay chat history messages. Called on SSE connect when a `history`
 * event arrives or after local cache restoration.
 */
function replayHistory(messages, saveToCache = true) {
  if (!messages?.length) return;
  const raw = JSON.stringify(messages);
  if (raw === currentLoadedHistoryRaw && $messages.children.length > 0) {
    return; // Already rendered identical history — skip re-rendering
  }
  currentLoadedHistoryRaw = raw;
  $messages.innerHTML = '';
  for (const msg of messages) {
    const role = msg.role === 'assistant' ? 'assistant' : msg.role === 'user' ? 'user' : 'system';
    const label = msg.nickname || (role === 'assistant' ? 'ᚱ' : 'You');
    if (role === 'system') {
      appendSystem(msg.content || '');
      continue;
    }
    appendMessage(role, msg.content || '', {
      senderLabel: label,
      model: msg.model,
      thinking: msg.thinking,
      steps: msg.steps,
      tokensIn: msg.tokens_in,
      tokensOut: msg.tokens_out,
      toolCalls: msg.tool_calls,
      createdAt: msg.created_at,
    });
  }
  $messages.scrollTop = $messages.scrollHeight;

  // Restore context overlay from last assistant message with context_tokens
  const lastWithCtx = [...messages].reverse().find(
    (m) => m.role === 'assistant' && m.context_tokens != null && m.model
  );
  if (lastWithCtx) {
    const modelEntry = availableModels.find((m) => m.id === lastWithCtx.model);
    const ctxWindow = modelEntry?.context_window || lastWithCtx.context_window;
    if (ctxWindow) {
      updateContextOverlay(lastWithCtx.context_tokens, ctxWindow);
    }
  }

  if (saveToCache && activeNoteId) {
    saveHistoryCache(activeNoteId, messages);
  }
}

const STATUS_ICONS = {
  idle: '<svg viewBox="0 0 24 24" fill="currentColor" width="10" height="10" class="rune-icon"><circle cx="12" cy="12" r="5.2"/></svg>',
  typing: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="11" height="11" class="rune-icon"><path d="M12 20h8.5"/><path d="M16.4 3.6a2.1 2.1 0 0 1 3 3L7.6 18.4 3.3 19.7l1.3-4.3Z"/></svg>',
  thinking: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="11" height="11" class="rune-icon"><path d="M17.5 3.2a3 3 0 0 0-3 3v11.6a3 3 0 1 0 3-3H6.5a3 3 0 1 0 3 3V6.2a3 3 0 1 0-3 3h11a3 3 0 0 0 0-6Z"/></svg>',
  tool: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="11" height="11" class="rune-icon"><path d="M12 15.4a3.4 3.4 0 1 0 0-6.8 3.4 3.4 0 0 0 0 6.8Z"/><path d="M19.3 14.6a1.5 1.5 0 0 0 .3 1.7l.1.1a1.9 1.9 0 1 1-2.7 2.7l-.1-.1a1.5 1.5 0 0 0-2.5 1.1v.2a1.9 1.9 0 1 1-3.8 0v-.1a1.5 1.5 0 0 0-2.5-1.1l-.1.1a1.9 1.9 0 1 1-2.7-2.7l.1-.1a1.5 1.5 0 0 0-1.1-2.5h-.2a1.9 1.9 0 1 1 0-3.8h.1a1.5 1.5 0 0 0 1.1-2.5l-.1-.1a1.9 1.9 0 1 1 2.7-2.7l.1.1a1.5 1.5 0 0 0 2.5-1.1v-.2a1.9 1.9 0 1 1 3.8 0v.1a1.5 1.5 0 0 0 2.5 1.1l.1-.1a1.9 1.9 0 1 1 2.7 2.7l-.1.1a1.5 1.5 0 0 0 1.1 2.5h.2a1.9 1.9 0 1 1 0 3.8h-.1a1.5 1.5 0 0 0-1.4.9Z"/></svg>',
  disconnected: '<svg viewBox="0 0 24 24" fill="currentColor" width="10" height="10" class="rune-icon"><circle cx="12" cy="12" r="5.2"/></svg>',
};

let currentStatus = 'disconnected';
const MIN_TOOL_DISPLAY_MS = 600;
let toolStartTime = 0;
let clearToolTimer = null;

function paintStatus(state, label) {
  if (!$statusIndicator) return;
  $statusIndicator.className = `status ${state}`;
  $statusIndicator.title = label || state;
  $statusIndicator.setAttribute('aria-label', `Status: ${label || state}`);
  $statusIndicator.innerHTML = STATUS_ICONS[state] || STATUS_ICONS.idle;
}

function setStatus(state) {
  if (state && typeof state === 'string' && state.startsWith('tool:')) {
    setToolStatus(state.slice(5));
    return;
  }
  if (clearToolTimer) {
    clearTimeout(clearToolTimer);
    clearToolTimer = null;
  }
  currentStatus = state;
  paintStatus(state, state);
}

function setToolStatus(toolName) {
  if (clearToolTimer) {
    clearTimeout(clearToolTimer);
    clearToolTimer = null;
  }
  currentStatus = 'tool';
  toolStartTime = Date.now();
  paintStatus('tool', `tool: ${toolName}`);
}

function clearToolStatus() {
  if (currentStatus !== 'tool') {
    setStatus('thinking');
    return;
  }
  const elapsed = Date.now() - toolStartTime;
  if (elapsed < MIN_TOOL_DISPLAY_MS) {
    if (clearToolTimer) clearTimeout(clearToolTimer);
    clearToolTimer = setTimeout(() => {
      clearToolTimer = null;
      if (currentStatus === 'tool') {
        setStatus('thinking');
      }
    }, MIN_TOOL_DISPLAY_MS - elapsed);
  } else {
    setStatus('thinking');
  }
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
    if (resp?.ok && resp.data) {
      if (resp.data.current_model) {
        activeModel = resp.data.current_model;
        updateModelIndicator();
        updateThinkingSelect();
        browser.storage.local.set({ [`rune_model_${noteId}`]: activeModel }).catch(() => {});
      }
      if (resp.data.history?.length) {
        historyLoaded = true;
        replayHistory(resp.data.history);
      } else {
        historyLoaded = true; // no history — mark loaded so we don't retry
      }
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
  if (!tab || !tab.id || !tab.url) return null;

  // Skip browser internal / restricted pages where extensions cannot inspect content
  if (
    tab.url.startsWith('chrome://') ||
    tab.url.startsWith('chrome-extension://') ||
    tab.url.startsWith('about:') ||
    tab.url.startsWith('edge://') ||
    tab.url.startsWith('view-source:')
  ) {
    return null;
  }

  // 1. Try sendMessage to already-injected content script
  try {
    const res = await browser.tabs.sendMessage(tab.id, { type: 'rune:getPageContext' });
    if (res && (res.title || res.content)) return res;
  } catch (_) {
    // Content script not loaded / not responding (e.g. tab opened before extension load)
  }

  // 2. Fallback: dynamically execute extractor via scripting API
  if (browser.scripting?.executeScript) {
    try {
      const results = await browser.scripting.executeScript({
        target: { tabId: tab.id },
        func: () => {
          const sel = window.getSelection();
          const selection = sel ? sel.toString().trim() : '';
          const bodyText = document.body ? document.body.innerText.slice(0, 20000) : '';
          return {
            url: location.href,
            title: document.title,
            selection,
            content: selection || bodyText,
          };
        },
      });
      if (results && results[0] && results[0].result) {
        return results[0].result;
      }
    } catch (e) {
      console.warn('[rune-chat] getActiveTabPageContext fallback failed:', e);
    }
  }

  return null;
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
  setStatus('idle');
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
  let payload = {};
  try {
    payload = JSON.parse(rec.data);
  } catch (e) {
    return;
  }
  switch (rec.event) {
    case 'chat_token':
      if (currentStatus !== 'typing') {
        setStatus('typing');
      }
      if (!currentAssistantEl) {
        const { div, body } = appendMessage('assistant', '');
        currentAssistantDiv = div;
        currentAssistantEl = body;
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
    case 'chat_meta':
      attachMetaToLastAssistant(
        payload.model,
        payload.tokens_in,
        payload.tokens_out,
        payload.context_tokens,
        payload.context_window,
        payload.steps,
        payload.tool_calls,
        payload.thinking
      );
      break;
    case 'chat_done':
      if (currentAssistantEl && typeof marked !== 'undefined') {
        currentAssistantEl.replaceChildren(markdownFragment(currentAssistantText));
        renderMermaidBlocks(currentAssistantEl);
        attachCodeCopyButtons(currentAssistantEl);
      }
      currentAssistantEl = null;
      currentAssistantText = '';
      currentAssistantDiv = null;
      setStatus('idle');
      break;
    case 'chat_message':
      // Another participant's message (multi-user room); skip our own echo.
      break;
    case 'status':
      setStatus(payload.state);
      break;
    case 'tool_status':
      if (payload.state === 'start') {
        setToolStatus(payload.tool);
      } else {
        clearToolStatus();
      }
      break;
    case 'error':
      appendSystem(`❌ Error: ${payload.message ?? 'unknown error'}`);
      currentAssistantEl = null;
      currentAssistantText = '';
      currentAssistantDiv = null;
      clearToolStatus();
      setStatus('idle');
      break;
    case 'auth_error':
      appendSystem(`❌ Authentication failed: ${payload.message ?? 'unauthorized'}`);
      setStatus('disconnected');
      break;
    case 'model_list':
      if (Array.isArray(payload.models)) {
        availableModels = payload.models;
        browser.storage.local.set({ rune_cached_models: availableModels }).catch(() => {});
      }
      if (payload.active) {
        activeModel = payload.active;
        browser.storage.local.set({ [`rune_model_${activeNoteId}`]: activeModel }).catch(() => {});
      }
      if (payload.thinking) {
        currentThinking = payload.thinking;
        browser.storage.local.set({ [`rune_thinking_${activeNoteId}`]: currentThinking }).catch(() => {});
      }
      updateModelIndicator();
      updateThinkingSelect();
      break;
    case 'model_changed':
      if (payload.model) {
        activeModel = payload.model;
        browser.storage.local.set({ [`rune_model_${activeNoteId}`]: activeModel }).catch(() => {});
      }
      if (payload.thinking) {
        currentThinking = payload.thinking;
        browser.storage.local.set({ [`rune_thinking_${activeNoteId}`]: currentThinking }).catch(() => {});
      }
      updateModelIndicator();
      updateThinkingSelect();
      appendSystem(`Model switched to: ${activeModel} ${currentThinking}`);
      if (lastContextTokens !== null) {
        const newModel = availableModels.find((m) => (m.id || m) === activeModel);
        if (newModel && newModel.context_window) {
          updateContextOverlay(lastContextTokens, newModel.context_window);
        }
      }
      break;
    case 'thinking_changed':
      if (payload.thinking) {
        currentThinking = payload.thinking;
        browser.storage.local.set({ [`rune_thinking_${activeNoteId}`]: currentThinking }).catch(() => {});
      }
      updateThinkingSelect();
      appendSystem(`Model switched to: ${activeModel} ${currentThinking}`);
      break;
    case 'archive_done':
      $messages.innerHTML = '';
      currentLoadedHistoryRaw = null;
      browser.storage.local.remove(`rune_cached_history_${activeNoteId}`).catch(() => {});
      appendSystem('Chat archived.');
      {
        const overlay = document.getElementById('context-overlay');
        if (overlay) overlay.classList.add('hidden');
      }
      break;
    case 'note_list':
      // Populate the note switcher dropdown and update the header badge.
      if (Array.isArray(payload.notes)) {
        browser.storage.local.set({ rune_cached_notes: payload.notes }).catch(() => {});
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
      // users_update / file_list / etc. — ignore for chat UI.
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
  browser.storage.local.set({ lastSelectedNote: activeNoteId }).catch(() => {});

  try {
    await subscribeEvents({
      serverUrl,
      accessToken,
      noteId: activeNoteId,
      signal: sseAbortController.signal,
      onEvent: handleSseEvent,
    });
  } catch (e) {
    setStatus('disconnected');
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
  currentAssistantDiv = null;
  currentLoadedHistoryRaw = null;
  historyLoaded = false;
  const overlay = document.getElementById('context-overlay');
  if (overlay) overlay.classList.add('hidden');
  browser.storage.local.set({ lastSelectedNote: noteId }).catch(() => {});
  restoreCachedState(noteId);
  startSseSubscription(noteId);
}

$input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    $form.dispatchEvent(new Event('submit', { cancelable: true }));
  }
});

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

// Title click listener: switch to existing Rune server tab or open a new one (or options page if not configured yet)
$runeTitle?.addEventListener('click', async (e) => {
  e.preventDefault();
  const { serverUrl } = await getSyncSettings();
  if (!serverUrl) {
    if (browser.runtime.openOptionsPage) {
      browser.runtime.openOptionsPage();
    }
    return;
  }

  try {
    const normalizedUrl = serverUrl.replace(/\/+$/, '');
    const tabs = await browser.tabs.query({});
    const matchingTabs = tabs.filter((t) => {
      if (!t.url) return false;
      const tUrl = t.url.replace(/\/+$/, '');
      return tUrl === normalizedUrl || tUrl.startsWith(normalizedUrl + '/') || tUrl.startsWith(normalizedUrl + '?');
    });

    if (matchingTabs.length > 0) {
      const currentWindow = await browser.windows?.getCurrent?.().catch(() => null);
      const targetTab = (currentWindow && matchingTabs.find((t) => t.windowId === currentWindow.id)) || matchingTabs[0];
      if (targetTab?.id !== undefined) {
        await browser.tabs.update(targetTab.id, { active: true });
        if (targetTab.windowId !== undefined && browser.windows?.update) {
          await browser.windows.update(targetTab.windowId, { focused: true });
        }
        return;
      }
    }

    await browser.tabs.create({ url: serverUrl });
  } catch (err) {
    console.error('[rune] failed to switch/open tab:', err);
    browser.tabs.create({ url: serverUrl }).catch(() => {});
  }
});

// Model indicator & thinking listeners
$modelName?.addEventListener('click', showModelDialog);
$modelSearchInput?.addEventListener('input', (e) => {
  renderModelList(e.target.value.trim());
});
$thinkingSelect?.addEventListener('change', async (e) => {
  currentThinking = e.target.value;
  await browser.runtime.sendMessage({
    type: 'rune:patchNote',
    noteId: activeNoteId,
    patch: { thinking: currentThinking },
  });
});
$modelModalCancel?.addEventListener('click', hideModelDialog);

// Archive dialog listeners
$btnArchive?.addEventListener('click', () => {
  $archiveModal?.classList.remove('hidden');
});
$archiveModalCancel?.addEventListener('click', () => {
  $archiveModal?.classList.add('hidden');
});
$archiveModalConfirm?.addEventListener('click', async () => {
  $archiveModal?.classList.add('hidden');
  const resp = await browser.runtime.sendMessage({
    type: 'rune:archiveChat',
    noteId: activeNoteId,
  });
  if (resp?.ok) {
    $messages.innerHTML = '';
    currentLoadedHistoryRaw = null;
    browser.storage.local.remove(`rune_cached_history_${activeNoteId}`).catch(() => {});
    appendSystem('Chat archived.');
    const overlay = document.getElementById('context-overlay');
    if (overlay) overlay.classList.add('hidden');
  } else {
    appendSystem(`❌ Failed to archive: ${resp?.error ?? 'unknown error'}`);
  }
});

function initChatInputResize(resizerEl, inputEl, storageKey = 'rune_chat_input_height') {
  if (!resizerEl || !inputEl) return;

  browser.storage.local.get(storageKey).then((data) => {
    const saved = data[storageKey];
    if (saved) {
      const h = parseInt(saved, 10);
      if (h >= 56 && h <= window.innerHeight * 0.6) {
        inputEl.style.height = h + 'px';
      }
    }
  }).catch(() => {});

  resizerEl.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    try { resizerEl.setPointerCapture(e.pointerId); } catch (_) {}
    resizerEl.classList.add('dragging');
    document.body.style.userSelect = 'none';
    document.body.style.cursor = 'row-resize';

    const startY = e.clientY;
    const startH = inputEl.offsetHeight;
    const minH = 56;
    let currentH = startH;

    function onPointerMove(ev) {
      const maxH = Math.min(400, Math.floor(window.innerHeight * 0.6));
      const delta = startY - ev.clientY;
      currentH = Math.max(minH, Math.min(maxH, startH + delta));
      inputEl.style.height = currentH + 'px';
    }

    function onPointerUp(ev) {
      try { resizerEl.releasePointerCapture(ev.pointerId); } catch (_) {}
      resizerEl.classList.remove('dragging');
      document.body.style.userSelect = '';
      document.body.style.cursor = '';
      resizerEl.removeEventListener('pointermove', onPointerMove);
      resizerEl.removeEventListener('pointerup', onPointerUp);
      resizerEl.removeEventListener('pointercancel', onPointerUp);
      browser.storage.local.set({ [storageKey]: currentH }).catch(() => {});
    }

    resizerEl.addEventListener('pointermove', onPointerMove);
    resizerEl.addEventListener('pointerup', onPointerUp);
    resizerEl.addEventListener('pointercancel', onPointerUp);
  });
}

initChatInputResize(document.getElementById('chat-input-resizer'), $input);

setStatus('disconnected');

checkConfigured().then(async (configured) => {
  if (configured) {
    const { lastSelectedNote } = await browser.storage.local.get('lastSelectedNote');
    const initialNoteId = lastSelectedNote || DEFAULT_NOTE_ID;
    activeNoteId = initialNoteId;
    await restoreCachedState(initialNoteId);
    startSseSubscription(initialNoteId);
  }
});
