use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WorktreeManager {
    pub repo_path: PathBuf,
    pub loop_id: String,
    pub path: PathBuf,
}

impl WorktreeManager {
    pub fn create<P: AsRef<Path>>(repo_path: P, loop_id: &str) -> std::io::Result<Self> {
        let repo_path = repo_path.as_ref().to_path_buf();

        // Find the git common dir safely to support subdirectories and checkouts
        let output = Command::new("git")
            .args(&["rev-parse", "--git-common-dir"])
            .current_dir(&repo_path)
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Not in a git repository: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let git_common_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let git_common_path = PathBuf::from(git_common_str);
        let abs_git_common = if git_common_path.is_absolute() {
            git_common_path
        } else {
            repo_path.join(git_common_path)
        };
        let abs_git_common = std::fs::canonicalize(abs_git_common)?;

        let path = abs_git_common.join("rune-worktrees").join(loop_id);
        let branch = format!("rune/loop-{}", loop_id);

        // Ensure the parent directory (.git/rune-worktrees) exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Check if branch already exists
        let branch_ref = format!("refs/heads/{}", branch);
        let branch_exists = Command::new("git")
            .current_dir(&repo_path)
            .args(&["show-ref", "--verify", &branch_ref])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        if !branch_exists {
            // git branch rune/loop-ID
            let output = Command::new("git")
                .current_dir(&repo_path)
                .args(&["branch", &branch])
                .output()?;

            if !output.status.success() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "git branch failed to create branch {}: {}",
                        branch,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                ));
            }
        }

        // git worktree add path branch
        // Avoid to_str().unwrap() by passing Path directly to arg
        let output = Command::new("git")
            .current_dir(&repo_path)
            .arg("worktree")
            .arg("add")
            .arg(&path)
            .arg(&branch)
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
            repo_path,
            loop_id: loop_id.to_string(),
            path,
        })
    }

    pub fn remove(&self) -> std::io::Result<()> {
        // Avoid to_str().unwrap() by passing Path directly to arg
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
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
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("branch")
            .arg("-D")
            .arg(&branch)
            .output()?;

        if !output.status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "git branch -D failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (_temp_dir, temp_path) = setup_temp_git_repo();

        let loop_id = "test-loop-id-123";
        let manager =
            WorktreeManager::create(&temp_path, loop_id).expect("Failed to create worktree");

        assert_eq!(manager.loop_id, loop_id);
        assert!(manager.path.exists());
        assert!(manager.path.join("dummy.txt").exists());

        manager.remove().expect("Failed to remove worktree");
        assert!(!manager.path.exists());
    }

    #[test]
    fn test_worktree_existing_branch() {
        let (_temp_dir, temp_path) = setup_temp_git_repo();

        let loop_id = "test-loop-existing-branch";
        let branch = format!("rune/loop-{}", loop_id);

        // Pre-create the branch
        let status = Command::new("git")
            .current_dir(&temp_path)
            .args(&["branch", &branch])
            .status()
            .expect("Failed to execute git");
        assert!(status.success());

        // Now create the worktree; it should succeed and reuse the branch
        let manager = WorktreeManager::create(&temp_path, loop_id)
            .expect("Failed to create worktree with existing branch");
        assert!(manager.path.exists());

        manager.remove().expect("Failed to remove worktree");
        assert!(!manager.path.exists());
    }

    #[test]
    fn test_worktree_invalid_repo() {
        let temp_dir = tempfile::tempdir().unwrap();
        let invalid_path = temp_dir.path().to_path_buf(); // Not a git repo

        let loop_id = "test-loop-invalid";
        let res = WorktreeManager::create(&invalid_path, loop_id);
        assert!(res.is_err());
    }
}
