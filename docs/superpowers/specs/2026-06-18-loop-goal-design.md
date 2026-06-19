# Design: `/loop`, `/goal`, and `/model` — Scheduled Execution & Goal-Driven Agents

**Date:** 2026-06-18
**Status:** Design v2 — updated after reading Claude Code docs  
**Scope:** CLI + rune notes (serve mode)

---

## 1. What We're Building

Three new features modelling Claude Code's `/loop`, `/goal`, and `/model`:

| Feature | Semantics |
|---|---|
| `/loop [interval] [model] [prompt]` | Schedule a prompt on a cron-like interval. No interval = Claude picks dynamically. No prompt = built-in maintenance prompt. |
| `/goal [model] condition` | Set a completion condition. After each agent turn, an evaluator (LLM or keyword) judges the condition. Session continues until met or cleared. |
| `/model [name]` | Show or switch the active model for the current CLI session. |

---

## 2. Reference: Key Differences from Initial Design

After reading the Claude Code docs, the correct understanding is:

| Aspect | Initial (wrong) design | Correct design |
|---|---|---|
| `/loop` mechanism | outer loop calling agent.run() repeatedly | **cron-based scheduler** with job IDs |
| `/goal` evaluator | GOAL_COMPLETE keyword only | **second LLM call** (fast/cheap model) |
| `/loop` no-prompt | empty input | **built-in maintenance prompt** |
| State | memory-only | CLI = memory; notes = chat_db |

---

## 3. `/loop` — Cron Scheduler

### 3.1 Syntax

```
/loop [interval] [[model]] [prompt]
/loop                        → maintenance prompt, Claude picks interval
/loop 5m                     → maintenance prompt, every 5 minutes
/loop 5m check the deploy    → custom prompt, every 5 minutes
/loop check the deploy       → custom prompt, Claude picks interval
/loop 5m [gemini-flash] check deploy  → custom model for this loop run
```

Interval formats: `30s`, `5m`, `2h`, `1d` (seconds round up to 1-minute cron granularity).

### 3.2 Cron Tools (new tools for CLI + notes)

Three new tools exposed to the LLM when loop/cron mode is active:

```json
CronCreate  { "cron": "*/5 * * * *", "prompt": "check deploy", "recurring": true }
CronList    {}
CronDelete  { "id": "a1b2c3d4" }
```

The agent calls these tools directly in response to natural-language requests:
```
"what loops do I have?"  → agent calls CronList
"cancel the deploy loop" → agent calls CronDelete with matching ID
```

`CronCreate` also accepts `"model"` for the evaluator model override.

### 3.3 Scheduler Implementation (CLI)

```
CliScheduler (tokio task, runs in background)
  ├─ holds Vec<CronJob> { id, cron_expr, prompt, model, recurring, created_at }
  ├─ checks every second for due jobs
  ├─ enqueues due job prompt into pending_prompt channel
  ├─ agent picks up pending_prompt between user inputs (non-blocking)
  └─ stops with the CLI session
```

State: **in-memory only** (session-scoped, matches Claude Code behaviour).  
The `--resume` equivalent is not planned for the initial version.

### 3.4 Built-in Maintenance Prompt

When `/loop` is invoked without a prompt:

```
Review the current state of the session:
1. Continue any unfinished work from the conversation.
2. If a goal is active, make progress toward it.
3. Run cleanup passes (bug hunts, refactoring) when nothing else is pending.
Do not start new initiatives outside this scope.
```

Customisable: if `.rune/loop.md` exists in the current directory, its content replaces the built-in prompt.

### 3.5 Dynamic Interval

When no interval is given, after each iteration the agent appends a line:

```
[next-check: 3m — PR is active, checking frequently]
```

The scheduler reads this annotation and schedules the next fire accordingly. Fallback: 10 minutes.

### 3.6 Stop / Cancel

