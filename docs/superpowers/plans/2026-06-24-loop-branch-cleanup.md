# Loop Git Branch Naming and Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clean up the branch naming format for goal/loop engines to avoid double prefixes (`rune/loop-loop-*`), and automatically clean up git worktrees and delete git branches when loops fail.

**Architecture:** Centralize branch name formatting in a `WorktreeManager` helper function. Wrap `LoopEngine::run_loop`'s core execution in a way that executes `worktree.remove()` upon failure/exhaustion of the loop, keeping it on success or pause.

**Tech Stack:** Rust, Git

## Global Constraints
- Target branch name formats: `rune/loop-<timestamp>` and `rune/goal-<timestamp>` when the `loop_id` starts with `"loop-"` or `"goal-"`.
- Worktrees and branches must be deleted upon failure (maximum iterations reached or fatal loop execution error), but kept on success or pause.
- Follow existing patterns in the codebase and write/update tests using `cargo test`.
- All code must pass `cargo fmt --all -- --check` and `cargo clippy`.

---

### Task 1: Add Branch Name Formatting Helper and Unit Tests

**Files:**
- Modify: `src/loop_engine/worktree.rs`
- Test: `src/loop_engine/worktree.rs`

**Interfaces:**
- Consumes: None
- Produces: `WorktreeManager::get_branch_name(loop_id: &str) -> String`

- [ ] **Step 1: Add tests for get_branch_name**

Add a test case in `src/loop_engine/worktree.rs` under `mod tests`:
```rust
    #[test]
    fn test_get_branch_name() {
        assert_eq!(WorktreeManager::get_branch_name("loop-1234"), "rune/loop-1234");
        assert_eq!(WorktreeManager::get_branch_name("goal-5678"), "rune/goal-5678");
        assert_eq!(WorktreeManager::get_branch_name("my-note"), "rune/loop-my-note");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test loop_engine::worktree::tests::test_get_branch_name`
Expected: Compile error because `get_branch_name` does not exist.

- [ ] **Step 3: Implement get_branch_name**

In `src/loop_engine/worktree.rs`, add the associated function `get_branch_name` on `WorktreeManager`:
```rust
impl WorktreeManager {
    pub fn get_branch_name(loop_id: &str) -> String {
        if loop_id.starts_with("loop-") || loop_id.starts_with("goal-") {
            format!("rune/{}", loop_id)
        } else {
            format!("rune/loop-{}", loop_id)
        }
    }
    // ...
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test loop_engine::worktree::tests::test_get_branch_name`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/loop_engine/worktree.rs
git commit -m "feat: add get_branch_name helper to WorktreeManager"
```

---

### Task 2: Refactor WorktreeManager to use get_branch_name Helper

**Files:**
- Modify: `src/loop_engine/worktree.rs`

**Interfaces:**
- Consumes: `WorktreeManager::get_branch_name`
- Produces: Updated `WorktreeManager::create` and `WorktreeManager::remove`

- [ ] **Step 1: Refactor worktree.rs code**

Replace occurrences of branch name formatting with `Self::get_branch_name`.
In `WorktreeManager::create`:
```rust
        let branch = Self::get_branch_name(loop_id);
```

In `WorktreeManager::remove`:
```rust
        let branch = Self::get_branch_name(&self.loop_id);
```

In `tests::test_worktree_existing_branch`:
```rust
        let branch = WorktreeManager::get_branch_name(loop_id);
```

- [ ] **Step 2: Run all worktree tests**

Run: `cargo test loop_engine::worktree::tests`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/loop_engine/worktree.rs
git commit -m "refactor: use get_branch_name helper in WorktreeManager"
```

---

### Task 3: Implement Automatic Cleanup on Loop Failure

**Files:**
- Modify: `src/loop_engine/mod.rs`
- Test: `src/loop_engine/tests.rs`

**Interfaces:**
- Consumes: `WorktreeManager::remove`
- Produces: LoopEngine automatically deletes worktree and branch on failure/exhaustion

- [ ] **Step 1: Write a failing integration test**

In `src/loop_engine/tests.rs`, add a test that executes a failing loop and asserts that the worktree and branch are cleaned up.
First, view `src/loop_engine/tests.rs` to see what helper functions we can use to stub the LoopModeAdapter and check the directory structure.

