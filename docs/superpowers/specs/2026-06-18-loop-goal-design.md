# Design: `/loop`, `/goal`, and `/model` — Persistent Goal-Driven Agents with Worktree Isolation

**Date:** 2026-06-19  
**Status:** Approved (v4)  
**Scope:** CLI + rune notes (serve mode)

---

## 1. Overview & Goals

This specification defines the integration of goal-driven execution loops, persistent scheduling, and safety measures within Rune. The goal is to evolve the agent from single-shot interactions into a persistent system capable of running complex loops safely and autonomously.

We implement three user-facing features:
1.  **`/loop`**: Schedules or runs a prompt repeatedly.
2.  **`/goal`**: Executes a loop that continues until an evaluation step signals the goal has been successfully completed.
3.  **`/model`**: Views or switches the active model for the session.
4.  **`/agent`**: Views or switches the active custom agent profile for the session.

To ensure safety, isolation, and reuse, the implementation is built on:
*   **Unified LoopEngine**: Orchestrates loop states across CLI and Notes via a `LoopModeAdapter` interface.
*   **Git Worktree Isolation**: Spawns isolated directories to run tasks without dirtying the user's workspace.
*   **Sub-agent Isolation**: Splitting work between an **Implementer** role (making changes) and a **Verifier** role (auditing and checking goals).
*   **Custom Agent Profiles**: Storing customized agent templates (models and prompts) globally under `[agents]` and referencing them directly under `[loop]`.
*   **Persistence & Audit Logs**: Saving states to disk (`~/.rune/loops/`) to support session resumption and tracing.

---

## 2. Architecture & Orchestration

The core architecture decouples loop control, file editing, verification, and output adapter logic:

```
                  ┌────────────────────────────────┐
                  │          LoopEngine            │
                  └──────────────┬─────────────────┘
                                 │
         ┌───────────────────────┼───────────────────────┐
         ▼                       ▼                       ▼
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│ LoopModeAdapter │     │ WorktreeManager │     │    Sub-Agents   │
│ (CLI / Notes)   │     │ (Git Isolation) │     │ (Impl / Verify) │
└─────────────────┘     └─────────────────┘     └─────────────────┘
```

### 2.1 The `LoopModeAdapter` Trait
Defined in `src/loop_engine/mod.rs`, this trait abstracts environment differences (CLI vs Notes):

```rust
pub trait LoopModeAdapter: Send + Sync {
    /// Called when the loop initializes.
    fn on_loop_start(&self, loop_id: &str, goal: &str);

    /// Called at the start of each iteration.
    fn on_iteration_start(&self, iteration: u32, max_iterations: u32);

    /// Called when an iteration's Implementer and Verifier steps complete.
    fn on_iteration_complete(&self, iteration: u32, summary: &IterationSummary);

    /// Checks if cancellation has been requested (Ctrl+C, UI Cancel, etc.).
    fn check_cancellation(&self) -> bool;

    /// Request manual input from the user (for Hybrid review mode).
    async fn request_human_input(&self, prompt: &str) -> Option<String>;
}
```

### 2.2 Persistence Layout
Every execution loop is stored locally under:
`~/.rune/loops/{loop_id}/`

1.  **`state.json`**:
    ```json
    {
      "loop_id": "loop-abc12345",
      "goal": "Refactor authentication tests to use tempfile",
      "status": "Running", // "Running" | "Paused" | "Complete" | "Failed"
      "current_iteration": 3,
      "max_iterations": 20,
      "worktree_path": "/home/user/project/.git/rune-worktrees/loop-abc12345",
      "created_at": "2026-06-19T15:10:00Z",
      "updated_at": "2026-06-19T15:15:00Z"
    }
    ```
2.  **`audit.jsonl`**:
    An append-only log recording every step:
    ```json
    {"timestamp": "...", "role": "Implementer", "action": "LLM_Request", "tokens": 4201}
    {"timestamp": "...", "role": "Implementer", "action": "Tool_Call", "tool": "write_file", "args": {...}}
    {"timestamp": "...", "role": "Verifier", "action": "Tool_Call", "tool": "execute_cmd", "cmd": "cargo test"}
    ```

---

## 3. Sub-agents & Verification

To prevent confirmation bias, we separate the loop execution context into two isolated sub-agents:

