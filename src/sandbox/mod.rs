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
    /// Session-scoped temporary directory to bind-mount over /tmp (None = use isolated per-invocation tmpfs).
    pub session_tmp_dir: Option<PathBuf>,
    /// Custom directory to bind-mount over real HOME in sandbox.
    pub mount_home: Option<PathBuf>,
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
            read_only_paths: ["/bin", "/usr", "/lib", "/lib64", "/etc"]
                .iter()
                // Allow /etc read access for DNS resolution and dynamic linking.
                // Security: sensitive files (/etc/passwd, /etc/hostname) are NOT exposed
                // because sandbox mounts /tmp/.etc over /etc — only files explicitly
                // copied into /tmp/.etc are visible (resolv.conf, nsswitch.conf,
                // ld.so.cache, ssl certs, etc).
                .filter(|p| std::path::Path::new(p).exists())
                .map(PathBuf::from)
                .collect(),
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
            session_tmp_dir: None,
            mount_home: None,
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

    /// Access sandbox config.
    pub fn config(&self) -> &SandboxConfig {
        &self.config
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
                flags.push("--pid");
                flags.push("--fork");
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

        let (mount_exe_cmd, rune_exe_inside) = if use_tmpfs && Self::is_rune_binary() {
            if let Ok(exe_path) = std::env::current_exe() {
                let escaped_exe = shell_escape(&exe_path.to_string_lossy());
                (
                    format!(" && {{ touch /tmp/.rune-exe 2>/dev/null || true; mount --bind {} /tmp/.rune-exe 2>/dev/null || true; }}", escaped_exe),
                    "/tmp/.rune-exe".to_string(),
                )
            } else {
                (String::new(), "rune".to_string())
            }
        } else {
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "rune".to_string());
            (String::new(), self_exe)
        };

        // Layer 3: Seccomp filter via _seccomp subcommand
        let seccomp_wrapper = self.build_seccomp_wrapper(&rune_exe_inside, false).await;
        if let Some(ref sw) = seccomp_wrapper {
            active_layers.push("seccomp(ptrace,mount,kexec,bpf)".to_string());
            debug!("sandbox: seccomp filter active");
        }

        // Layer 4: Landlock filesystem restriction
        let landlock_wrapper = self.build_landlock_wrapper(&rune_exe_inside).await;
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
            net_guard_wrapper = Some(format!(
                "'{}' _net-guard --allow-domains \"\" --",
                rune_exe_inside
            ));
        }
        if !self.config.allowed_domains.is_empty()
            && !self.config.allowed_domains.iter().any(|d| d == "*")
            && Self::is_rune_binary()
        {
            let domains = self.config.allowed_domains.join(",");
            net_guard_wrapper = Some(format!(
                "'{}' _net-guard --allow-domains {} --",
                rune_exe_inside, domains
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

        let mut cleanup_files: Vec<PathBuf> = Vec::new();
        let mut cleanup_dirs: Vec<PathBuf> = Vec::new();

        let (mount_home_cmd, target_home, target_home_raw) = if let Some(ref custom_home) =
            self.config.mount_home
        {
            let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            let escaped_real_home = shell_escape(&real_home);
            let escaped_custom_home = shell_escape(&custom_home.to_string_lossy());

            // Phase 1: Stash a handle to the real original HOME, then bind custom_home over real_home
            let mut cmd = format!(
                " && {{ mkdir -p /tmp/.orig_home 2>/dev/null || true; mount --bind {} /tmp/.orig_home 2>/dev/null || true; }}",
                escaped_real_home
            );
            cmd.push_str(&format!(
                " && {{ mkdir -p {} 2>/dev/null || true; mount --bind {} {} 2>/dev/null || true; }}",
                escaped_real_home, escaped_custom_home, escaped_real_home
            ));

            // Phase 2: Overlay whitelist items (from rune.toml, skills, and -M/-m) that reside under real_home
            let real_home_path = std::path::Path::new(&real_home);
            let custom_home_canon =
                std::fs::canonicalize(custom_home).unwrap_or_else(|_| custom_home.clone());
            let mut overlay_paths: Vec<PathBuf> = Vec::new();
            for p in self
                .config
                .read_write_paths
                .iter()
                .chain(self.config.read_only_paths.iter())
            {
                let p_canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                // Skip custom_home itself to prevent recursive self-mounts like notes-home/notes-home
                if p.starts_with(real_home_path)
                    && p_canon != custom_home_canon
                    && !overlay_paths.contains(p)
                {
                    overlay_paths.push(p.clone());
                }
            }

            for p in &overlay_paths {
                if let Ok(rel) = p.strip_prefix(real_home_path) {
                    if rel.as_os_str().is_empty() {
                        continue;
                    }

                    let target_in_custom = custom_home.join(rel);
                    if !target_in_custom.exists() {
                        if p.is_dir() {
                            cleanup_dirs.push(target_in_custom.clone());
                        } else {
                            cleanup_files.push(target_in_custom.clone());
                        }
                        let mut parent = target_in_custom.parent();
                        while let Some(pr) = parent {
                            if pr == custom_home || !pr.starts_with(custom_home) {
                                break;
                            }
                            if !pr.exists() && !cleanup_dirs.contains(&pr.to_path_buf()) {
                                cleanup_dirs.push(pr.to_path_buf());
                            }
                            parent = pr.parent();
                        }
                    }

                    let rel_str = rel.to_string_lossy();
                    let orig_source = format!("/tmp/.orig_home/{}", rel_str);
                    let target_dest = p.to_string_lossy().to_string();
                    let esc_source = shell_escape(&orig_source);
                    let esc_dest = shell_escape(&target_dest);

                    if p.is_dir() {
                        cmd.push_str(&format!(
                            " && if [ -d {} ]; then mkdir -p {} 2>/dev/null && mount --bind {} {} 2>/dev/null || true; fi",
                            esc_source, esc_dest, esc_source, esc_dest
                        ));
                    } else {
                        cmd.push_str(&format!(
                            " && if [ -f {} ]; then mkdir -p $(dirname {}) 2>/dev/null && touch {} 2>/dev/null && mount --bind {} {} 2>/dev/null || true; fi",
                            esc_source, esc_dest, esc_dest, esc_source, esc_dest
                        ));
                    }
                }
            }

            (cmd, escaped_real_home, real_home)
        } else {
            (String::new(), "'/tmp'".to_string(), "/tmp".to_string())
        };

        let target_cwd = cwd.unwrap_or(if self.config.mount_home.is_some() {
            &target_home_raw
        } else {
            "/tmp"
        });
        let escaped_target_cwd = shell_escape(target_cwd);

        let mount_tmp = if let Some(ref session_dir) = self.config.session_tmp_dir {
            let escaped_session_dir = shell_escape(&session_dir.to_string_lossy());
            format!(
                "mkdir -p {} && mount --bind {} /tmp",
                escaped_session_dir, escaped_session_dir
            )
        } else {
            format!(
                "mount -t tmpfs -o size={}M,mode=1777 tmpfs /tmp",
                self.config.tmp_size_mb
            )
        };

        // If tmpfs isolation is active, mount tmpfs/session_dir + isolate /etc and /proc
        let inner_cmd = if use_tmpfs {
            // Build the full script that runs inside the mount namespace.
            // unshare -- needs a single command, so wrap in sh -c "script"
            let mount_setup = format!(
                concat!(
                    "{mount_tmp}",
                    "{mount_exe_cmd}",
                    "{mount_home_cmd}",
                    // cd re-resolves CWD through the new mount so that Landlock's
                    // inode-based rule matches the process's CWD inode.
                    " && cd {target_cwd}",
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
                    " && mount -t proc proc /proc",
                    " && mount -t tmpfs -o size=0 tmpfs /var/run",
                    " && unset INVOCATION_ID JOURNAL_STREAM SYSTEMD_EXEC_PID MANAGERPID DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR PWD && export PWD={target_cwd} HOME={target_home} && exec {cmd}",
                ),
                mount_tmp = mount_tmp,
                mount_exe_cmd = mount_exe_cmd,
                mount_home_cmd = mount_home_cmd,
                target_cwd = escaped_target_cwd,
                target_home = target_home,
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
        command.env("HOME", &target_home_raw);
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
            if std::path::Path::new(dir).is_dir() {
                command.current_dir(dir);
            }
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

        // Clean up temporary mountpoint files and empty placeholder directories created in custom_home
        for f in cleanup_files {
            if f.is_file() {
                let _ = std::fs::remove_file(&f);
            }
        }
        cleanup_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for d in cleanup_dirs {
            if d.is_dir() {
                let _ = std::fs::remove_dir(&d); // only removes if empty
            }
        }

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

    /// Check if current_exe is the rune binary (not a test runner).
    fn is_rune_binary() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .map(|name| name == "rune")
            .unwrap_or(false)
    }

    async fn build_seccomp_wrapper(&self, self_exe: &str, block_net: bool) -> Option<String> {
        // Use self-exe _seccomp subcommand (always available — single binary)
        if !Self::is_rune_binary() {
            // Not running as rune (e.g. test binary) — skip sandbox wrappers
            return None;
        }

        // Probe whether prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER) is permitted.
        // Some container runtimes (e.g. ChromeOS Crostini / LXC) block this via
        // SECCOMP_RET_TRAP, which kills the child with SIGTRAP (exit 133) before
        // any useful work happens.  Run a trivial allow-all filter test first; if it
        // fails we degrade gracefully rather than crashing every tool invocation.
        let probe_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "rune".to_string());
        let seccomp_available = Command::new(&probe_exe)
            .args(["_seccomp", "--allow-syscalls", "*", "true"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !seccomp_available {
            warn!(
                "sandbox: prctl(PR_SET_SECCOMP) not available on this host, skipping seccomp layer"
            );
            return None;
        }

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
        Some(cmd)
    }

    /// Build the landlock argument list with configured paths.
    pub fn build_landlock_args(&self, self_exe: &str) -> Vec<String> {
        let custom_home_canon = self
            .config
            .mount_home
            .as_ref()
            .map(|h| std::fs::canonicalize(h).unwrap_or_else(|_| h.clone()));
        let real_home_pb = std::env::var("HOME").ok().map(PathBuf::from);

        let is_custom_home_path = |p: &PathBuf| -> bool {
            if let Some(ref ch) = custom_home_canon {
                let p_canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                p == ch || &p_canon == ch || p.starts_with(ch) || p_canon.starts_with(ch)
            } else {
                false
            }
        };

        // Build _landlock subcommand with configured paths
        let mut parts = vec![format!("'{}' _landlock", self_exe)];
        let mut has_real_home_rw = false;
        for p in &self.config.read_write_paths {
            if is_custom_home_path(p) {
                continue;
            }
            if let Some(ref rh) = real_home_pb {
                if p == rh {
                    has_real_home_rw = true;
                }
            }
            parts.push("--rw".to_string());
            parts.push(p.display().to_string());
        }
        if self.config.mount_home.is_some() && !has_real_home_rw {
            if let Some(ref rh) = real_home_pb {
                parts.push("--rw".to_string());
                parts.push(rh.display().to_string());
            }
        }
        for p in &self.config.read_only_paths {
            if is_custom_home_path(p) {
                continue;
            }
            parts.push("--ro".to_string());
            parts.push(p.display().to_string());
        }
        for p in &self.config.traverse_paths {
            if is_custom_home_path(p) {
                continue;
            }
            parts.push("--traverse".to_string());
            parts.push(p.display().to_string());
        }
        parts.push("--".to_string());
        parts
    }

    /// Build a landlock wrapper using a helper script.
    /// Landlock requires direct syscalls; we approximate with filesystem checks
    /// in the probe phase since raw landlock from a shell wrapper is impractical.
    /// The real protection comes from the user namespace UID remapping.
    async fn build_landlock_wrapper(&self, self_exe: &str) -> Option<String> {
        // Use self-exe _landlock subcommand (always available — single binary)
        if !Self::is_rune_binary() {
            return None;
        }

        let parts = self.build_landlock_args(self_exe);
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
    async fn test_sandbox_custom_cwd_with_tmpfs() {
        let cwd = std::env::current_dir().unwrap();
        let cwd_str = cwd.to_string_lossy();
        let config = SandboxConfig {
            tmp_size_mb: 10,
            read_write_paths: vec![PathBuf::from("/tmp"), cwd.clone()],
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);

        let result = executor
            .run_shell_command("pwd", Some(&cwd_str), None)
            .await
            .expect("should succeed");

        assert!(
            result.stdout.trim().contains(&*cwd_str),
            "Expected pwd to match custom CWD {} but got: {}",
            cwd_str,
            result.stdout
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

    #[tokio::test]
    async fn test_sandbox_mount_home() {
        let custom_home = tempfile::tempdir_in("/var/tmp").expect("tempdir in var tmp");
        let mut config = SandboxConfig::default();
        config.mount_home = Some(custom_home.path().to_path_buf());
        config
            .read_write_paths
            .push(custom_home.path().to_path_buf());
        let executor = SandboxExecutor::new(config);
        let result = executor
            .run_shell_command("echo $HOME", None, None)
            .await
            .expect("should succeed");
        let expected_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        assert!(
            result.stdout.contains(&expected_home),
            "HOME should match real home path when mount_home is set, got: stdout={:?} stderr={:?}",
            result.stdout,
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_sandbox_overlay_mounts_with_mount_home() {
        let custom_home = tempfile::tempdir_in("/var/tmp").expect("tempdir custom home");
        let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let real_home_path = PathBuf::from(&real_home);

        // Create a test file in real home (or test directory under HOME if HOME exists)
        if !real_home_path.exists() {
            return;
        }

        let mut config = SandboxConfig::default();
        config.mount_home = Some(custom_home.path().to_path_buf());
        config
            .read_write_paths
            .push(custom_home.path().to_path_buf());

        // Target an existing file/directory in HOME (e.g. .cargo or .gitconfig or .bashrc if exists)
        let sample_file = real_home_path.join(".bashrc");
        if sample_file.exists() {
            config.read_only_paths.push(sample_file.clone());
        }

        let executor = SandboxExecutor::new(config);
        let result = executor
            .run_shell_command("pwd", None, None)
            .await
            .expect("should succeed");
        assert!(
            result.stdout.contains(&real_home),
            "PWD should match real home path when mount_home is set"
        );
    }

    #[tokio::test]
    async fn test_sandbox_mount_home_leaves_no_artifacts() {
        let custom_home = tempfile::tempdir_in("/var/tmp").expect("tempdir custom home");
        let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let real_home_path = PathBuf::from(&real_home);

        if !real_home_path.exists() {
            return;
        }

        let mut config = SandboxConfig::default();
        config.mount_home = Some(custom_home.path().to_path_buf());
        config
            .read_write_paths
            .push(custom_home.path().to_path_buf());

        // Add dummy paths under HOME that don't exist in custom_home
        let sample_file = real_home_path.join(".bashrc");
        if sample_file.exists() {
            config.read_only_paths.push(sample_file.clone());
        }

        let executor = SandboxExecutor::new(config);
        let result = executor
            .run_shell_command("echo hello", None, None)
            .await
            .expect("should succeed");
        assert_eq!(result.exit_code, 0);

        // Verify custom_home is completely clean
        let entries: Vec<_> = std::fs::read_dir(custom_home.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "custom_home should not contain any leftover placeholder files/dirs, found: {:?}",
            entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn test_sandbox_landlock_filters_custom_home_path() {
        let custom_home = tempfile::tempdir_in("/var/tmp").expect("tempdir custom home");
        let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let mut config = SandboxConfig::default();
        config.mount_home = Some(custom_home.path().to_path_buf());
        config
            .read_write_paths
            .push(custom_home.path().to_path_buf());
        config
            .read_only_paths
            .push(custom_home.path().to_path_buf());

        let executor = SandboxExecutor::new(config);
        let args = executor.build_landlock_args("/usr/local/bin/rune");
        let w = args.join(" ");
        let ch_str = custom_home.path().to_string_lossy();

        // Host path of custom_home should not appear in --rw or --ro
        assert!(
            !w.contains(&format!("--rw {}", ch_str)),
            "landlock args should not contain host path of custom_home in --rw, got: {}",
            w
        );
        assert!(
            !w.contains(&format!("--ro {}", ch_str)),
            "landlock args should not contain host path of custom_home in --ro, got: {}",
            w
        );
        // Real home should be included in --rw
        assert!(
            w.contains(&format!("--rw {}", real_home)),
            "landlock args should contain real home in --rw when mount_home is set, got: {}",
            w
        );
    }
}