Let's write a new test `test_failing_loop_cleans_up_worktree` in `src/loop_engine/tests.rs`:
```rust
    #[tokio::test]
    async fn test_failing_loop_cleans_up_worktree() {
        let (temp_dir, repo_path) = setup_temp_git_repo();
        let loop_id = "test-failing-cleanup";
        let state_dir = temp_dir.path().join("loops");
        
        let engine = LoopEngine::new(
            crate::config::RuneConfig {
                model: "test-model".to_string(),
                loop_config: crate::config::LoopConfig {
                    max_iterations: 1,
                    implementer_agent: None,
                    verifier_agent: None,
                },
                ..Default::default()
            },
            state_dir.clone(),
        );

        let adapter = TestLoopAdapter {
            cancel: std::sync::atomic::AtomicBool::new(false),
        };

        // Verifier never says GOAL_COMPLETE, so this will fail.
        // We will pass a goal.
        let run_res = engine.run_loop(loop_id, "Always fail", &repo_path, &adapter).await;
        assert!(run_res.is_err());

        // Verify the worktree path is cleaned up
        let worktree_dir = repo_path.join(".git").join("rune-worktrees").join(loop_id);
        assert!(!worktree_dir.exists(), "Worktree directory should have been cleaned up");

        // Verify the branch was deleted
        let branch = WorktreeManager::get_branch_name(loop_id);
        let branch_ref = format!("refs/heads/{}", branch);
        let branch_exists = Command::new("git")
            .current_dir(&repo_path)
            .args(&["show-ref", "--verify", &branch_ref])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(!branch_exists, "Branch should have been deleted");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test loop_engine::tests::test_failing_loop_cleans_up_worktree`
Expected: FAIL (assertion fails: worktree directory or branch still exists after failure)

- [ ] **Step 3: Modify LoopEngine::run_loop to clean up on failure**