1.  **Implementer**:
    *   **Prompt**: Guided to write code, create files, and fix bugs based on the goal.
    *   **Context**: Clean prompt thread, receiving feedback from the Verifier from the previous iteration.
2.  **Verifier**:
    *   **Prompt**: Review changes. Instructed to run tests/linters and check if the goal is satisfied.
    *   **Constraint**: Under a strict read-only/verify system prompt. Must output `GOAL_COMPLETE` on success, or a detailed breakdown of failures if unsatisfied.

### Config
Users configure customized agent profiles in `~/.rune/rune.toml`:
```toml
# 1. Global Agent Registry
[agents.builder]
model = "gemini-2.5-flash"
system_prompt = "You are a builder. Focus on writing clean code and executing implementation tasks."

[agents.thinker]
model = "claude-sonnet-4"
system_prompt = "You are a thinker. Analyze requirements, create specs, and plan steps."

# 2. Loop Configuration (Direct Reference)
[loop]
implementer_agent = "builder"
verifier_agent = "thinker"
```

---

## 4. Git Worktree Isolation

To run loops without disrupting the active directory:

1.  **Branch Setup**: Create a branch `rune/loop-{loop_id}` at the current `HEAD`.
2.  **Checkout Worktree**: Excecute `git worktree add .git/rune-worktrees/{loop_id} rune/loop-{loop_id}`.
3.  **Sandbox Mapping**: Override the base path for all agent actions (like file reads/writes and command execution) to the worktree path.
4.  **Completion Strategy**:
    *   **Success**: CLI prompts user: `"Goal achieved! Merge changes into your active branch? [Y/n]"`.
    *   **Failure / Pause**: Keep the worktree directory intact to allow `/loop --resume <id>`.
    *   **Cleanup**: Once merged or explicitly discarded, call `git worktree remove --force <path>`.

---

## 5. Command Interface & Syntax

### 5.1 CLI Commands
*   `/loop [interval] [[model]] [prompt]`
    *   No interval = dynamically scheduled by model.
    *   No prompt = uses built-in maintenance prompt.
*   `/goal [[model]] <condition>`
    *   Initiates goal loop execution.
*   `/model [name]`
    *   Views or switches the active model for the session.
*   `/agent [name]`
    *   Views or switches the active agent profile for the session (loading its associated system prompt and model).

### 5.2 Notes API / SSE
*   **SSE Events**: `loop_iteration`, `loop_done`, `goal_status`, `goal_achieved`.
*   **Endpoints**:
    *   `POST /api/chat/cancel` (note_id)
    *   `POST /api/goal/set` (note_id, condition, model)
    *   `POST /api/goal/clear` (note_id)

---

## 6. Files to Modify

| File | Changes |
|---|---|
| `src/loop_engine/mod.rs` | Core loop logic, `LoopEngine` definition, state loading, and loop iteration controller. |
| `src/loop_engine/state.rs` | Definitions for `LoopState`, serialization, and `audit.jsonl` logging. |
| `src/loop_engine/worktree.rs` | `WorktreeManager` implementing `git worktree add`, `remove`, and path resolution. |
| `src/loop_engine/sub_agent.rs` | Orchestrating the Implementer and Verifier sub-agent prompts and invocation contexts. |
| `src/cli/mod.rs` | REPL commands for `/loop`, `/goal`, `/model`, and `/agent` parsing and CLI adapter implementation. |
| `src/serve/api.rs` | Notes SSE hooks and endpoint handlers for room-level loop/goal scheduling. |
| `src/serve/mod.rs` | Routing for new REST API paths. |
| `src/config/mod.rs` | Parse new global `[agents]` registry and `implementer_agent`/`verifier_agent` loop configs. |
| `src/agent/mod.rs` | Mapping file execution tools to the remapped worktree path if loop mode is active. |

---

## 7. Testing Plan

*   `test_worktree_creation_and_removal`: Verify git worktree commands execute correctly and cleanup handles modified files.
*   `test_state_persistence_and_resume`: Verify states write to `state.json` and a loop can resume from iteration N.
*   `test_sub_agent_handshake`: Verify Verifier feedback is correctly formatted and passed back to the Implementer.
*   `test_goal_keyword_and_evaluator`: Ensure both keyword completion and LLM condition evaluation stop loops correctly.
*   `test_agent_profile_resolution`: Ensure custom agent system prompt and model configurations are resolved correctly.
