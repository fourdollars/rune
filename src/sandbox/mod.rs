use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{info, warn};

/// Sandbox configuration for constrained command execution.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Domains allowed for outbound network (used with DNS proxy, future).
    pub allowed_domains: Vec<String>,
    /// Paths where the sandboxed process can read and write.
    pub read_write_paths: Vec<PathBuf>,
    /// Paths where the sandboxed process can only read.
    pub read_only_paths: Vec<PathBuf>,
    /// Paths explicitly denied.
    pub denied_paths: Vec<PathBuf>,
    /// Maximum execution time in seconds.
    pub timeout_secs: u64,
    /// UID to run the sandboxed process as (0 = no change).
    pub uid: u32,
    /// GID to run the sandboxed process as (0 = no change).
    pub gid: u32,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            allowed_domains: Vec::new(),
            read_write_paths: vec![PathBuf::from("/tmp")],
            read_only_paths: vec![
                PathBuf::from("/bin"),
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
            ],
            denied_paths: vec![
                PathBuf::from("/root"),
                PathBuf::from("/etc/shadow"),
            ],
            timeout_secs: 30,
            uid: 0,
            gid: 0,
        }
    }
}

/// Result of a sandboxed execution.
#[derive(Debug)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    /// Whether full sandbox was applied or we fell back to basic execution.
    pub degraded: bool,
}

/// Executor that wraps shell commands with best-effort Linux isolation.
pub struct SandboxExecutor {
    config: SandboxConfig,
}

impl SandboxExecutor {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Create with default (minimal) sandbox config.
    pub fn with_defaults() -> Self {
        Self::new(SandboxConfig::default())
    }

    /// Execute a shell command with best-effort sandboxing.
    ///
    /// Isolation strategy (best-effort, degrades gracefully):
    /// 1. Try `unshare -n` for network namespace isolation
    /// 2. Try `setpriv --reuid --regid` for identity downgrade
    /// 3. If tools are unavailable or fail, run without isolation but log warnings
    pub async fn run_shell_command(
        &self,
        cmd: &str,
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<SandboxResult> {
        // Probe available isolation tools
        let has_unshare = probe_tool("unshare").await;
        let has_setpriv = probe_tool("setpriv").await;

        let mut degraded = false;
        let mut wrapper_parts: Vec<String> = Vec::new();

        // Network namespace isolation
        if has_unshare {
            wrapper_parts.push("unshare -n --".to_string());
            info!("sandbox: network namespace via unshare");
        } else {
            warn!("sandbox: unshare not available, skipping network isolation");
            degraded = true;
        }

        // Identity downgrade (only if uid != 0 requested)
        if self.config.uid != 0 && has_setpriv {
            wrapper_parts.push(format!(
                "setpriv --reuid={} --regid={} --clear-groups --",
                self.config.uid, self.config.gid
            ));
            info!(uid = self.config.uid, gid = self.config.gid, "sandbox: identity downgrade via setpriv");
        } else if self.config.uid != 0 {
            warn!("sandbox: setpriv not available, skipping identity downgrade");
            degraded = true;
        }

        // Build the final command
        // If we have wrapper parts, chain them; otherwise just run the raw command
        let final_cmd = if wrapper_parts.is_empty() {
            if !degraded {
                degraded = true;
            }
            warn!("sandbox: running in fully degraded mode (no isolation)");
            cmd.to_string()
        } else {
            // Nest: unshare -n -- setpriv ... -- sh -c "user_cmd"
            let mut full = wrapper_parts.join(" ");
            full.push_str(&format!(" sh -c {}", shell_escape(cmd)));
            full
        };

        info!(final_cmd = %final_cmd, timeout = self.config.timeout_secs, "sandbox: executing");

        let mut command = Command::new("sh");
        command.arg("-c").arg(&final_cmd);

        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        if let Some(envs) = env {
            for (k, v) in envs {
                command.env(k, v);
            }
        }

        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let child = command.spawn()
            .context("failed to spawn sandboxed command")?;

        let timeout = std::time::Duration::from_secs(self.config.timeout_secs);
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);

                if !stderr.is_empty() {
                    let preview: String = stderr.chars().take(500).collect();
                    warn!(exit_code, "sandbox stderr: {}", preview);
                }

                Ok(SandboxResult {
                    stdout,
                    stderr,
                    exit_code,
                    timed_out: false,
                    degraded,
                })
            }
            Ok(Err(e)) => {
                Err(anyhow::anyhow!("sandbox command error: {}", e))
            }
            Err(_) => {
                warn!(cmd, timeout_secs = self.config.timeout_secs, "sandbox: command timed out");
                Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: format!("command timed out after {}s", self.config.timeout_secs),
                    exit_code: -1,
                    timed_out: true,
                    degraded,
                })
            }
        }
    }
}

/// Check if a tool binary is available AND usable.
/// For `unshare`, we test with a real invocation since it may need privileges.
async fn probe_tool(name: &str) -> bool {
    if name == "unshare" {
        // Test actual capability, not just existence
        Command::new("unshare")
            .args(["-n", "--", "true"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        Command::new("which")
            .arg(name)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Simple shell escaping for wrapping a command string.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Landlock / Seccomp placeholder ──────────────────────────────
//
// Future phases will add:
// - Landlock ABI v1+ filesystem restrictions (requires kernel >= 5.13)
// - Seccomp BPF syscall filtering (ptrace, unshare, mount, kexec_load, bpf, setns)
// - cgroups resource limits (CPU, memory, pids)
//
// These are intentionally left as config fields in SandboxConfig
// so the data model is ready when implementation lands.

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sandbox_basic_command() {
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("echo hello", None, None)
            .await
            .expect("should succeed");
        assert!(result.stdout.trim().contains("hello"));
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_sandbox_timeout() {
        let config = SandboxConfig {
            timeout_secs: 1,
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);
        let result = executor
            .run_shell_command("sleep 10", None, None)
            .await
            .expect("should return timeout result");
        assert!(result.timed_out);
    }

    #[tokio::test]
    async fn test_sandbox_nonzero_exit() {
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("exit 42", None, None)
            .await
            .expect("should succeed");
        assert_eq!(result.exit_code, 42);
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }
}
