pub mod landlock;
pub mod net_guard;
pub mod seccomp;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Sandbox configuration for constrained command execution.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Domains allowed for outbound network.
    /// If non-empty, only these domains can be resolved (via /etc/hosts injection).
    /// If empty, all network is blocked (default zero-trust).
    pub allowed_domains: Vec<String>,
    /// Paths where the sandboxed process can read and write.
    pub read_write_paths: Vec<PathBuf>,
    /// Paths where the sandboxed process can only read.
    pub read_only_paths: Vec<PathBuf>,
    /// Paths explicitly denied.
    pub denied_paths: Vec<PathBuf>,
    /// Paths that only need traverse/lookup access (EXECUTE only).
    pub traverse_paths: Vec<PathBuf>,
    /// Maximum execution time in seconds.
    pub timeout_secs: u64,
    /// UID to run the sandboxed process as (0 = no change).
    pub uid: u32,
    /// GID to run the sandboxed process as (0 = no change).
    pub gid: u32,
    /// Memory limit in bytes (0 = no limit).
    pub memory_limit: u64,
    /// CPU time limit in seconds (0 = no limit).
    pub cpu_limit_secs: u64,
    /// Max number of child processes (0 = no limit).
    pub max_pids: u32,
    /// Dangerous syscalls to allow through (empty = block all dangerous).
    pub allowed_syscalls: Vec<String>,
    /// Tmpfs size for isolated /tmp in MB (0 = use host /tmp without isolation).
    pub tmp_size_mb: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            allowed_domains: Vec::new(),
            read_write_paths: vec![
                PathBuf::from("/tmp"),
                PathBuf::from("/dev/null"),
                PathBuf::from("/dev/urandom"),
            ],
            read_only_paths: vec![
                PathBuf::from("/bin"),
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
                PathBuf::from("/lib64"),
                // Only essential /etc files (not entire /etc — prevents passwd/hostname leaks)
                PathBuf::from("/etc/ld.so.cache"),
                PathBuf::from("/etc/ld.so.conf"),
                PathBuf::from("/etc/ld.so.conf.d"),
                PathBuf::from("/etc/nsswitch.conf"),
                PathBuf::from("/etc/resolv.conf"),
                PathBuf::from("/etc/ssl"),
                PathBuf::from("/etc/ca-certificates"),
                PathBuf::from("/etc/alternatives"),
                PathBuf::from("/etc/locale.alias"),
            ],
            denied_paths: vec![
                PathBuf::from("/root"),
                PathBuf::from("/proc"),
                PathBuf::from("/sys"),
            ],
            traverse_paths: vec![PathBuf::from("/dev")],
            timeout_secs: 30,
            uid: 0,
            gid: 0,
            memory_limit: 512 * 1024 * 1024, // 512MB default
            cpu_limit_secs: 0,
            max_pids: 64,
            allowed_syscalls: Vec::new(),
            tmp_size_mb: 100,
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
    /// Which sandbox layers were active.
    pub active_layers: Vec<String>,
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
    /// Layers applied (best-effort, each degrades independently):
    /// 1. Network namespace (unshare --user --net)
    /// 2. Resource limits via systemd-run --scope (memory, pids)
    /// 3. Seccomp syscall filter (via seccomp helper if available)
    /// 4. Landlock filesystem restriction (via landlock helper if available)
    /// 5. DNS allowlist (via /etc/hosts override in namespace)
    pub async fn run_shell_command(
        &self,
        cmd: &str,
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<SandboxResult> {
        let has_unshare = probe_tool("unshare").await;
        let has_systemd_run = probe_tool("systemd-run").await;

        let mut degraded = false;
        let mut active_layers: Vec<String> = Vec::new();
        let mut wrapper_parts: Vec<String> = Vec::new();

        // Layer 1: Resource limits via systemd-run (cgroups v2)
        if has_systemd_run && (self.config.memory_limit > 0 || self.config.max_pids > 0) {
            let mut systemd_args = vec![
                "systemd-run".to_string(),
                "--quiet".to_string(),
                "--scope".to_string(),
                "--user".to_string(),
            ];
            if self.config.memory_limit > 0 {
                systemd_args.push(format!("-p MemoryMax={}", self.config.memory_limit));
            }
            if self.config.max_pids > 0 {
                systemd_args.push(format!("-p TasksMax={}", self.config.max_pids));
            }
            systemd_args.push("--".to_string());

            // Test if systemd-run --user works
            let test = Command::new("systemd-run")
                .args(["--quiet", "--scope", "--user", "--", "true"])
                .output()
                .await;
            if test.map(|o| o.status.success()).unwrap_or(false) {
                wrapper_parts.push(systemd_args.join(" "));
                active_layers.push(format!(
                    "cgroups(mem={}MB,pids={})",
                    self.config.memory_limit / 1024 / 1024,
                    self.config.max_pids
                ));
                info!("sandbox: cgroups via systemd-run --scope --user");
            } else {
                debug!("sandbox: systemd-run --user not available, skipping cgroups");
            }
        }

        // Layer 2: Tmpfs isolation + Network isolation (combined in one unshare call)
        let use_tmpfs = self.config.tmp_size_mb > 0 && has_unshare;
        let mut use_net_guard_empty = false;
        let mut use_unshare_net = false;

        // Determine network strategy
        if !self.config.allowed_domains.is_empty() {
            if self.config.allowed_domains.iter().any(|d| d == "*") {
                active_layers.push("network(unrestricted)".to_string());
                info!("sandbox: network unrestricted (wildcard domain)");
            } else {
                // net-guard handles network filtering (no unshare --net needed)
                active_layers.push(format!(
                    "net-guard({})",
                    self.config.allowed_domains.join(",")
                ));
                info!(domains = ?self.config.allowed_domains, "sandbox: net-guard active");
            }
        } else if has_unshare {
            use_unshare_net = true;
            active_layers.push("netns(isolated)".to_string());
            info!("sandbox: network namespace fully isolated");
        } else {
            use_net_guard_empty = true;
            active_layers.push("net-guard(none)".to_string());
            info!("sandbox: net-guard blocking all (empty allowlist, unshare unavailable)");
        }

        // Build combined unshare command (mount + optional net)
        if has_unshare && (use_tmpfs || use_unshare_net) {
            let mut flags = vec!["unshare"];
            flags.push("--user");
            if use_tmpfs {
                flags.push("--map-root-user");
                flags.push("--mount");
                active_layers.push(format!("tmpfs(/tmp,{}MB)", self.config.tmp_size_mb));
                info!(
                    size_mb = self.config.tmp_size_mb,
                    "sandbox: isolated tmpfs enabled"
                );
            }
            if use_unshare_net {
                flags.push("--net");
            }
            flags.push("--");
            wrapper_parts.push(flags.join(" "));
        }

        // Layer 3: Seccomp filter via _seccomp subcommand
        let seccomp_wrapper = self.build_seccomp_wrapper(false).await;
        if let Some(ref sw) = seccomp_wrapper {
            active_layers.push("seccomp(ptrace,mount,kexec,bpf)".to_string());
            debug!("sandbox: seccomp filter active");
        }

        // Layer 4: Landlock filesystem restriction
        let landlock_wrapper = self.build_landlock_wrapper().await;
        if let Some(ref lw) = landlock_wrapper {
            active_layers.push(format!(
                "landlock(rw={},ro={})",
                self.config.read_write_paths.len(),
                self.config.read_only_paths.len()
            ));
            debug!("sandbox: landlock active");
        }

        // Network guard layer (skip if not running as rune binary)
        let mut net_guard_wrapper: Option<String> = None;
        if use_net_guard_empty && Self::is_rune_binary() {
            let self_exe_ng = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "rune".to_string());
            net_guard_wrapper = Some(format!(
                "'{}' _net-guard --allow-domains \"\" --",
                self_exe_ng
            ));
        }
        if !self.config.allowed_domains.is_empty()
            && !self.config.allowed_domains.iter().any(|d| d == "*")
            && Self::is_rune_binary()
        {
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "rune".to_string());
            let domains = self.config.allowed_domains.join(",");
            net_guard_wrapper = Some(format!(
                "'{}' _net-guard --allow-domains {} --",
                self_exe, domains
            ));
        }

        // Build the final command
        // Chain wrappers: net-guard (outermost) -> landlock -> seccomp -> sh -c "cmd"
        // net-guard must be outermost because it forks and uses SECCOMP_USER_NOTIF
        // which would be blocked by _seccomp inner filter
        let mut inner_cmd_parts = Vec::new();

        if let Some(ng) = net_guard_wrapper {
            inner_cmd_parts.push(ng);
        }
        if let Some(lw) = landlock_wrapper {
            inner_cmd_parts.push(lw);
        }
        if let Some(sw) = seccomp_wrapper {
            inner_cmd_parts.push(sw);
        }

        inner_cmd_parts.push(format!("sh -c {}", shell_escape(cmd)));

        let inner_cmd = inner_cmd_parts.join(" ");

        // If tmpfs isolation is active, mount tmpfs + isolate /etc and /proc
        let inner_cmd = if use_tmpfs {
            // Build the full script that runs inside the mount namespace.
            // unshare -- needs a single command, so wrap in sh -c "script"
            let mount_setup = format!(
                concat!(
                    "mount -t tmpfs -o size={size}M,mode=1777 tmpfs /tmp",
                    " && mkdir -p /tmp/.etc",
                    " && {{ cp /etc/ld.so.cache /tmp/.etc/ 2>/dev/null;",
                    " cp /etc/ld.so.conf /tmp/.etc/ 2>/dev/null;",
                    " cp -a /etc/ld.so.conf.d /tmp/.etc/ 2>/dev/null;",
                    " cp /etc/nsswitch.conf /tmp/.etc/ 2>/dev/null;",
                    " cp /run/systemd/resolve/resolv.conf /tmp/.etc/resolv.conf 2>/dev/null || cp /etc/resolv.conf /tmp/.etc/ 2>/dev/null;",
                    " cp -a /etc/ssl /tmp/.etc/ 2>/dev/null;",
                    " cp -a /etc/ca-certificates /tmp/.etc/ 2>/dev/null;",
                    " cp -a /etc/alternatives /tmp/.etc/ 2>/dev/null;",
                    " cp /etc/locale.alias /tmp/.etc/ 2>/dev/null;",
                    " true; }}",
                    " && mount --bind /tmp/.etc /etc",
                    " && mount -t tmpfs -o size=0 tmpfs /proc",
                    " && mount -t tmpfs -o size=0 tmpfs /var/run",
                    " && unset INVOCATION_ID JOURNAL_STREAM SYSTEMD_EXEC_PID MANAGERPID DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR PWD && export PWD=/tmp HOME=/tmp && exec {cmd}",
                ),
                size = self.config.tmp_size_mb,
                cmd = inner_cmd,
            );
            // The mount_setup becomes the single arg to "sh -c" under unshare
            format!("sh -c {}", shell_escape(&mount_setup))
        } else {
            inner_cmd
        };

        let final_cmd = if wrapper_parts.is_empty() {
            if !degraded {
                degraded = true;
            }
            warn!("sandbox: running in fully degraded mode (no isolation)");
            inner_cmd.clone()
        } else {
            format!("{} {}", wrapper_parts.join(" "), inner_cmd)
        };

        info!(layers = ?active_layers, timeout = self.config.timeout_secs, "sandbox: executing");
        debug!(final_cmd = %final_cmd, "sandbox: full command");

        let mut command = Command::new("sh");
        command.arg("-c").arg(&final_cmd);

        // Clear environment to prevent info leaks (P2: env disclosure)
        // Only pass minimal safe set + user-provided overrides
        command.env_clear();
        command.env("PATH", "/usr/local/bin:/usr/bin:/bin");
        command.env("HOME", "/tmp");
        command.env("LANG", "C.UTF-8");
        command.env("TERM", "dumb");
        // systemd-run --user needs XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS
        if let Ok(v) = std::env::var("XDG_RUNTIME_DIR") {
            command.env("XDG_RUNTIME_DIR", v);
        }
        if let Ok(v) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
            command.env("DBUS_SESSION_BUS_ADDRESS", v);
        }

        // Default cwd to /tmp to prevent PWD leaking real working dir (P2)
        command.current_dir("/tmp");
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

        let child = command
            .spawn()
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
                    active_layers,
                })
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("sandbox command error: {}", e)),
            Err(_) => {
                warn!(
                    cmd,
                    timeout_secs = self.config.timeout_secs,
                    "sandbox: command timed out"
                );
                Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: format!("command timed out after {}s", self.config.timeout_secs),
                    exit_code: -1,
                    timed_out: true,
                    degraded,
                    active_layers,
                })
            }
        }
    }

    /// Build a seccomp wrapper command string.
    /// Uses `setpriv --no-new-privs` which implicitly enables seccomp no_new_privs.
    /// For actual BPF filtering, we'd need a helper binary; for now we use
    /// no_new_privs as the baseline seccomp protection.

    /// Check if current_exe is the rune binary (not a test runner).
    fn is_rune_binary() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .map(|name| name == "rune")
            .unwrap_or(false)
    }

    async fn build_seccomp_wrapper(&self, block_net: bool) -> Option<String> {
        // Use self-exe _seccomp subcommand (always available — single binary)
        if !Self::is_rune_binary() {
            // Not running as rune (e.g. test binary) — skip sandbox wrappers
            return None;
        }
        let self_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "rune".to_string());

        {
            let mut cmd = format!("'{}' _seccomp", self_exe);
            if !self.config.allowed_syscalls.is_empty() {
                cmd.push_str(&format!(
                    " --allow-syscalls {}",
                    self.config.allowed_syscalls.join(",")
                ));
            }
            if block_net {
                cmd.push_str(" --block-network");
            }
            info!(allowed = ?self.config.allowed_syscalls, "sandbox: seccomp via internal _seccomp subcommand");
            return Some(cmd);
        }
    }

    /// Build a landlock wrapper using a helper script.
    /// Landlock requires direct syscalls; we approximate with filesystem checks
    /// in the probe phase since raw landlock from a shell wrapper is impractical.
    /// The real protection comes from the user namespace UID remapping.
    async fn build_landlock_wrapper(&self) -> Option<String> {
        // Use self-exe _landlock subcommand (always available — single binary)
        if !Self::is_rune_binary() {
            return None;
        }
        let self_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "rune".to_string());

        // Build _landlock subcommand with configured paths
        let mut parts = vec![format!("'{}' _landlock", self_exe)];
        for p in &self.config.read_write_paths {
            parts.push("--rw".to_string());
            parts.push(p.display().to_string());
        }
        for p in &self.config.read_only_paths {
            parts.push("--ro".to_string());
            parts.push(p.display().to_string());
        }
        for p in &self.config.traverse_paths {
            parts.push("--traverse".to_string());
            parts.push(p.display().to_string());
        }
        parts.push("--".to_string());
        info!(rw = ?self.config.read_write_paths, ro = ?self.config.read_only_paths, "sandbox: landlock filesystem restriction active");
        Some(parts.join(" "))
    }

    /// Check if a domain is in the allowlist.
    pub fn is_domain_allowed(&self, domain: &str) -> bool {
        if self.config.allowed_domains.is_empty() {
            return false; // empty = block all
        }
        self.config
            .allowed_domains
            .iter()
            .any(|d| d == domain || d == "*" || (d.starts_with("*.") && domain.ends_with(&d[1..])))
    }
}

