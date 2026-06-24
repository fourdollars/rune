use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WorktreeManager {
    pub repo_path: PathBuf,
    pub loop_id: String,
    pub path: PathBuf,
}

impl WorktreeManager {
    pub fn get_branch_name(loop_id: &str) -> String {
        if loop_id.starts_with("loop-") || loop_id.starts_with("goal-") {
            format!("rune/{}", loop_id)
        } else {
            format!("rune/loop-{}", loop_id)
        }
    }

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
        let branch = Self::get_branch_name(loop_id);

        if path.exists() {
            return Ok(Self {
                repo_path,
                loop_id: loop_id.to_string(),
                path,
            });
        }

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
        let worktree_result = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
            .output();

        let worktree_failed = match &worktree_result {
            Ok(output) => !output.status.success(),
            Err(_) => true,
        };

        if worktree_failed {
            // Attempt to clean up git's worktree metadata directory to allow branch deletion
            if let Ok(output) = Command::new("git")
                .args(&["rev-parse", "--git-common-dir"])
                .current_dir(&self.repo_path)
                .output()
            {
                if output.status.success() {
                    let git_common_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let git_common_path = PathBuf::from(git_common_str);
                    let abs_git_common = if git_common_path.is_absolute() {
                        git_common_path
                    } else {
                        self.repo_path.join(git_common_path)
                    };
                    if let Ok(abs_git_common) = std::fs::canonicalize(abs_git_common) {
                        let meta_path = abs_git_common.join("worktrees").join(&self.loop_id);
                        let _ = std::fs::remove_dir_all(meta_path);
                    }
                }
            }
            if self.path.exists() {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        // Clean up the branch after worktree is removed to avoid dangling branches
        let branch = Self::get_branch_name(&self.loop_id);
        let branch_result = Command::new("git")
            .current_dir(&self.repo_path)
            .arg("branch")
            .arg("-D")
            .arg(&branch)
            .output();

        let worktree_err = match worktree_result {
            Ok(output) => {
                if !output.status.success() {
                    Some(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!(
                            "git worktree remove failed: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        ),
                    ))
                } else {
                    None
                }
            }
            Err(e) => Some(e),
        };

        let branch_err = match branch_result {
            Ok(output) => {
                if !output.status.success() {
                    Some(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!(
                            "git branch -D failed: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        ),
                    ))
                } else {
                    None
                }
            }
            Err(e) => Some(e),
        };

        if let Some(err) = worktree_err {
            Err(err)
        } else if let Some(err) = branch_err {
            Err(err)
        } else {
            Ok(())
        }
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

        // Try creating it again while it already exists - should return Ok and not fail
        let manager2 = WorktreeManager::create(&temp_path, loop_id)
            .expect("Failed to recreate existing worktree");
        assert_eq!(manager2.loop_id, loop_id);
        assert!(manager2.path.exists());

        manager.remove().expect("Failed to remove worktree");
        assert!(!manager.path.exists());
    }

    #[test]
    fn test_worktree_existing_branch() {
        let (_temp_dir, temp_path) = setup_temp_git_repo();

        let loop_id = "test-loop-existing-branch";
        let branch = WorktreeManager::get_branch_name(loop_id);

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

    #[test]
    fn test_remove_robust_cleanup() {
        let (_temp_dir, temp_path) = setup_temp_git_repo();

        let loop_id = "test-loop-robust";
        let mut manager =
            WorktreeManager::create(&temp_path, loop_id).expect("Failed to create worktree");

        // Now, manually change manager's path to something that isn't a worktree
        manager.path = temp_path.join("non-existent-worktree-path");

        // The branch should still exist right now
        let branch = WorktreeManager::get_branch_name(loop_id);
        let branch_ref = format!("refs/heads/{}", branch);
        let check_branch = || {
            Command::new("git")
                .current_dir(&temp_path)
                .args(&["show-ref", "--verify", &branch_ref])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        };
        assert!(check_branch(), "Branch should exist before remove");

        // Call remove. It should return Err because git worktree remove fails,
        // but it should still attempt to delete the branch.
        let res = manager.remove();
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("git worktree remove failed"),
            "Expected worktree remove to fail"
        );

        // Verify that the branch was still deleted!
        assert!(
            !check_branch(),
            "Branch should have been deleted despite worktree remove failure"
        );
    }

    #[test]
    fn test_get_branch_name() {
        assert_eq!(
            WorktreeManager::get_branch_name("loop-1234"),
            "rune/loop-1234"
        );
        assert_eq!(
            WorktreeManager::get_branch_name("goal-5678"),
            "rune/goal-5678"
        );
        assert_eq!(
            WorktreeManager::get_branch_name("my-note"),
            "rune/loop-my-note"
        );
    }
}
