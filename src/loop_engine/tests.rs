use super::*;
use crate::config::RuneConfig;
use std::sync::Mutex;
use tempfile::tempdir;

struct TestAdapter {
    on_loop_start_called: Mutex<bool>,
    on_iteration_start_called: Mutex<bool>,
    on_iteration_complete_called: Mutex<bool>,
}

impl LoopModeAdapter for TestAdapter {
    fn on_loop_start(&self, _loop_id: &str, _goal: &str) {
        *self.on_loop_start_called.lock().unwrap() = true;
    }
    fn on_iteration_start(&self, _iteration: u32, _max_iterations: u32) {
        *self.on_iteration_start_called.lock().unwrap() = true;
    }
    fn on_iteration_complete(&self, _iteration: u32, _record: &IterationRecord) {
        *self.on_iteration_complete_called.lock().unwrap() = true;
    }
    fn check_cancellation(&self) -> bool {
        false
    }
    fn request_human_input<'a>(
        &'a self,
        _prompt: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

fn setup_temp_git_repo() -> (tempfile::TempDir, PathBuf) {
    use std::process::Command;
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_path_buf();

    let run_cmd = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&path)
            .status()
            .expect("Failed to execute git");
        assert!(status.success(), "Git command failed: {:?}", args);
    };

    run_cmd(&["init"]);
    run_cmd(&["config", "user.name", "Test User"]);
    run_cmd(&["config", "user.email", "test@example.com"]);
    run_cmd(&["config", "commit.gpgsign", "false"]);

    let file_path = path.join("dummy.txt");
    std::fs::write(&file_path, "dummy").unwrap();
    run_cmd(&["add", "dummy.txt"]);
    run_cmd(&["commit", "-m", "initial commit"]);

    (temp_dir, path)
}

#[tokio::test]
async fn test_run_loop_completes() {
    let (_temp_repo, repo_path) = setup_temp_git_repo();
    let temp_state = tempdir().unwrap();

    let mut config = RuneConfig::default();
    config.model = "mock-loop".to_string();
    config.provider = Some("mock-loop".to_string());
    config.api_key = Some("dummy-key".to_string());
    config.loop_config.max_iterations = 5;

    let engine = LoopEngine::new(config, temp_state.path().to_path_buf());
    let adapter = TestAdapter {
        on_loop_start_called: Mutex::new(false),
        on_iteration_start_called: Mutex::new(false),
        on_iteration_complete_called: Mutex::new(false),
    };

    let result = engine
        .run_loop(
            "test-run-loop-id",
            "Satisfy this goal",
            &repo_path,
            &adapter,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.contains("GOAL_COMPLETE"));

    assert!(*adapter.on_loop_start_called.lock().unwrap());
    assert!(*adapter.on_iteration_start_called.lock().unwrap());
    assert!(*adapter.on_iteration_complete_called.lock().unwrap());
}

#[tokio::test]
async fn test_failing_loop_cleans_up_worktree() {
    let (temp_dir, repo_path) = setup_temp_git_repo();
    let loop_id = "test-failing-cleanup";
    let state_dir = temp_dir.path().join("loops");

    let mut config = RuneConfig::default();
    config.model = "mock-loop".to_string();
    config.provider = Some("mock-loop".to_string());
    config.api_key = Some("dummy-key".to_string());
    config.loop_config.max_iterations = 1;

    let engine = LoopEngine::new(config, state_dir);
    let adapter = TestAdapter {
        on_loop_start_called: Mutex::new(false),
        on_iteration_start_called: Mutex::new(false),
        on_iteration_complete_called: Mutex::new(false),
    };

    let run_res = engine
        .run_loop(loop_id, "Always fail", &repo_path, &adapter)
        .await;
    assert!(run_res.is_err());

    // Verify the worktree path is cleaned up
    let worktree_dir = repo_path.join(".git").join("rune-worktrees").join(loop_id);
    assert!(
        !worktree_dir.exists(),
        "Worktree directory should have been cleaned up"
    );

    // Verify the branch was deleted
    let branch = WorktreeManager::get_branch_name(loop_id);
    let branch_ref = format!("refs/heads/{}", branch);
    let branch_exists = std::process::Command::new("git")
        .current_dir(&repo_path)
        .args(&["show-ref", "--verify", &branch_ref])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(!branch_exists, "Branch should have been deleted");
}

struct CancelAdapter {
    cancelled: Mutex<bool>,
}

impl LoopModeAdapter for CancelAdapter {
    fn on_loop_start(&self, _loop_id: &str, _goal: &str) {}
    fn on_iteration_start(&self, _iteration: u32, _max_iterations: u32) {}
    fn on_iteration_complete(&self, _iteration: u32, _record: &IterationRecord) {}
    fn check_cancellation(&self) -> bool {
        *self.cancelled.lock().unwrap()
    }
    fn request_human_input<'a>(
        &'a self,
        _prompt: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

#[tokio::test]
async fn test_loop_engine_cancellation() {
    let (temp_dir, repo_path) = setup_temp_git_repo();
    let loop_id = "test-cancellation-id";
    let state_dir = temp_dir.path().join("loops");

    let mut config = RuneConfig::default();
    config.model = "mock-loop".to_string();
    config.provider = Some("mock-loop".to_string());
    config.api_key = Some("dummy-key".to_string());
    config.loop_config.max_iterations = 3;

    let engine = LoopEngine::new(config, state_dir.clone());
    let adapter = CancelAdapter {
        cancelled: Mutex::new(true),
    };

    let run_res = engine
        .run_loop(loop_id, "Some goal", &repo_path, &adapter)
        .await;
    assert!(run_res.is_ok());
    assert_eq!(run_res.unwrap(), "Loop paused by user");

    // Verify the state on disk has status "Paused"
    let state =
        crate::loop_engine::state::load_state(&state_dir.join(loop_id).to_string_lossy()).unwrap();
    assert_eq!(state.status, "Paused");

    // Verify the worktree path is NOT cleaned up
    let worktree_dir = repo_path.join(".git").join("rune-worktrees").join(loop_id);
    assert!(
        worktree_dir.exists(),
        "Worktree directory should not have been cleaned up when paused"
    );

    // Clean up
    let worktree = WorktreeManager::create(&repo_path, loop_id).unwrap();
    let _ = worktree.remove();
}
