# Design: Persist context_tokens in chat history

**Date:** 2026-06-08

## Problem

The "context used" overlay in `rune notes` shows what percentage of the model's context window the current conversation occupies. It is populated only by the live `chat_meta` SSE event, so it disappears on every page reload. The data needed to restore it — `context_tokens` — is not persisted with the chat history.

## Goal

Store `context_tokens` alongside each assistant message so that `replayHistory()` can restore the context overlay after a reload.

## Design

### 1. Database migration (`src/serve/db.rs`)

Add `context_tokens INTEGER` to the `messages` table.

The codebase uses additive `ALTER TABLE … ADD COLUMN` statements at startup to evolve the schema without destructive migrations. Add one more:

```sql
ALTER TABLE messages ADD COLUMN context_tokens INTEGER;
```

Existing rows get `NULL`, which the application treats as "unknown" (overlay stays hidden).

### 2. `ChatRecord` struct (`src/serve/db.rs`)

Add one field:

```rust
pub context_tokens: Option<i32>,
```

`Option<i32>` so NULL rows from before the migration deserialize without error.

### 3. `insert_with_meta` / `insert_with_meta_async` (`src/serve/db.rs`)

Add `context_tokens: Option<i32>` parameter. Write it to the new column in the `INSERT` statement.

### 4. Call site (`src/serve/api.rs`, chat handler ~line 2048)

Pass `Some(agent.total_context_tokens() as i32)` — the same value already emitted in the `ChatMeta` SSE event, so no new computation is needed.

### 5. `replayHistory()` (`web/app.js`)

After the replay loop, find the last assistant message that has a non-null `context_tokens`. Look up that message's `model` in the global `availableModels` array to get `context_window`. If both values are present, call `updateContextOverlay(m.context_tokens, contextWindow)`. If the model is not in `availableModels` (retired or switched), skip — the overlay stays hidden rather than showing a stale or wrong percentage.

## Data flow (after change)

```
agent.total_context_tokens()
  → insert_with_meta_async(..., context_tokens)
    → messages.context_tokens column (SQLite)
      → load_recent_async() → ChatRecord.context_tokens
        → history SSE / note/switch REST response
          → replayHistory() → updateContextOverlay()
```

## Out of scope

- `context_window` is not stored per-message; it is looked up from `availableModels` at replay time.
- No changes to the `chat_meta` SSE event or token estimation logic.
- No changes to the archive or search features.
