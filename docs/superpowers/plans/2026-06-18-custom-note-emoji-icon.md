# Custom Note Emoji Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable administrators to customize the note emoji icon in each note's settings dialog, persisting it in the SQLite database and displaying it on the desktop and mobile sidebars.

**Architecture:** Extend SQLite `sessions` schema to store an `icon` string, update the backend API handler for `note/rename` to receive and update it, add a field in the frontend Settings dialog, and dynamically update sidebar rendering to use the custom emoji if set.

**Tech Stack:** Rust (Axum, Rusqlite), JavaScript, HTML.

## Global Constraints
- All backend files must compile cleanly with `cargo check`.
- Code must format correctly using `cargo fmt --all`.
- Frontend assets must not be broken; static files tests must pass.

---

### Task 1: Database Schema & Operations Update

**Files:**
- Modify: `src/serve/db.rs`

**Interfaces:**
- Consumes: None (starting task)
- Produces: `NoteRecord` with `icon` field, updated `rename_note` helper.

- [ ] **Step 1: Update NoteRecord struct in `src/serve/db.rs`**
  Add `icon: Option<String>` to `NoteRecord`:
  ```rust
  pub struct NoteRecord {
      pub id: String,
      pub name: String,
      pub created_at: i64,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub created_by: Option<String>,
      pub public: bool,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub model_override: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub icon: Option<String>,
  }
  ```

- [ ] **Step 2: Add Alter Table Migration to schema initialization in `src/serve/db.rs`**
  Locate schema setup in `ChatDb::open` and add the alter statement:
  ```rust
  let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN icon TEXT;");
  ```
  And inside `ChatDb::open_lazy`:
  ```rust
  CREATE TABLE IF NOT EXISTS sessions (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL,
      created_at  INTEGER NOT NULL,
      created_by  TEXT,
      public      INTEGER DEFAULT 0,
      model_override TEXT,
      icon        TEXT
  );
  ```

- [ ] **Step 3: Update `list_notes` and `get_session` to retrieve the `icon` field**
  Modify queries to select `icon` and parse it:
  ```rust
  // In list_notes:
  "SELECT id, name, created_at, created_by, COALESCE(public, 0), model_override, icon FROM sessions ORDER BY name ASC"
  // Map row:
  Ok(NoteRecord {
      id: row.get(0)?,
      name: row.get(1)?,
      created_at: row.get(2)?,
      created_by: row.get(3)?,
      public: row.get::<_, i32>(4).unwrap_or(0) != 0,
      model_override: row.get(5)?,
      icon: row.get(6)?,
  })

  // In get_session:
  "SELECT id, name, created_at, created_by, COALESCE(public, 0), model_override, icon FROM sessions WHERE id = ?1"
  // Map row:
  Ok(NoteRecord {
      id: row.get(0)?,
      name: row.get(1)?,
      created_at: row.get(2)?,
      created_by: row.get(3)?,
      public: row.get::<_, i32>(4).unwrap_or(0) != 0,
      model_override: row.get(5)?,
      icon: row.get(6)?,
  })
  ```

- [ ] **Step 4: Update `rename_note` to update the `icon` column**
  Change function signature:
  ```rust
  pub fn rename_note(&self, id: &str, new_name: &str, icon: Option<&str>) -> anyhow::Result<Option<String>> {
  ```
  Update SQL operations within `rename_note`:
  ```rust
        if target_exists {
            conn.execute(
                "UPDATE sessions SET icon = ?1 WHERE id = ?2",
                params![icon, new_name],
            )?;
            conn.execute(
                "UPDATE messages SET note_id = ?1 WHERE note_id = ?2",
                params![new_name, id],
            )?;
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        } else {
            conn.execute(
                "UPDATE sessions SET id = ?1, name = ?1, icon = ?2 WHERE id = ?3",
                params![new_name, icon, id],
            )?;
            conn.execute(
                "UPDATE messages SET note_id = ?1 WHERE note_id = ?2",
                params![new_name, id],
            )?;
        }
  ```

- [ ] **Step 5: Verify backend builds**
  Run: `cargo check`
  Expected output: Compilation success (except possibly minor compile errors in `src/serve/api.rs` which will be fixed in Task 2).

- [ ] **Step 6: Commit changes**
  Run:
  ```bash
  git add src/serve/db.rs
  git commit -m "feat(serve): add icon column to sessions schema and update db operations"
  ```

---

### Task 2: API Handlers & Tests Update

**Files:**
- Modify: `src/serve/api.rs`

**Interfaces:**
- Consumes: `rename_note` signature from Task 1.
- Produces: Updated `/api/note/rename` handler.

- [ ] **Step 1: Update NoteRenameReq struct**
  Add `icon: Option<String>` to `NoteRenameReq`:
  ```rust
  #[derive(Debug, Deserialize)]
  pub struct NoteRenameReq {
      pub note_id: String,
      pub name: String,
      pub icon: Option<String>,
  }
  ```

