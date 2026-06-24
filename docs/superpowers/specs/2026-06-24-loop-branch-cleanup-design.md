# Design: Loop Git Branch Naming and Cleanup on Failure

**Date:** 2026-06-24  
**Status:** Pending Review  
**Scope:** loop_engine (WorktreeManager & LoopEngine run_loop)

---

## 1. Overview & Goals

This specification outlines the improvements to the `rune` loop git branch handling:
1. **Fix Branch Naming:** Eliminate double prefixes like `rune/loop-loop-*` and `rune/loop-goal-*` by standardizing the naming logic.
2. **Auto-Cleanup on Failure:** Automatically remove the git worktree and delete the git branch if a loop reaches its maximum iterations or fails with a fatal error. Successful or paused loops will keep their branches.

---

## 2. Detailed Design

### 2.1 Branch Naming Helper

We will introduce a helper method `get_branch_name` in `src/loop_engine/worktree.rs` to centralize the mapping from `loop_id` to branch name:

```rust
fn get_branch_name(loop_id: &str) -> String {
    if loop_id.starts_with("loop-") || loop_id.starts_with("goal-") {
        format!("rune/{}", loop_id)
    } else {
        format!("rune/loop-{}", loop_id)
    }
}
```

This method will be used:
* When checking if a branch exists
* When creating a branch
* When adding the git worktree
* When removing the git worktree and deleting the branch

### 2.2 Run Loop Cleanup on Failure

In `src/loop_engine/mod.rs`, the `run_loop` method manages the execution loop. We will wrap the loop execution in a way that catches failure or exit paths:

1. **Failure Cases:**
   - The loop runs to completion of all iterations but `is_satisfied` remains false.
   - Any fatal setup or execution error occurs during the iterations that causes `run_loop` to exit with `Err`.
2. **Action:**
   - Call `worktree.remove()` to clean up the worktree directory and delete the branch.
3. **Success Cases:**
   - The verifier returns `GOAL_COMPLETE`. The status changes to `"Complete"`. The branch is kept so the user can merge it.
4. **Pause / Interrupt Cases:**
   - `check_cancellation` returns true. The status changes to `"Paused"`. The worktree and branch are kept to allow potential resume.

---

## 3. Testing Plan

* **Unit Tests for Naming:** Verify that `get_branch_name` handles `loop-` and `goal-` prefixed IDs correctly without double prefixing.
* **Failure Cleanup Test:** Add/modify tests to verify that if `run_loop` fails or is run with a failing verification, `worktree.remove()` is called, removing the worktree path and deleting the branch.
* **Success Keep Test:** Verify that when a goal is successful, the branch remains intact.
