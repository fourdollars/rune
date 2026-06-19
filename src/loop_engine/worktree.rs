use std::path::PathBuf;
use std::process::Command;

pub struct WorktreeManager {
    pub loop_id: String,
    pub path: PathBuf,
}

impl WorktreeManager {
    pub fn create(loop_id: &str) -> std::io::Result<Self> {
        let path = std::env::current_dir()?
            .join(".git")
            .join("rune-worktrees")
            .join(loop_id);
        let branch = format!("rune/loop-{}", loop_id);

        // Ensure the parent directory (.git/rune-worktrees) exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // git branch rune/loop-ID
        // We ignore failure here since the branch might already exist.
        let _ = Command::new("git").args(&["branch", &branch]).output();

        // git worktree add path branch
        let output = Command::new("git")
            .args(&["worktree", "add", path.to_str().unwrap(), &branch])
            .output()?;

        if !output.status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "git worktree add failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }

        Ok(Self {
            loop_id: loop_id.to_string(),
            path,
        })
    }

    pub fn remove(&self) -> std::io::Result<()> {
        let output = Command::new("git")
            .args(&["worktree", "remove", "--force", self.path.to_str().unwrap()])
            .output()?;

        if !output.status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "git worktree remove failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }

        // Clean up the branch after worktree is removed to avoid dangling branches
        let branch = format!("rune/loop-{}", self.loop_id);
        let _ = Command::new("git")
            .args(&["branch", "-D", &branch])
            .output();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Use a static Mutex to prevent parallel tests from interfering with the current directory.
    static DIR_MUTEX: Mutex<()> = Mutex::new(());

    fn setup_temp_git_repo() -> (tempfile::TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().unwrap();
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

        // Write a dummy file and commit it so HEAD is pointing to a valid commit
        let file_path = path.join("dummy.txt");
        std::fs::write(&file_path, "dummy").unwrap();
        run_cmd(&["add", "dummy.txt"]);
        run_cmd(&["commit", "-m", "initial commit"]);

        (temp_dir, path)
    }

    #[test]
    fn test_worktree_lifecycle() {
        let _guard = DIR_MUTEX.lock().unwrap();
        let original_dir = env::current_dir().unwrap();

        let (_temp_dir, temp_path) = setup_temp_git_repo();
        env::set_current_dir(&temp_path).unwrap();

        let loop_id = "test-loop-id-123";
        let manager = WorktreeManager::create(loop_id).expect("Failed to create worktree");

        assert_eq!(manager.loop_id, loop_id);
        assert!(manager.path.exists());
        assert!(manager.path.join("dummy.txt").exists());

        manager.remove().expect("Failed to remove worktree");
        assert!(!manager.path.exists());

        // Restore current directory
        env::set_current_dir(&original_dir).unwrap();
    }
}