- [ ] **Step 2: Update `note_rename_handler`**
  Modify invocation of `rename_note` to pass the icon:
  ```rust
  pub async fn note_rename_handler(
      State(state): State<ServerState>,
      Json(req): Json<NoteRenameReq>,
  ) -> Json<ApiResponse> {
      match state.chat_db.rename_note(&req.note_id, &req.name, req.icon.as_deref()) {
          ...
  ```

- [ ] **Step 3: Update `test_session_rename` unit test**
  Modify the mock payload to include `icon` field and assert persistence in `src/serve/api.rs`:
  ```rust
      // Inside test_session_rename function:
      let (_, rename_body) = post_json(
          &app,
          "/api/session/rename",
          json!({
              "note_id": "old-name",
              "name": "new-name-test",
              "icon": "🚀"
          }),
      )
      .await;
      // Assert that new note session icon is indeed "🚀" in db:
      let record = state.chat_db.get_session("new-name-test").unwrap().unwrap();
      assert_eq!(record.icon.as_deref(), Some("🚀"));
  ```

- [ ] **Step 4: Run unit tests**
  Run: `cargo test serve::api::tests::test_session_rename`
  Expected output: test result: ok. 1 passed; 0 failed

- [ ] **Step 5: Commit changes**
  Run:
  ```bash
  git add src/serve/api.rs
  git commit -m "feat(serve): update note rename handler and unit tests to support custom icons"
  ```

---

### Task 3: UI Layout Update

**Files:**
- Modify: `web/index.html`

- [ ] **Step 1: Add Icon input field to Note Settings Dialog in `web/index.html`**
  Insert the form group right below the "Name" input field (around line 97):
  ```html
              <div class="form-group">
                  <label>Icon (Emoji)</label>
                  <input type="text" id="note-settings-icon" placeholder="Default" maxlength="10" style="width: 80px;" />
              </div>
  ```

- [ ] **Step 2: Run static asset tests**
  Run: `cargo test serve::static_files::tests::test_static_assets_present`
  Expected: PASS

- [ ] **Step 3: Commit changes**
  Run:
  ```bash
  git add web/index.html
  git commit -m "feat(web): add icon field input to note settings layout"
  ```

---

### Task 4: Frontend Logic Update

**Files:**
- Modify: `web/app.js`

- [ ] **Step 1: Update Note Settings open logic**
  Locate `showNoteSettings(sessionId)` and populate `#note-settings-icon`:
  ```javascript
  function showNoteSettings(sessionId) {
      const s = notes.find(x => x.id === sessionId);
      if (!s) return;
      settingsNoteId = sessionId;
      document.getElementById('note-settings-title').textContent = 'Note: ' + s.name;
      document.getElementById('note-settings-name').value = s.name;
      document.getElementById('note-settings-icon').value = s.icon || '';
      // Hide delete button for default session
      const delBtn = document.getElementById('btn-delete-note');
      if (delBtn) delBtn.style.display = sessionId === 'default' ? 'none' : '';
      document.getElementById('note-settings-modal').classList.remove('hidden');
  }
  ```

- [ ] **Step 2: Update Note Settings save logic**
  Locate `saveNoteSettings()` and send `icon` in the payload:
  ```javascript
  function saveNoteSettings() {
      if (!settingsNoteId) return;
      const name = document.getElementById('note-settings-name').value.trim();
      const icon = document.getElementById('note-settings-icon').value.trim();
      const s = notes.find(x => x.id === settingsNoteId);
      if (s) {
          if (name !== s.name || icon !== (s.icon || '')) {
              api('note/rename', { note_id: settingsNoteId, name, icon: icon || null });
          }
      }
      hideNoteSettings();
  }
  ```

- [ ] **Step 3: Update Note Explorer sidebar icon rendering**
  Modify `renderNoteList()` to use custom icon if defined:
  ```javascript
          const notePublic = !!s.public;
          const folderIcon = document.createElement('span');
          folderIcon.className = 'icon' + (isAdmin ? ' clickable' : '') + (notePublic ? '' : ' private');
          if (s.icon) {
              folderIcon.textContent = s.icon;
          } else {
              folderIcon.textContent = notePublic ? ((s.id === currentNoteId) ? '📂' : '📁') : '🔐';
          }
  ```

- [ ] **Step 4: Update Mobile Note Tree rendering**
  Modify `renderMobileNoteTree()` to respect custom icon:
  ```javascript
          const notePublic = !!s.public;
          let noteIconStr = '';
          if (s.icon) {
              noteIconStr = s.icon;
          } else {
              noteIconStr = notePublic ? (s.id === currentNoteId ? '📂' : '📁') : '🔐';
          }
          noteHeader.innerHTML = '<span class="mobile-note-icon">' + noteIconStr + '</span><span class="mobile-note-name">' + (s.name || s.id) + '</span>';
  ```

- [ ] **Step 5: Run tests and verify code format**
  Run: `cargo test`
  Expected: PASS
  Run: `cargo fmt --all -- --check`
  Expected: exit code 0 (all formatted)

- [ ] **Step 6: Commit changes**
  Run:
  ```bash
  git add web/app.js
  git commit -m "feat(web): support custom note emoji icon rendering in sidebar and saving settings"
  ```
