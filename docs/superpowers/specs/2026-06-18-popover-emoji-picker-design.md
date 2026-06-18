# Popover Emoji Picker Design Specification

Enable administrators to select a custom emoji icon for a note using an interactive popover emoji picker instead of a text input field in the Note Settings dialog.

## Goals
- Replace the text input field in the Note Settings dialog with a premium popover emoji picker button.
- Show the current selected emoji (or default folder) on the button itself.
- Toggling the button displays a grid popover containing 32 categorized emojis plus a "Default (Clear)" option.
- Highlight the active emoji in the popover grid.
- Handle click-outside events to close the popover.

## Proposed Changes

### 1. HTML Layout (`web/index.html`)
- Modify `#note-settings-modal` to replace the `#note-settings-icon` text input with the popover structure:
  ```html
  <div class="form-group emoji-picker-group">
      <label>Icon (Emoji)</label>
      <div class="emoji-picker-wrapper">
          <button type="button" id="emoji-picker-trigger" class="emoji-picker-trigger">📂</button>
          <div id="emoji-picker-popover" class="emoji-picker-popover hidden">
              <button type="button" class="emoji-clear-btn" id="emoji-clear-btn">Reset to Default</button>
              <div class="emoji-grid" id="emoji-picker-grid"></div>
          </div>
      </div>
  </div>
  ```

### 2. Styling (`web/style.css`)
- Style the picker trigger, popover, and grid:
  ```css
  .emoji-picker-wrapper {
      position: relative;
      display: inline-block;
  }
  .emoji-picker-trigger {
      font-size: 24px;
      padding: 8px 12px;
      border: 1px solid var(--border);
      background: var(--bg-secondary);
      border-radius: 6px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      transition: border-color 0.2s;
  }
  .emoji-picker-trigger:hover {
      border-color: var(--accent);
  }
  .emoji-picker-popover {
      position: absolute;
      top: calc(100% + 6px);
      left: 0;
      z-index: 1000;
      background: var(--bg-primary);
      border: 1px solid var(--border);
      border-radius: 6px;
      box-shadow: 0 4px 16px rgba(0,0,0,0.25);
      padding: 10px;
      width: 240px;
  }
  .emoji-clear-btn {
      width: 100%;
      padding: 6px;
      margin-bottom: 8px;
      border: 1px solid var(--border);
      background: var(--bg-secondary);
      border-radius: 4px;
      font-size: 13px;
      cursor: pointer;
  }
  .emoji-clear-btn:hover {
      background: var(--bg-hover);
      border-color: var(--accent);
  }
  .emoji-grid {
      display: grid;
      grid-template-columns: repeat(6, 1fr);
      gap: 6px;
  }
  .emoji-btn {
      font-size: 20px;
      padding: 4px;
      border: 1px solid transparent;
      background: none;
      border-radius: 4px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      transition: background 0.15s, border-color 0.15s;
  }
  .emoji-btn:hover {
      background: var(--bg-hover);
      border-color: var(--border);
  }
  .emoji-btn.active {
      border-color: var(--accent);
      background: var(--bg-hover);
  }
  ```

### 3. Frontend Logic (`web/app.js`)
- Maintain a list of 32 expanded emojis inside `web/app.js`:
  ```javascript
  const EMOJI_LIST = [
      '🚀', '💡', '🎨', '🔧', '📊', '⚙️', '🖥️', '💼', '🧠', '📌', '🏷️',
      '📝', '📚', '📓',
      '😀', '😎', '🤔', '🔥', '❤️', '🌟', '🎉', '👍', '👀',
      '🏠', '🌍', '🌳',
      '📅', '⏰', '💬', '🔔',
      '✅', '⚠️', '🔒'
  ];
  ```
- Keep track of the active selected emoji in a global state variable `selectedNoteIcon`.
- Initialize/render the emoji buttons inside the `#emoji-picker-grid` on page load.
- Setup toggle events for `#emoji-picker-trigger`:
  - Clicking it toggles the popover visibility.
  - Clicking outside the `.emoji-picker-wrapper` closes the popover.
- Setup click events for the emoji buttons:
  - Clicking an emoji updates `selectedNoteIcon` to that emoji, updates `#emoji-picker-trigger` text, and closes the popover.
  - Clicking `#emoji-clear-btn` resets `selectedNoteIcon` to `null` (default), updates trigger text to `📂`, and closes the popover.
- Update `showNoteSettings(sessionId)`:
  - Set `selectedNoteIcon` to `s.icon || null`.
  - Update `#emoji-picker-trigger` text to `selectedNoteIcon || '📂'`.
- Update `saveNoteSettings()`:
  - Send the value of `selectedNoteIcon` as `icon` in the payload of `note/rename` POST request.