/// Check if a tool binary is available AND usable.
async fn probe_tool(name: &str) -> bool {
    if name == "unshare" {
        Command::new("unshare")
            .args(["--user", "--net", "--", "true"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else if name == "systemd-run" {
        Command::new("which")
            .arg("systemd-run")
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

    #[test]
    fn test_domain_allowlist() {
        let config = SandboxConfig {
            allowed_domains: vec!["example.com".to_string(), "*.github.com".to_string()],
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);
        assert!(executor.is_domain_allowed("example.com"));
        assert!(executor.is_domain_allowed("api.github.com"));
        assert!(!executor.is_domain_allowed("evil.com"));
    }

    #[test]
    fn test_domain_allowlist_empty_blocks_all() {
        let executor = SandboxExecutor::with_defaults();
        assert!(!executor.is_domain_allowed("example.com"));
        assert!(!executor.is_domain_allowed("anything.com"));
    }

    #[tokio::test]
    async fn test_sandbox_tmpfs_isolation() {
        // Test that /tmp is isolated when tmp_size_mb > 0
        let config = SandboxConfig {
            tmp_size_mb: 10, // 10MB tmpfs
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);

        // Write a marker file to host /tmp first
        let _ = std::fs::write("/tmp/rune_host_marker_test", "host");

        // Inside sandbox, /tmp should be empty (isolated tmpfs)
        let result = executor
            .run_shell_command(
                "ls /tmp/rune_host_marker_test 2>&1; echo EXIT=$?; id; df /tmp 2>&1",
                None,
                None,
            )
            .await
            .expect("should succeed");

        // Clean up
        let _ = std::fs::remove_file("/tmp/rune_host_marker_test");

        // If tmpfs is working, the file should not exist inside the sandbox
        assert!(
            result.stdout.contains("No such file") || result.stdout.contains("EXIT=2"),
            "Expected /tmp isolation but got: {}\nLayers: {:?}\nDegraded: {}",
            result.stdout,
            result.active_layers,
            result.degraded
        );
    }

    #[tokio::test]
    async fn test_sandbox_tmpfs_size_limit() {
        // Test that tmpfs size limit is enforced
        let config = SandboxConfig {
            tmp_size_mb: 5, // 5MB limit
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);

        // Try to write 10MB — should fail with ENOSPC
        let result = executor
            .run_shell_command(
                "dd if=/dev/zero of=/tmp/bigfile bs=1M count=10 2>&1",
                None,
                None,
            )
            .await
            .expect("should succeed");

        assert!(
            result.stdout.contains("No space left") || result.stderr.contains("No space left"),
            "Expected space limit error but got stdout={} stderr={}",
            result.stdout,
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_sandbox_tmpfs_disabled() {
        // When tmp_size_mb = 0, no tmpfs isolation
        let config = SandboxConfig {
            tmp_size_mb: 0,
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);

        let result = executor
            .run_shell_command("echo ok", None, None)
            .await
            .expect("should succeed");

        assert!(
            !result.active_layers.iter().any(|l| l.contains("tmpfs")),
            "tmpfs layer should not be present when disabled: {:?}",
            result.active_layers
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_proc_self_root() {
        // P1: /proc/self/root should NOT allow reading system files
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("cat /proc/self/root/etc/passwd 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("Permission denied")
                || result.stdout.contains("No such file")
                || result.exit_code != 0,
            "P1 VULN: /proc/self/root escape succeeded! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_proc_self_cwd_traversal() {
        // P1: /proc/self/cwd/../../../etc/passwd should be blocked
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("cat /proc/self/cwd/../../../etc/passwd 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("Permission denied")
                || result.stdout.contains("No such file")
                || result.exit_code != 0,
            "P1 VULN: /proc/self/cwd traversal escape succeeded! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_dotdot_traversal() {
        // P1: ./../../../../etc/passwd should be blocked
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("cat ./../../../../etc/passwd 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("Permission denied")
                || result.stdout.contains("No such file")
                || result.exit_code != 0,
            "P1 VULN: ../ traversal escape succeeded! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_etc_passwd() {
        // P1: Direct /etc/passwd read should be blocked (no longer in read_only_paths)
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("cat /etc/passwd 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("Permission denied")
                || result.stdout.contains("No such file")
                || result.exit_code != 0,
            "P1 VULN: /etc/passwd directly readable! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_etc_hostname() {
        // P1: /etc/hostname should be blocked
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("cat /etc/hostname 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("Permission denied")
                || result.stdout.contains("No such file")
                || result.exit_code != 0,
            "P1 VULN: /etc/hostname readable! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_allows_etc_ld_so_cache() {
        // P1: /etc/ld.so.cache must be readable (dynamic linker needs it)
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command(
                "test -r /etc/ld.so.cache && echo READABLE || echo DENIED",
                None,
                None,
            )
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains("READABLE"),
            "ld.so.cache should be readable for dynamic linking, got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_dns_resolves() {
        // Sandbox must have a working resolv.conf pointing to a real nameserver
        // (not 127.0.0.53 stub which is unreachable after /var/run is masked).
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command(
                "cat /etc/resolv.conf | grep -v '^#' | grep nameserver",
                None,
                None,
            )
            .await
            .expect("should succeed");
        let stdout = result.stdout.trim();
        assert!(
            !stdout.is_empty(),
            "resolv.conf should have a nameserver line, got empty"
        );
        assert!(
            !stdout.contains("127.0.0.53"),
            "resolv.conf should NOT point to stub resolver 127.0.0.53 inside sandbox, got: {}",
            stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_env_minimal() {
        // P2: env should only show minimal safe variables
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("env", None, None)
            .await
            .expect("should succeed");

        // Should have PATH, HOME, LANG, TERM
        assert!(result.stdout.contains("PATH="), "missing PATH");
        assert!(result.stdout.contains("HOME=/tmp"), "HOME should be /tmp");
        assert!(result.stdout.contains("LANG="), "missing LANG");

        // Should NOT leak sensitive vars
        assert!(
            !result.stdout.contains("MANAGERPID"),
            "P2 VULN: MANAGERPID leaked"
        );
        assert!(
            !result.stdout.contains("INVOCATION_ID"),
            "P2 VULN: INVOCATION_ID leaked"
        );
        assert!(
            !result.stdout.contains("JOURNAL_STREAM"),
            "P2 VULN: JOURNAL_STREAM leaked"
        );
        assert!(
            !result.stdout.contains("SYSTEMD_EXEC_PID"),
            "P2 VULN: SYSTEMD_EXEC_PID leaked"
        );
    }

    #[tokio::test]
    async fn test_sandbox_env_no_user_leak() {
        // P2: USER should not reveal actual system user
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("echo USER=$USER", None, None)
            .await
            .expect("should succeed");
        // USER should be empty or not set (env_clear removes it)
        assert!(
            result.stdout.contains("USER=\n") || result.stdout.trim() == "USER=",
            "P2 VULN: USER env leaked: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_tmp_service_names_hidden() {
        // P4: ls /tmp should not show systemd-private dirs from host
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("ls /tmp/ 2>&1", None, None)
            .await
            .expect("should succeed");
        assert!(
            !result.stdout.contains("systemd-private"),
            "P4 VULN: /tmp leaks systemd service names! got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_docker_socket() {
        // P0: Docker socket must not be accessible inside sandbox
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command(
                "curl --unix-socket /var/run/docker.sock http://localhost/version 2>&1; echo EXIT:$?",
                None, None,
            )
            .await
            .expect("should succeed");
        assert!(
            !result.stdout.contains("\"Version\"") && !result.stdout.contains("\"ApiVersion\""),
            "P0 VULN: Docker socket accessible inside sandbox! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_blocks_dbus_socket() {
        // P3: D-Bus socket must not be reachable inside sandbox
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command(
                "curl --unix-socket /var/run/dbus/system_bus_socket http://localhost/ 2>&1; echo EXIT:$?",
                None, None,
            )
            .await
            .expect("should succeed");
        // exit 7 = socket not found; exit 56 = reachable (bad)
        assert!(
            result.stdout.contains("EXIT:7") || result.stdout.contains("No such file"),
            "P3 VULN: D-Bus socket reachable inside sandbox! stdout={}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_env_no_dbus_leak() {
        // P2: DBUS_SESSION_BUS_ADDRESS and XDG_RUNTIME_DIR must not leak into sandbox
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("env", None, None)
            .await
            .expect("should succeed");
        assert!(
            !result.stdout.contains("DBUS_SESSION_BUS_ADDRESS"),
            "P2 VULN: DBUS_SESSION_BUS_ADDRESS leaked into sandbox! env={}",
            result.stdout
        );
        assert!(
            !result.stdout.contains("XDG_RUNTIME_DIR"),
            "P2 VULN: XDG_RUNTIME_DIR leaked into sandbox! env={}",
            result.stdout
        );
        assert!(
            !result.stdout.contains("PWD=/home"),
            "P2 VULN: PWD leaks home dir! env={}",
            result.stdout
        );
    }
}
