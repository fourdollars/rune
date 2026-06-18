# Design: `/loop` and `/goal` — Persistent Agent Execution

**Date:** 2026-06-18  
**Status:** Design (pending approval)  
**Scope:** CLI + rune notes (serve mode)

---

## 1. What We're Building

Two new commands mirroring Claude Code's `/loop` and `/goal`, available in both the interactive CLI and the rune notes web UI.

| Command | Semantics |
|---|---|
| `/loop` | Run the agent repeatedly. After each `FinalAnswer`, automatically continue without user input, until the user stops it (Ctrl+C in CLI; Stop button in notes). |
| `/goal <text>` | Like `/loop`, but with automatic completion detection: stop when the agent signals it has achieved the goal. |

---

## 2. Core Design Decisions

### 2.1 Where does the outer loop live?

**Decision: Agent method, shared by CLI and notes.**

`Agent::run(input)` is a complete multi-step inner loop (handles N tool calls per call). `/loop` and `/goal` are an **outer loop** that calls `agent.run()` multiple times.

New method on `Agent`:

```rust
pub async fn run_loop<F, Fut>(
    &mut self,
    initial_input: &str,
    continuation_prompt: &str,
    goal: Option<&str>,
    max_iterations: Option<u32>,
    on_iteration: F,
) -> LoopStopReason
where
    F: Fn(u32, &StopReason) -> Fut,
    Fut: std::future::Future<Output = bool>, // return false = cancel
```

### 2.2 Goal completion detection

**Decision: Keyword marker (`GOAL_COMPLETE`).**

1. Inject into continuation prompt: *"When the goal is fully achieved, end your response with `GOAL_COMPLETE` on its own line."*
2. After each `FinalAnswer`, check `answer.contains("GOAL_COMPLETE")`.
3. Found → strip marker, stop with `GoalAchieved`.
4. Not found → continue next iteration.

No extra LLM call. Deterministic. User-visible.

### 2.3 Safety limit

| Context | Default max iterations |
|---|---|
| CLI `/loop` | 50 |
| CLI `/goal` | 50 |
| Notes (any loop) | 20 |

Override: `/loop 10` (CLI) or `loop_max` field in API request (notes).

### 2.4 Cancellation

| Context | Mechanism |
|---|---|
| CLI | Ctrl+C → `UserInterrupt` already wired into `Agent::run()`. Outer loop exits on `UserInterrupt`. |
| Notes | Existing `CancellationToken` per room. New `/api/chat/cancel` endpoint triggers it. |

---

## 3. CLI Changes (`src/cli/mod.rs`)

### New slash commands

```
/loop [max]          Persistent loop. Optional max iterations (default 50).
/goal <text> [max]   Loop until goal achieved. Optional max iterations.
```

### Console output during loop

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ Loop iteration 1 ━━━
<agent response>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ Loop iteration 2 ━━━
<agent response>
...
✅ Goal achieved after 3 iterations.
  or
⚠️  Loop stopped: max iterations (50) reached.
  or
