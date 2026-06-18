# Custom Note Icon Design Specification

Enable administrators to customize the note emoji icon in each note's settings dialog in Rune Notes.

## Goals
- Allow custom emoji icons (e.g. `🚀`, `🎨`, `💡`) for notes/folders instead of default open/closed/locked folders.
- Persist custom icon configuration to the SQLite sessions database table.
- Display the customized icon in both desktop and mobile sidebar note lists.
- Dim custom icons when private using the `0.45` opacity styling to match visibility indicators.

## Proposed Changes

### 1. Backend SQLite database schema & operations (`src/serve/db.rs`)
- Alter the `sessions` table to add an `icon TEXT` column (run in both schema migrations).
- Update the `NoteRecord` struct:
  ```rust
  pub struct NoteRecord {
      ...
      #[serde(skip_serializing_if = "Option::is_none")]
      pub icon: Option<String>,
  }
  ```
- Update query mappings (`list_notes`, `get_session`) to read and map the `icon` field.
- Update `rename_note` to take `icon: Option<&str>` and update the `sessions` table.

### 2. API requests and handlers (`src/serve/api.rs`)
- Add `pub icon: Option<String>` to `NoteRenameReq`.
- In `note_rename_handler`, pass the icon from request parameters down to `chat_db.rename_note(...)`.

### 3. Frontend UI Layout (`web/index.html`)
- Inside `note-settings-modal`, add a new form group for "Icon (Emoji)":
  ```html
  <div class="form-group">
      <label>Icon (Emoji)</label>
      <input type="text" id="note-settings-icon" placeholder="Default" maxlength="10" style="width: 80px;" />
  </div>
  ```

### 4. Frontend logic (`web/app.js`)
- Populate `#note-settings-icon` on opening settings in `showNoteSettings(sessionId)`.
- Extract the value and pass it in the `note/rename` POST API payload within `saveNoteSettings()`.
- Use the custom emoji in folder rendering:
  - If a custom emoji is set, use it.
  - Otherwise, fallback to the default (`notePublic ? ((s.id === currentNoteId) ? '📂' : '📁') : '🔐'`).
- Apply this logic to both desktop note list rendering and mobile note tree rendering.
