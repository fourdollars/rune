# Popover Emoji Picker Design Specification

Enable administrators to select a custom emoji icon for a note using a comprehensive, scrollable, categorized popover emoji picker with category tabs and a search/filter input.

## Goals
- Replace the text input field in the Note Settings dialog with a premium popover emoji picker button.
- Toggle an overlay panel displaying a full "emoji wall" categorized into:
  - Smileys & Emotion (`😀`)
  - Animals & Nature (`🐶`)
  - Food & Drink (`🍔`)
  - Activities & Sports (`⚽`)
  - Travel & Places (`🚗`)
  - Objects & Tools (`💡`)
  - Flags (`🚩`)
- Support top category tabs for quick scrolling/anchoring.
- Support real-time search/filtering.
- Support a distinct "Reset to Default" button.
- Dismiss popover on selecting an emoji or clicking outside.

## Proposed Changes

### 1. HTML Layout (`web/index.html`)
Update `#note-settings-modal`'s form-group for "Icon" to host the emoji picker:
```html
            <div class="form-group emoji-picker-group">
                <label>Icon (Emoji)</label>
                <div class="emoji-picker-wrapper">
                    <button type="button" id="emoji-picker-trigger" class="emoji-picker-trigger">📂</button>
                    <div id="emoji-picker-popover" class="emoji-picker-popover hidden">
                        <input type="text" id="emoji-search-input" class="emoji-search-input" placeholder="Search emoji..." autocomplete="off" />
                        <div class="emoji-tabs" id="emoji-picker-tabs"></div>
                        <div class="emoji-scroll-area" id="emoji-picker-scroll-area">
                            <button type="button" class="emoji-clear-btn" id="emoji-clear-btn">Reset to Default</button>
                            <div id="emoji-categories-container"></div>
                        </div>
                    </div>
                </div>
            </div>
```

### 2. Styling (`web/style.css`)
Style the popover to resemble a native OS picker:
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
    width: 280px;
}
.emoji-search-input {
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-primary);
    border-radius: 4px;
    font-size: 13px;
    margin-bottom: 8px;
    box-sizing: border-box;
}
.emoji-tabs {
    display: flex;
    justify-content: space-between;
    border-bottom: 1px solid var(--border);
    padding-bottom: 6px;
    margin-bottom: 8px;
}
.emoji-tab-btn {
    background: none;
    border: none;
    font-size: 16px;
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    transition: background 0.15s;
}
.emoji-tab-btn:hover {
    background: var(--bg-hover);
}
.emoji-scroll-area {
    max-height: 220px;
    overflow-y: auto;
    padding-right: 4px;
}
.emoji-clear-btn {
    width: 100%;
    padding: 6px;
    margin-bottom: 10px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text-primary);
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
}
.emoji-clear-btn:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
}
.emoji-category-section {
    margin-bottom: 12px;
}
.emoji-category-title {
    font-size: 11px;
    font-weight: bold;
    color: var(--text-secondary);
    margin-bottom: 6px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}
.emoji-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
}
.emoji-btn {
    font-size: 18px;
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
- Store the comprehensive, categorized emojis list inside `web/app.js`.
- Render tabs dynamically. Clicking a tab scrolls the `.emoji-scroll-area` to the corresponding category header.
- Implement search input keyup handler:
  - If search query is empty: show all categories and all emojis.
  - If search query is non-empty: perform string contains matching on emoji keyword tags. Show matching emojis in a single "Search Results" view inside the scroll area (hiding categories).
- Hook up popover show/hide events:
  - Toggling picker button shows/hides popover.
  - Clicking outside wrapper closes popover.
- Update `showNoteSettings()` and `saveNoteSettings()` to get/set the active icon state (`selectedNoteIcon`).