⛔  Loop cancelled (Ctrl+C).
```

---

## 4. Notes (Serve Mode) Changes

### 4.1 Protocol: ChatReq extension

```rust
pub struct ChatReq {
    pub note_id: String,
    pub content: String,
    pub nickname: Option<String>,
    // NEW:
    pub loop_mode: Option<bool>,
    pub goal: Option<String>,
    pub loop_max: Option<u32>,
}
```

Frontend parses `/loop` or `/goal <text>` prefix from input box and sets these fields.

### 4.2 New SSE events

```
loop_iteration  { iteration: u32, max: u32 }
loop_done       { reason: "goal" | "max" | "cancel" | "error", iterations: u32 }
```

### 4.3 handle_chat_message

When `loop_mode = true` or `goal` is set, calls `agent.run_loop()` instead of `agent.run()`. The `on_iteration` callback broadcasts `loop_iteration` SSE. On finish, broadcasts `loop_done`.

### 4.4 New endpoint: `POST /api/chat/cancel`

```json
{ "note_id": "my-note" }
```

Fires the room's `CancellationToken`. Returns `{"ok": true}`.

### 4.5 Frontend UI

- Parse `/loop` or `/goal <text>` from input on Send.
- Show a banner:
  ```
  🔁 Loop mode — iteration 3/20  [Stop]
  🎯 Goal: "Refactor tests" — iteration 2/20  [Stop]
  ```
- **Stop** button calls `/api/chat/cancel`.
- On `loop_done`: banner disappears, system message shown.

---

## 5. `LoopStopReason` enum (new in `src/agent/mod.rs`)

```rust
pub enum LoopStopReason {
    GoalAchieved { iterations: u32 },
    MaxIterations { iterations: u32 },
    Cancelled     { iterations: u32 },
    InnerError    { iterations: u32, reason: StopReason },
}
```

---

## 6. Data Flow

### CLI

```
User types /goal <text>
  → parse goal
  → agent.run_loop(initial, continuation, goal, max, on_iter)
       ↳ loop {
           run(prompt) → FinalAnswer(answer)
           if GOAL_COMPLETE in answer → GoalAchieved
           if i >= max              → MaxIterations
           if UserInterrupt         → Cancelled
           prompt = continuation
         }
  → print LoopStopReason
```

### Notes

```
Browser: POST /api/chat { goal: "...", loop_max: 20 }
  → chat_handler spawns handle_chat_message(goal, max)
       ↳ agent.run_loop(
           initial      = content,
           continuation = "Continue. Goal: <text>. Signal GOAL_COMPLETE when done.",
           goal         = Some("..."),
           max          = Some(20),
           on_iter      = |i, _| broadcast loop_iteration
         )
       ↳ on finish → broadcast loop_done
```

---

## 7. Files to Change

| File | What changes |
|---|---|
| `src/agent/mod.rs` | Add `LoopStopReason` enum + `run_loop()` method |
| `src/cli/mod.rs` | Add `/loop` and `/goal` slash commands |
| `src/serve/api.rs` | Extend `ChatReq`; new SSE variants; extend `handle_chat_message`; `/api/chat/cancel` handler |
| `src/serve/mod.rs` | Route `/api/chat/cancel` |
| `web/app.js` | Parse prefix; banner; counter; cancel; new SSE events |
| `web/index.html` | Loop banner element |
| `web/style.css` | Banner styles |

**Not changed:** `Agent::run()`, Concourse mode, pipe mode, `StopReason` enum.

---

## 8. Testing Plan

| Test | Type |
|---|---|
| `test_run_loop_goal_achieved` | Unit — GOAL_COMPLETE detected after N iterations |
| `test_run_loop_max_iterations` | Unit — stops at max, returns MaxIterations |
| `test_run_loop_inner_error` | Unit — inner Error bubbles as InnerError |
| `test_chat_loop_mode_sse` | Integration — loop_iteration + loop_done events |
| `test_chat_cancel_endpoint` | Integration — /api/chat/cancel fires token |

---

## 9. Open Questions for User

**Q1 — `/loop` continuation prompt**: After a FinalAnswer in loop mode (no goal), what should the next prompt say?
- A: `"Review your work and continue if there is more to do."`
- B: `"What should you do next?"`
- C: Empty — agent continues from accumulated context

**Q2 — Notes input UX**: Should `/loop` and `/goal` be typed in the chat input box (Claude Code style), or as separate UI buttons?
- A: Parse from input box text (lowest friction, familiar)
- B: Dedicated Loop / Goal buttons next to Send

**Q3 — History persistence per iteration**: Should each iteration's `FinalAnswer` be saved to `chat.db`?
- A: Yes — full history, all iterations visible on reload
- B: No — only last iteration saved (cleaner history)
- C: Yes, but grouped under a single "loop run" entry

*Self-review: no TBDs, no contradictions, scope is focused.*