In `src/loop_engine/mod.rs`, modify the return points. If we reach the end of the iterations without satisfaction (meaning the loop is exhausted/failed) or if any error is encountered after the worktree is created, call `worktree.remove()` to clean up.
Specifically:
```rust
    pub async fn run_loop<A: LoopModeAdapter>(
        &self,
        loop_id: &str,
        goal: &str,
        repo_path: &Path,
        adapter: &A,
    ) -> Result<String, anyhow::Error> {
        let loop_dir = self.state_dir.join(loop_id);
        std::fs::create_dir_all(&loop_dir)?;

        let worktree = WorktreeManager::create(repo_path, loop_id)?;

        let mut state = match crate::loop_engine::state::load_state(&loop_dir.to_string_lossy()) {
            Ok(s) => s,
            Err(_) => LoopState {
                loop_id: loop_id.to_string(),
                goal: goal.to_string(),
                status: "Running".to_string(),
                current_iteration: 0,
                max_iterations: self.config.loop_config.max_iterations,
                worktree_path: Some(worktree.path.to_string_lossy().to_string()),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        };
        state.status = "Running".to_string();
        if let Err(e) = save_state(&state, &loop_dir.to_string_lossy()) {
            let _ = worktree.remove();
            return Err(e);
        }

        adapter.on_loop_start(loop_id, goal);

        let mut verifier_feedback = format!(
            "Goal to accomplish: {}\n\nPlease implement changes to satisfy this goal.",
            goal
        );

        let start_iteration = state.current_iteration;
        let max_iters = state.max_iterations;

        for iteration in (start_iteration + 1)..=max_iters {
            if adapter.check_cancellation() {
                state.status = "Paused".to_string();
                state.updated_at = now_rfc3339();
                let _ = save_state(&state, &loop_dir.to_string_lossy());
                let _ = log_audit(
                    &loop_dir.to_string_lossy(),
                    "system",
                    "loop_paused",
                    serde_json::json!({
                        "iteration": iteration - 1,
                        "reason": "user_cancelled"
                    }),
                );
                return Ok("Loop paused by user".to_string());
            }

            adapter.on_iteration_start(iteration, max_iters);
            state.current_iteration = iteration;
            state.updated_at = now_rfc3339();
            if let Err(e) = save_state(&state, &loop_dir.to_string_lossy()) {
                let _ = worktree.remove();
                return Err(e);
            }

            if let Err(e) = log_audit(
                &loop_dir.to_string_lossy(),
                "system",
                "iteration_start",
                serde_json::json!({
                    "iteration": iteration
                }),
            ) {
                let _ = worktree.remove();
                return Err(e);
            }

            // --- 1. Run Implementer ---
            let (impl_model, impl_prompt_opt) =
                self.get_agent_settings(self.config.loop_config.implementer_agent.as_deref());
            let mut impl_config = self.config.clone();
            impl_config.model = impl_model;

            let mut implementer = Agent::new(
                impl_config,
                crate::cli::init_provider(&self.config),
                false, // non-interactive
                None,
            );
            if let Some(ref prompt) = impl_prompt_opt {
                implementer.set_system_prompt(prompt);
            }
            implementer.worktree_path = Some(worktree.path.clone());

            let impl_start = std::time::Instant::now();
            let stop_reason = implementer.run(&verifier_feedback).await;
            let impl_duration = impl_start.elapsed();

            let impl_output = match &stop_reason {
                StopReason::FinalAnswer(ref ans) => ans.clone(),
                StopReason::Error(ref err) => {
                    let _ = worktree.remove();
                    return Err(anyhow::anyhow!("Implementer error: {}", err));
                }
                other => {
                    let _ = worktree.remove();
                    return Err(anyhow::anyhow!("Implementer stopped unexpectedly: {:?}", other));
                }
            };

            if let Err(e) = log_audit(
                &loop_dir.to_string_lossy(),
                "Implementer",
                "run_complete",
                serde_json::json!({
                    "stop_reason": format!("{:?}", stop_reason),
                    "tokens_used": implementer.tokens_used(),
                    "duration_ms": impl_duration.as_millis() as u64
                }),
            ) {
                let _ = worktree.remove();
                return Err(e);
            }

            // --- 2. Run Verifier ---
            let (verifier_model, verifier_prompt_opt) =
                self.get_agent_settings(self.config.loop_config.verifier_agent.as_deref());
            let mut verifier_config = self.config.clone();
            verifier_config.model = verifier_model;

            let mut verifier = Agent::new(
                verifier_config,
                crate::cli::init_provider(&self.config),
                false, // non-interactive
                None,
            );
            let verifier_prompt_combined = match verifier_prompt_opt {
                Some(custom) => format!("{}\n\n{}", custom, VERIFIER_SYSTEM_PROMPT),
                None => VERIFIER_SYSTEM_PROMPT.to_string(),
            };
            verifier.set_system_prompt(&verifier_prompt_combined);
            verifier.worktree_path = Some(worktree.path.clone());

            let verifier_input = format!(
                "Goal to verify: {}\n\nImplementer has updated the files and reported:\n{}\n\nPlease verify these changes now.",
                goal, impl_output
            );

            let verifier_start = std::time::Instant::now();
            let verifier_stop = verifier.run(&verifier_input).await;
            let verifier_duration = verifier_start.elapsed();

            let verifier_output = match &verifier_stop {
                StopReason::FinalAnswer(ref ans) => ans.clone(),
                StopReason::Error(ref err) => {
                    let _ = worktree.remove();
                    return Err(anyhow::anyhow!("Verifier error: {}", err));
                }
                other => {
                    let _ = worktree.remove();
                    return Err(anyhow::anyhow!("Verifier stopped unexpectedly: {:?}", other));
                }
            };

            if let Err(e) = log_audit(
                &loop_dir.to_string_lossy(),
                "Verifier",
                "run_complete",
                serde_json::json!({
                    "stop_reason": format!("{:?}", verifier_stop),
                    "tokens_used": verifier.tokens_used(),
                    "duration_ms": verifier_duration.as_millis() as u64
                }),
            ) {
                let _ = worktree.remove();
                return Err(e);
            }

            let is_satisfied = verifier_output.contains("GOAL_COMPLETE");

            let record = IterationRecord {
                iteration,
                input_summary: verifier_feedback.clone(),
                tool_calls: implementer.tool_call_names().to_vec(),
                output_summary: impl_output.clone(),
                tokens_used: Some(implementer.tokens_used() + verifier.tokens_used()),
                duration_ms: Some((impl_duration + verifier_duration).as_millis() as u64),
                error: if is_satisfied {
                    None
                } else {
                    Some(verifier_output.clone())
                },
            };

            adapter.on_iteration_complete(iteration, &record);

            if is_satisfied {
                state.status = "Complete".to_string();
                state.updated_at = now_rfc3339();
                save_state(&state, &loop_dir.to_string_lossy())?;
                let _ = log_audit(
                    &loop_dir.to_string_lossy(),
                    "system",
                    "goal_achieved",
                    serde_json::json!({
                        "verifier_explanation": verifier_output
                    }),
                );
                
                // Keep branch, but remove the worktree itself
                let _ = Command::new("git")
                    .current_dir(repo_path)
                    .arg("worktree")
                    .arg("remove")
                    .arg("--force")
                    .arg(&worktree.path)
                    .output();
                
                return Ok(verifier_output);
            }

            verifier_feedback = format!(
                "Your previous implementation changes did not fully satisfy the goal.\nVerifier feedback:\n{}\n\nPlease address the feedback and adjust the files accordingly.",
                verifier_output
            );
        }

        state.status = "Failed".to_string();
        state.updated_at = now_rfc3339();
        let _ = save_state(&state, &loop_dir.to_string_lossy());
        let _ = log_audit(
            &loop_dir.to_string_lossy(),
            "system",
            "loop_exhausted",
            serde_json::json!({
                "max_iterations": max_iters
            }),
        );

        // Failure cleanup
        let _ = worktree.remove();

        Err(anyhow::anyhow!(
            "Loop reached maximum iterations without satisfying the goal"
        ))
    }
```

- [ ] **Step 4: Run the failure cleanup integration test**

Run: `cargo test loop_engine::tests::test_failing_loop_cleans_up_worktree`
Expected: PASS

- [ ] **Step 5: Run all tests in loop_engine**

Run: `cargo test loop_engine::`
Expected: PASS

- [ ] **Step 6: Run cargo fmt and cargo clippy**

Run: `cargo fmt --all -- --check` and `cargo clippy --all-targets`
Expected: PASS with zero warnings/errors.

- [ ] **Step 7: Commit**

```bash
git add src/loop_engine/mod.rs src/loop_engine/tests.rs
git commit -m "feat: automatically clean up worktree and delete branch on loop failure"
```
