# context_tokens History Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist `context_tokens` in the chat history database so the "context used" overlay can be restored after a page reload.

**Architecture:** Add `context_tokens INTEGER` to the SQLite `messages` table via the existing additive-migration pattern, thread the new field through `ChatRecord` → `insert_with_meta` → `load_recent` → history SSE/REST, then call `updateContextOverlay()` at the end of `replayHistory()` in the frontend.

**Tech Stack:** Rust (rusqlite, tokio), JavaScript (vanilla)

---

## File map

| File | Change |
|---|---|
| `src/serve/db.rs` | Migration, `ChatRecord`, `insert_with_meta`, `insert_with_meta_async`, `insert_async`, `load_recent`, `archive`, `search` |
| `src/serve/api.rs` | Pass `context_tokens` to `insert_with_meta_async` |
| `web/app.js` | Restore overlay in `replayHistory()` |

---

### Task 1: Add `context_tokens` to `ChatRecord` and the DB migration

**Files:**
- Modify: `src/serve/db.rs`

- [ ] **Step 1: Write the failing test**

Add this test inside the `#[cfg(test)]` block in `src/serve/db.rs` (after the existing `test_insert_with_meta_persists_model_tokens` test, ~line 964):

```rust
#[test]
fn test_insert_with_meta_persists_context_tokens() {
    let db = in_memory_db();
    db.insert_with_meta(
        "default",
        "assistant",
        "ᚱᚢᚾᛖ",
        "hello",
        Some("gpt-5-mini"),
        Some(100),
        Some(42),
        Some(3),
        Some(2),
        None,
        Some(4200),
    )
    .unwrap();
    let rows = db.load_recent("default", 1).unwrap();
    assert_eq!(rows[0].context_tokens, Some(4200));
}

#[test]
fn test_insert_with_meta_context_tokens_none() {
    let db = in_memory_db();
    db.insert_with_meta(
        "default",
        "assistant",
        "ᚱᚢᚾᛖ",
        "hello",
        Some("gpt-5-mini"),
        Some(100),
        Some(42),
        Some(3),
        Some(2),
        None,
        None,
    )
    .unwrap();
    let rows = db.load_recent("default", 1).unwrap();
    assert!(rows[0].context_tokens.is_none());
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p rune test_insert_with_meta_persists_context_tokens test_insert_with_meta_context_tokens_none 2>&1 | tail -20
```

Expected: compile error — `context_tokens` field does not exist on `ChatRecord` and `insert_with_meta` has wrong arity.

- [ ] **Step 3: Add `context_tokens` field to `ChatRecord`**

In `src/serve/db.rs`, update the `ChatRecord` struct (currently ends at line ~41). Add after `thinking`:

```rust
    /// Total context tokens at the time of this response (assistant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<i32>,
```

- [ ] **Step 4: Add the DB migration**

In `src/serve/db.rs`, find the block of `let _ = conn.execute_batch(...)` additive migrations (around line 99–122). Add at the end of that block:

```rust
        let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN context_tokens INTEGER;");
```

- [ ] **Step 5: Update `insert_with_meta` signature and SQL**

Replace the existing `insert_with_meta` function body (lines ~233–257) with:

```rust
    pub fn insert_with_meta(
        &self,
        note_id: &str,
        role: &str,
        nickname: &str,
        content: &str,
        model: Option<&str>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<&str>,
        context_tokens: Option<i32>,
    ) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO messages (note_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![note_id, role, nickname, content, ts, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens],
        )?;
        Ok(conn.last_insert_rowid())
    }
```

- [ ] **Step 6: Update `insert_with_meta_async` signature and call**

Replace the existing `insert_with_meta_async` function (lines ~309–341) with:

```rust
    /// Async wrapper for insert_with_meta.
    pub async fn insert_with_meta_async(
        &self,
        note_id: String,
        role: String,
        nickname: String,
        content: String,
        model: Option<String>,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        steps: Option<i32>,
        tool_calls: Option<i32>,
        thinking: Option<String>,
        context_tokens: Option<i32>,
    ) {
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.insert_with_meta(
                &note_id,
                &role,
                &nickname,
                &content,
                model.as_deref(),
                tokens_in,
                tokens_out,
                steps,
                tool_calls,
                thinking.as_deref(),
                context_tokens,
            ) {
                warn!("Failed to persist chat message: {}", e);
            }
        })
        .await
        .ok();
    }
```

- [ ] **Step 7: Update `insert_async` to pass `None` for the new field**

`insert_async` (lines ~294–306) forwards to `insert_with_meta_async`. Add `None` as the final argument:

```rust
    pub async fn insert_async(
        &self,
        note_id: String,
        role: String,
        nickname: String,
        content: String,
    ) {
        self.insert_with_meta_async(
            note_id, role, nickname, content, None, None, None, None, None, None, None,
        )
        .await;
    }
```

- [ ] **Step 8: Update `load_recent` to select and map the new column**

Replace the SQL and row-mapping in `load_recent` (lines ~260–292) with:

