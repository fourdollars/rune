# Hide Context Overlay Design Specification

Only show the "context used" overlay in the Rune Notes chat panel when the message input box is empty.

## Goals
- Hide the `#context-overlay` element when the user starts typing or has entered any text in the `#chat-input` textarea.
- Re-display the `#context-overlay` element if the `#chat-input` textarea becomes empty (e.g. manually cleared by the user or programmatically cleared after sending a message).

## Proposed Changes

### 1. Frontend Styling (`web/style.css`)
- Introduce a CSS rule targeting the `#context-overlay` sibling element when `#chat-input` matches `:not(:placeholder-shown)`:
  ```css
  /* Hide context overlay when chat input contains text */
  #chat-input:not(:placeholder-shown) ~ #context-overlay {
      display: none !important;
  }
  ```

## Verification Plan
1. Launch the `rune` notes server.
2. Open the note workspace in the browser.
3. Verify that the context overlay is visible when the input area is empty (assuming some context is active/loaded).
4. Type some text into the `#chat-input` field. Verify that the context overlay is immediately hidden.
5. Backspace/delete the text until the field is empty again. Verify that the context overlay immediately reappears.
6. Type some text, press `Enter` to send, and verify that the context overlay reappears immediately after the input is cleared on send.