| Context | How to stop |
|---|---|
| CLI (waiting between iterations) | `Esc` — clears the pending wakeup |
| CLI (while running) | Ctrl+C → `UserInterrupt` |
| Notes | `/api/chat/cancel` endpoint (existing) |
| Via LLM | Agent calls `CronDelete` |

---

## 4. `/goal` — Goal-Driven Continuation

### 4.1 Syntax

```
/goal [model] condition    → set goal, start immediately
/goal                      → show current goal status
/goal clear                → cancel active goal
```

Model is optional and enclosed in square brackets:

```
/goal all auth tests pass
/goal [claude-haiku-4-5] all auth tests pass
/goal [gemini-2-5-flash] CHANGELOG has entry for every merged PR
```

### 4.2 Evaluation

After each agent turn, the evaluator runs:

**With model specified OR global evaluator model configured:**
```
Short system prompt: "You are a goal evaluator. Answer yes or no only."
User prompt: "Goal: <condition>\n\nConversation so far: <last N turns>\n\nIs the goal met?"
→ "yes" → clear goal, print achieved
→ "no: <reason>" → inject reason as guidance for next turn, continue
```

**No model specified AND no global config:**
Fallback to keyword detection: if `FinalAnswer` contains `GOAL_COMPLETE`, treat as achieved.

### 4.3 Goal State

```rust
struct GoalState {
    condition: String,
    evaluator_model: Option<String>,   // None → keyword fallback
    started_at: Instant,
    turns: u32,
    tokens_spent: u32,
    last_reason: Option<String>,
}
```

One goal per session. Setting a new goal replaces the previous one.

### 4.4 Status Display (`/goal` with no args)

```
◎ Goal active [12m 34s | 7 turns | 4,230 tokens]
  Condition: all tests in test/auth pass and lint is clean
  Last evaluation: "Tests still failing in test/auth/token.rs:42 — fix in progress"
```

### 4.5 Interaction with `/loop`

| Combination | Behaviour |
|---|---|
| `/loop` without `/goal` | Loops on schedule, never stops automatically |
| `/goal` without `/loop` | Runs a new turn immediately after each turn ends |
| Both active | `/loop` fires on schedule; after each fired turn, evaluator checks goal |

### 4.6 Configuration (optional global evaluator model)

```toml
[goal]
evaluator_model = "claude-haiku-4-5"   # used when /goal has no inline [model]
```

---

## 5. `/model` — Model Switching (CLI)

### 5.1 Syntax

```
/model                  → show current model
/model claude-sonnet-4  → switch for this session
/model gemini-2-5-pro   → switch for this session
```

### 5.2 Implementation

```rust
"/model" => {
    println!("  {} {}", "model:".bold(), cfg.model.cyan());
}
cmd if cmd.starts_with("/model ") => {
    let name = cmd.strip_prefix("/model ").unwrap().trim();
    if name.is_empty() {
        eprintln!("  Usage: /model <name>");
    } else {
        agent.config.model = name.to_string();
        println!("  {} Model switched to: {}", "✓".green(), name.cyan());
    }
}
```

Notes already has `/api/model/switch` — no change needed there.

---

## 6. Notes (Serve Mode) Support

### 6.1 Chat-level Trigger

The web UI parses special prefixes from the chat input:

```
/loop 5m check the deploy   → sets up cron in room, no agent turn
/loop                       → starts maintenance loop
/goal all tests pass        → sets goal for this room
/goal clear                 → clears goal
/goal                       → shows goal status as system message
```

Alternatively, the UI exposes dedicated buttons (to decide in implementation phase).

### 6.2 Room State Additions

```rust
pub struct NoteRoom {
    // existing fields...
    pub cron_jobs: Mutex<Vec<CronJob>>,
    pub goal: TokioRwLock<Option<GoalState>>,
}
```

### 6.3 New SSE Events