```rust
    pub fn load_recent(&self, note_id: &str, limit: usize) -> anyhow::Result<Vec<ChatRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens
             FROM messages
             WHERE note_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let rows: Vec<ChatRecord> = stmt
            .query_map(params![note_id, limit as i64], |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    role: row.get(2)?,
                    nickname: row.get(3)?,
                    content: row.get(4)?,
                    created_at: row.get(5)?,
                    model: row.get(6)?,
                    tokens_in: row.get(7)?,
                    tokens_out: row.get(8)?,
                    steps: row.get(9)?,
                    tool_calls: row.get(10)?,
                    thinking: row.get(11).ok().flatten(),
                    context_tokens: row.get(12).ok().flatten(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }
```

- [ ] **Step 9: Update `archive` query and row-mapping**

In `archive` (lines ~606–628), update the SELECT and the `ChatRecord` construction:

```rust
        let mut stmt = conn.prepare(
            "SELECT id, note_id, role, nickname, content, created_at, model, tokens_in, tokens_out, steps, tool_calls, thinking, context_tokens
             FROM messages WHERE note_id = ?1 ORDER BY id ASC",
        )?;
        let records: Vec<ChatRecord> = stmt
            .query_map(params![note_id], |row| {
                Ok(ChatRecord {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    role: row.get(2)?,
                    nickname: row.get(3)?,
                    content: row.get(4)?,
                    created_at: row.get(5)?,
                    model: row.get(6)?,
                    tokens_in: row.get(7)?,
                    tokens_out: row.get(8)?,
                    steps: row.get(9)?,
                    tool_calls: row.get(10)?,
                    thinking: row.get(11).ok().flatten(),
                    context_tokens: row.get(12).ok().flatten(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
```

- [ ] **Step 10: Update `search` live-DB query and row-mapping**

In `search` (lines ~686–706), update both the SELECT and the `ChatRecord` construction identically to the archive step above (same SQL columns, same row index mapping).

- [ ] **Step 11: Run the new tests to confirm they pass**

```bash
cargo test -p rune test_insert_with_meta_persists_context_tokens test_insert_with_meta_context_tokens_none 2>&1 | tail -10
```

Expected: `test result: ok. 2 passed`

- [ ] **Step 12: Run the full test suite**

```bash
cargo test --all --no-fail-fast 2>&1 | tail -20
```

Expected: all tests pass (no regressions in existing db tests).

- [ ] **Step 13: Commit**

```bash
git add src/serve/db.rs
git commit -m "feat(db): add context_tokens column to messages history"
```

---

### Task 2: Pass `context_tokens` from the chat handler

**Files:**
- Modify: `src/serve/api.rs:2051–2063`

- [ ] **Step 1: Update the `insert_with_meta_async` call**

In `src/serve/api.rs`, find the `insert_with_meta_async` call inside the `StopReason::FinalAnswer` branch (~line 2051). Add `Some(agent.total_context_tokens() as i32)` as the final argument:

```rust
            state
                .chat_db
                .insert_with_meta_async(
                    note_id.clone(),
                    "assistant".to_string(),
                    "ᚱᚢᚾᛖ".to_string(),
                    answer.clone(),
                    Some(meta_model.clone()),
                    Some(agent.tokens_in() as i32),
                    Some(agent.tokens_out() as i32),
                    Some(agent.step_count() as i32),
                    Some(agent.tool_call_count() as i32),
                    meta_thinking,
                    Some(agent.total_context_tokens() as i32),
                )
                .await;
```

- [ ] **Step 2: Build to confirm no compile errors**

```bash
cargo build 2>&1 | grep -E "^error" | head -20
```

Expected: no output (clean build).

- [ ] **Step 3: Commit**

```bash
git add src/serve/api.rs
git commit -m "feat(api): persist context_tokens when saving assistant messages"
```

---

### Task 3: Restore the context overlay in `replayHistory()`

**Files:**
- Modify: `web/app.js`

- [ ] **Step 1: Add overlay restore at the end of `replayHistory()`**

In `web/app.js`, find the end of `replayHistory()`. Currently the function ends with (lines ~861–863):

```javascript
    addSystemMessage('── current ──');
    chatMessages.scrollTop = chatMessages.scrollHeight;
}
```

Replace that closing block with:

```javascript
    addSystemMessage('── current ──');
    chatMessages.scrollTop = chatMessages.scrollHeight;

    // Restore context overlay from the last assistant message that has context_tokens
    const lastWithCtx = [...messages].reverse().find(
        m => m.role === 'assistant' && m.context_tokens != null && m.model
    );
    if (lastWithCtx) {
        const modelEntry = availableModels.find(m => m.id === lastWithCtx.model);
        if (modelEntry && modelEntry.context_window) {
            updateContextOverlay(lastWithCtx.context_tokens, modelEntry.context_window);
        }
    }
}
```

- [ ] **Step 2: Verify the build still compiles (Rust side unchanged, but confirm)**

```bash
cargo build 2>&1 | grep -E "^error" | head -5
```

Expected: no output.

- [ ] **Step 3: Manual smoke test**

1. Start `rune notes` and open a note in the browser.
2. Send a message and wait for the assistant reply — confirm the context overlay appears (e.g. "3% context used").
3. Reload the page — confirm the overlay reappears with the same percentage.
4. Check a note that has no chat history — confirm the overlay is hidden after reload (no spurious display).

- [ ] **Step 4: Commit**

```bash
git add web/app.js
git commit -m "feat(ui): restore context-used overlay from replayed history"
```
