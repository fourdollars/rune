// content-script.js — runs in the context of the visited page.
// Extracts either the user's current selection or a naive full-body text
// dump (see TODO below re: Readability), then hands it to the side panel /
// background on request.

/* global browser */

function getSelectionText() {
  const sel = window.getSelection();
  return sel ? sel.toString().trim() : '';
}

/**
 * TODO: integrate Readability.js (bundle a vendored copy) to produce a
 * clean markdown-ish extraction of the main article body when there is no
 * selection. For now this is a naive fallback.
 */
function getPageFallbackText() {
  return document.body ? document.body.innerText.slice(0, 20000) : '';
}

function getPageContext() {
  const selection = getSelectionText();
  return {
    url: location.href,
    title: document.title,
    selection,
    content: selection || getPageFallbackText(),
  };
}

browser.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === 'rune:getPageContext') {
    sendResponse(getPageContext());
    return true;
  }
  return false;
});