```
loop_iteration  { job_id: String, iteration: u32, prompt_preview: String }
loop_done       { job_id: String, reason: "goal" | "max" | "cancel" | "error" }
goal_status     { active: bool, condition: String, turns: u32, last_reason: String }
goal_achieved   { condition: String, turns: u32, duration_secs: u32 }
cron_list       { jobs: Vec<CronJobSummary> }
```

### 6.4 New API Endpoints

```
POST /api/chat/cancel           { "note_id": "..." }   → cancel running task
POST /api/cron/create           { "note_id", "cron", "prompt", "model", "recurring" }
POST /api/cron/delete           { "note_id", "id" }
GET  /api/cron/list?note_id=    → list active cron jobs for a note
POST /api/goal/set              { "note_id", "condition", "model" }
POST /api/goal/clear            { "note_id" }
GET  /api/goal?note_id=         → current goal state
```

---

## 7. Inline `[model]` Parser

A shared utility function used by both CLI and notes:

```rust
/// Parse optional [model] token from a command string.
/// "/goal [claude-haiku-4-5] all tests pass" →
///   model = Some("claude-haiku-4-5"), rest = "all tests pass"
fn parse_inline_model(s: &str) -> (Option<String>, &str) {
    let s = s.trim();
    if s.starts_with('[') {
        if let Some(end) = s.find(']') {
            let model = s[1..end].trim().to_string();
            let rest = s[end + 1..].trim();
            return (Some(model), rest);
        }
    }
    (None, s)
}
```

---

## 8. Files to Change

| File | What changes |
|---|---|
| `src/cli/mod.rs` | Add `/loop`, `/goal`, `/model` slash commands; CliScheduler; goal state; help text |
| `src/agent/mod.rs` | Goal evaluation after each `run()`; evaluator LLM call; `GoalState` struct |
| `src/tools/mod.rs` | Add `CronCreate`/`CronList`/`CronDelete` tool definitions; expose when cron_mode=true |
| `src/serve/api.rs` | `NoteRoom` fields; new SSE events; new API handlers; goal evaluation in handle_chat_message |
| `src/serve/mod.rs` | Route new endpoints |
| `src/config/mod.rs` | Optional `[goal]` section with `evaluator_model` |
| `web/app.js` | Parse `/loop`/`/goal` prefix; cron/goal status UI; stop buttons; new SSE handlers |
| `web/index.html` | Loop/goal status bar element |
| `web/style.css` | Status bar styles |

**Not changed:** `Agent::run()` internal loop, Concourse mode, pipe mode.

---

## 9. Testing Plan

| Test | Type |
|---|---|
| `test_parse_inline_model` | Unit — parser for `[model]` bracket syntax |
| `test_cron_create_list_delete` | Unit — CronJob CRUD |
| `test_goal_keyword_fallback` | Unit — GOAL_COMPLETE detection |
| `test_goal_evaluator_call` | Unit (mock provider) — second LLM call evaluates condition |
| `test_loop_fires_on_schedule` | Integration — job fires after interval |
| `test_goal_achieved_clears` | Integration — goal clears when condition met |
| `test_cron_api_endpoints` | Integration — REST endpoints |
| `test_model_slash_command` | Unit — `/model` switches cfg.model |

---

## 10. Open Questions Resolved

| Question | Answer |
|---|---|
| `/loop` mechanism | Full cron scheduler (A) |
| `/goal` evaluator | Second LLM call with inline `[model]` support; keyword fallback (C) |
| Cron state storage | CLI = memory; notes = room state in memory (session-scoped) |
| `/loop` no-prompt | Built-in maintenance prompt (same as Claude Code) |
| `/model` command | New CLI slash command, session-scoped switch |

---

## 11. Remaining Question for User

**Notes input UX**: Should `/loop` and `/goal` be typed in the chat input box (same as CLI), or should the web UI have dedicated sidebar controls?

- **A**: Type in chat box (lowest friction, consistent with CLI)  
- **B**: Dedicated UI panel in the notes sidebar (more discoverable, less typing)

