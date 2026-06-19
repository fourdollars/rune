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
