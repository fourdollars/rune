pub mod state;
#[cfg(test)]
pub mod tests;
pub mod worktree;

use crate::agent::{Agent, StopReason};
use crate::config::RuneConfig;
use crate::loop_engine::state::{log_audit, now_rfc3339, save_state, IterationRecord, LoopState};
use crate::loop_engine::worktree::WorktreeManager;
use std::path::{Path, PathBuf};

pub trait LoopModeAdapter: Send + Sync {
    /// Called when the loop initializes.
    fn on_loop_start(&self, loop_id: &str, goal: &str);

    /// Called at the start of each iteration.
    fn on_iteration_start(&self, iteration: u32, max_iterations: u32);

    /// Called when an iteration's Implementer and Verifier steps complete.
    fn on_iteration_complete(&self, iteration: u32, record: &IterationRecord);

    /// Checks if cancellation has been requested (Ctrl+C, UI Cancel, etc.).
    fn check_cancellation(&self) -> bool;

    /// Request manual input from the user (for Hybrid review mode).
    fn request_human_input<'a>(
        &'a self,
        prompt: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
}

pub struct LoopEngine {
    pub config: RuneConfig,
    pub state_dir: PathBuf,
}

const VERIFIER_SYSTEM_PROMPT: &str = r#"You are the Verifier agent.
Your sole task is to review the changes made by the Implementer and verify if the goal has been successfully completed.
You must run tests, check files, or execute standard checks in the repository using the available tools to verify correctness.
If the goal is fully satisfied and all tests pass, you must output "GOAL_COMPLETE" as your final answer.
Otherwise, output a detailed explanation of what is still failing or incomplete, so the Implementer can fix it in the next iteration.
Be strict and rigorous. Do not assume something works without running the verification tools."#;

impl LoopEngine {
    pub fn new(config: RuneConfig, state_dir: PathBuf) -> Self {
        Self { config, state_dir }
    }

    fn get_agent_settings(&self, name_opt: Option<&str>) -> (String, Option<String>) {
        if let Some(name) = name_opt {
            if let Some(profile) = self.config.agents.get(name) {
                let model = profile
                    .model
                    .clone()
                    .unwrap_or_else(|| self.config.model.clone());
                let prompt = profile.system_prompt.clone();
                return (model, prompt);
            }
        }
        (self.config.model.clone(), None)
    }

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
            Ok(mut s) => {
                if s.status == "Complete" || s.status == "Failed" || s.goal != goal {
                    s.current_iteration = 0;
                    s.goal = goal.to_string();
                }
                s
            }
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
        state.worktree_path = Some(worktree.path.to_string_lossy().to_string());
        if let Err(e) = save_state(&state, &loop_dir.to_string_lossy()) {
            let _ = worktree.remove();
            return Err(e.into());
        }

        adapter.on_loop_start(loop_id, goal);

        match self
            .run_loop_inner(
                loop_id, goal, repo_path, adapter, &mut state, &worktree, &loop_dir,
            )
            .await
        {
            Ok(res) => Ok(res),
            Err(e) => {
                // If the loop was paused or cancelled during execution, transition status to Paused,
                // save state, log a loop_paused event, and propagate the error without deleting the worktree.
                if state.status == "Paused" || adapter.check_cancellation() {
                    state.status = "Paused".to_string();
                    state.updated_at = now_rfc3339();
                    let _ = save_state(&state, &loop_dir.to_string_lossy());
                    let _ = log_audit(
                        &loop_dir.to_string_lossy(),
                        "system",
                        "loop_paused",
                        serde_json::json!({
                            "reason": "user_cancelled_during_execution"
                        }),
                    );
                    Err(e)
                } else {
                    state.status = "Failed".to_string();
                    state.updated_at = now_rfc3339();
                    let _ = save_state(&state, &loop_dir.to_string_lossy());
                    let _ = log_audit(
                        &loop_dir.to_string_lossy(),
                        "system",
                        "loop_failed",
                        serde_json::json!({
                            "error": e.to_string()
                        }),
                    );
                    let _ = worktree.remove();
                    Err(e)
                }
            }
        }
    }

    async fn run_loop_inner<A: LoopModeAdapter>(
        &self,
        loop_id: &str,
        goal: &str,
        repo_path: &Path,
        adapter: &A,
        state: &mut LoopState,
        worktree: &WorktreeManager,
        loop_dir: &Path,
    ) -> Result<String, anyhow::Error> {
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
                save_state(state, &loop_dir.to_string_lossy())?;
                log_audit(
                    &loop_dir.to_string_lossy(),
                    "system",
                    "loop_paused",
                    serde_json::json!({
                        "iteration": iteration - 1,
                        "reason": "user_cancelled"
                    }),
                )?;
                return Ok("Loop paused by user".to_string());
            }

            adapter.on_iteration_start(iteration, max_iters);
            state.current_iteration = iteration;
            state.updated_at = now_rfc3339();
            save_state(state, &loop_dir.to_string_lossy())?;

            log_audit(
                &loop_dir.to_string_lossy(),
                "system",
                "iteration_start",
                serde_json::json!({
                    "iteration": iteration
                }),
            )?;

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
                    return Err(anyhow::anyhow!("Implementer error: {}", err));
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "Implementer stopped unexpectedly: {:?}",
                        other
                    ));
                }
            };

            log_audit(
                &loop_dir.to_string_lossy(),
                "Implementer",
                "run_complete",
                serde_json::json!({
                    "stop_reason": format!("{:?}", stop_reason),
                    "tokens_used": implementer.tokens_used(),
                    "duration_ms": impl_duration.as_millis() as u64
                }),
            )?;

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
                    return Err(anyhow::anyhow!("Verifier error: {}", err));
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "Verifier stopped unexpectedly: {:?}",
                        other
                    ));
                }
            };

            log_audit(
                &loop_dir.to_string_lossy(),
                "Verifier",
                "run_complete",
                serde_json::json!({
                    "stop_reason": format!("{:?}", verifier_stop),
                    "tokens_used": verifier.tokens_used(),
                    "duration_ms": verifier_duration.as_millis() as u64
                }),
            )?;

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
                save_state(state, &loop_dir.to_string_lossy())?;
                log_audit(
                    &loop_dir.to_string_lossy(),
                    "system",
                    "goal_achieved",
                    serde_json::json!({
                        "verifier_explanation": verifier_output
                    }),
                )?;

                return Ok(verifier_output);
            }

            verifier_feedback = format!(
                "Your previous implementation changes did not fully satisfy the goal.\nVerifier feedback:\n{}\n\nPlease address the feedback and adjust the files accordingly.",
                verifier_output
            );
        }

        log_audit(
            &loop_dir.to_string_lossy(),
            "system",
            "loop_exhausted",
            serde_json::json!({
                "max_iterations": max_iters
            }),
        )?;

        Err(anyhow::anyhow!(
            "Loop reached maximum iterations without satisfying the goal"
        ))
    }
}
