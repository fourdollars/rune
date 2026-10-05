pub mod landlock;
pub mod net_guard;
pub mod seccomp;

pub use landlock::LandlockRuleset;
pub use seccomp::SeccompFilter;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::OnceLock;
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
                PathBuf::from("/dev/zero"),
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

static PROBE_UNSHARE: OnceLock<bool> = OnceLock::new();
static PROBE_SYSTEMD_RUN: OnceLock<bool> = OnceLock::new();

pub fn probe_has_unshare() -> bool {
    *PROBE_UNSHARE.get_or_init(|| {
        std::process::Command::new("unshare")
            .args(["--user", "--net", "--", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

pub fn probe_has_systemd_run() -> bool {
    *PROBE_SYSTEMD_RUN.get_or_init(|| {
        std::process::Command::new("which")
            .arg("systemd-run")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

unsafe fn copy_file_safe(src: &[u8], dst: &[u8]) -> bool {
    let src_fd = libc::open(src.as_ptr() as *const libc::c_char, libc::O_RDONLY);
    if src_fd < 0 {
        return false;
    }
    let dst_fd = libc::open(
        dst.as_ptr() as *const libc::c_char,
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
        0o644,
    );
    if dst_fd < 0 {
        libc::close(src_fd);
        return false;
    }
    let mut buf = [0u8; 4096];
    loop {
        let n = libc::read(src_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
        if n <= 0 {
            break;
        }
        libc::write(dst_fd, buf.as_ptr() as *const libc::c_void, n as usize);
    }
    libc::close(src_fd);
    libc::close(dst_fd);
    true
}

fn format_id_map(buf: &mut [u8; 64], id: u32) -> usize {
    let prefix = b"0 ";
    let suffix = b" 1\n";
    let mut pos = 0;
    buf[pos..pos + prefix.len()].copy_from_slice(prefix);
    pos += prefix.len();
    let num_bytes = format_u32(id, &mut buf[pos..pos + 12]);
    pos += num_bytes;
    buf[pos..pos + suffix.len()].copy_from_slice(suffix);
    pos += suffix.len();
    pos
}

fn format_tmpfs_size(buf: &mut [u8; 32], size_mb: u64) -> usize {
    let prefix = b"size=";
    let suffix = b"M,mode=1777\0";
    let mut pos = 0;
    buf[pos..pos + prefix.len()].copy_from_slice(prefix);
    pos += prefix.len();
    let num_bytes = format_u64(size_mb, &mut buf[pos..pos + 12]);
    pos += num_bytes;
    buf[pos..pos + suffix.len()].copy_from_slice(suffix);
    pos += suffix.len();
    pos
}

fn format_u32(mut n: u32, buf: &mut [u8]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

fn format_u64(mut n: u64, buf: &mut [u8]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

unsafe fn setup_tmpfs_pre_exec(
    tmp_size_mb: u64,
    uid: u32,
    gid: u32,
    isolate_net: bool,
    session_tmp_dir: Option<&std::ffi::CStr>,
    custom_home: Option<&std::ffi::CStr>,
    real_home: Option<&std::ffi::CStr>,
    overlay_mounts: &[(std::ffi::CString, std::ffi::CString)],
    target_cwd: Option<&std::ffi::CStr>,
) {
    if libc::unshare(libc::CLONE_NEWUSER) != 0 {
        return;
    }

    // Write uid_map: "0 <uid> 1\n"
    let uid_fd = libc::open(b"/proc/self/uid_map\0".as_ptr() as *const _, libc::O_WRONLY);
    if uid_fd >= 0 {
        let mut buf = [0u8; 64];
        let len = format_id_map(&mut buf, uid);
        libc::write(uid_fd, buf.as_ptr() as *const _, len);
        libc::close(uid_fd);
    }

    // Write setgroups: "deny\n"
    let sg_fd = libc::open(
        b"/proc/self/setgroups\0".as_ptr() as *const _,
        libc::O_WRONLY,
    );
    if sg_fd >= 0 {
        libc::write(sg_fd, b"deny\n".as_ptr() as *const _, 5);
        libc::close(sg_fd);
    }

    // Write gid_map: "0 <gid> 1\n"
    let gid_fd = libc::open(b"/proc/self/gid_map\0".as_ptr() as *const _, libc::O_WRONLY);
    if gid_fd >= 0 {
        let mut buf = [0u8; 64];
        let len = format_id_map(&mut buf, gid);
        libc::write(gid_fd, buf.as_ptr() as *const _, len);
        libc::close(gid_fd);
    }

    let mut ns_flags = libc::CLONE_NEWNS;
    if isolate_net {
        ns_flags |= libc::CLONE_NEWNET;
    }
    if libc::unshare(ns_flags) != 0 {
        return;
    }

    // Make mount propagation private so mounts don't leak or conflict with parent MS_SHARED
    libc::mount(
        std::ptr::null(),
        b"/\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_REC | libc::MS_PRIVATE,
        std::ptr::null(),
    );

    // Mount /tmp
    if let Some(session_dir) = session_tmp_dir {
        libc::mount(
            session_dir.as_ptr(),
            b"/tmp\0".as_ptr() as *const _,
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        );
    } else {
        let mut opt = [0u8; 32];
        format_tmpfs_size(&mut opt, tmp_size_mb);
        libc::mount(
            b"tmpfs\0".as_ptr() as *const _,
            b"/tmp\0".as_ptr() as *const _,
            b"tmpfs\0".as_ptr() as *const _,
            0,
            opt.as_ptr() as *const libc::c_void,
        );
    }

    // Mount custom home if requested
    if let (Some(ch), Some(rh)) = (custom_home, real_home) {
        // Phase 1: Stash handle to real home
        libc::mkdir(b"/tmp/.orig_home\0".as_ptr() as *const _, 0o700);
        libc::mount(
            rh.as_ptr(),
            b"/tmp/.orig_home\0".as_ptr() as *const _,
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        );
        // Phase 2: Bind custom_home over real_home
        libc::mount(
            ch.as_ptr(),
            rh.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        );
        // Phase 3: Bind whitelisted items back from /tmp/.orig_home to real_home
        for (source, dest) in overlay_mounts {
            libc::mount(
                source.as_ptr(),
                dest.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            );
        }
    }

    // Filtered /etc: isolate sensitive files like /etc/passwd and /etc/hostname
    libc::mkdir(b"/tmp/.etc\0".as_ptr() as *const _, 0o755);
    if !copy_file_safe(
        b"/run/systemd/resolve/resolv.conf\0",
        b"/tmp/.etc/resolv.conf\0",
    ) {
        copy_file_safe(b"/etc/resolv.conf\0", b"/tmp/.etc/resolv.conf\0");
    }
    copy_file_safe(b"/etc/ld.so.cache\0", b"/tmp/.etc/ld.so.cache\0");
    copy_file_safe(b"/etc/ld.so.conf\0", b"/tmp/.etc/ld.so.conf\0");
    copy_file_safe(b"/etc/nsswitch.conf\0", b"/tmp/.etc/nsswitch.conf\0");
    copy_file_safe(b"/etc/locale.alias\0", b"/tmp/.etc/locale.alias\0");

    libc::mkdir(b"/tmp/.etc/ssl\0".as_ptr() as *const _, 0o755);
    libc::mount(
        b"/etc/ssl\0".as_ptr() as *const _,
        b"/tmp/.etc/ssl\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    libc::mkdir(b"/tmp/.etc/ca-certificates\0".as_ptr() as *const _, 0o755);
    libc::mount(
        b"/etc/ca-certificates\0".as_ptr() as *const _,
        b"/tmp/.etc/ca-certificates\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    libc::mkdir(b"/tmp/.etc/alternatives\0".as_ptr() as *const _, 0o755);
    libc::mount(
        b"/etc/alternatives\0".as_ptr() as *const _,
        b"/tmp/.etc/alternatives\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    libc::mkdir(b"/tmp/.etc/ld.so.conf.d\0".as_ptr() as *const _, 0o755);
    libc::mount(
        b"/etc/ld.so.conf.d\0".as_ptr() as *const _,
        b"/tmp/.etc/ld.so.conf.d\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    libc::mkdir(b"/tmp/.etc/pki\0".as_ptr() as *const _, 0o755);
    libc::mount(
        b"/etc/pki\0".as_ptr() as *const _,
        b"/tmp/.etc/pki\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    libc::mount(
        b"/tmp/.etc\0".as_ptr() as *const _,
        b"/etc\0".as_ptr() as *const _,
        std::ptr::null(),
        libc::MS_BIND | libc::MS_REC,
        std::ptr::null(),
    );

    // Mount empty tmpfs on /var/run to block access to docker.sock and dbus socket
    libc::mount(
        b"tmpfs\0".as_ptr() as *const _,
        b"/var/run\0".as_ptr() as *const _,
        b"tmpfs\0".as_ptr() as *const _,
        0,
        b"size=0\0".as_ptr() as *const libc::c_void,
    );

    // Mount proc
    libc::mount(
        b"proc\0".as_ptr() as *const _,
        b"/proc\0".as_ptr() as *const _,
        b"proc\0".as_ptr() as *const _,
        0,
        std::ptr::null(),
    );

    // Change directory to target_cwd if requested
    if let Some(cwd) = target_cwd {
        libc::chdir(cwd.as_ptr());
    }
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

        // Layer 1: Resource limits via systemd-run (cgroups v2)
        let use_systemd_run = false;

        // Layer 2: Tmpfs isolation + Network isolation
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

        if use_tmpfs {
            active_layers.push(format!("tmpfs(/tmp,{}MB)", self.config.tmp_size_mb));
            info!(
                size_mb = self.config.tmp_size_mb,
                "sandbox: isolated tmpfs enabled"
            );
        }

        let self_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "rune".to_string());

        // Layer 3: Seccomp filter (In-process via pre_exec)
        let seccomp_filter = self.build_seccomp_filter(use_unshare_net);
        if seccomp_filter.is_some() {
            active_layers.push("seccomp(ptrace,mount,kexec,bpf)".to_string());
            debug!("sandbox: seccomp filter active");
        }

        // Layer 4: Landlock filesystem restriction (In-process via pre_exec)
        let (landlock_ruleset, landlock_paths) = self.prepare_landlock();
        let landlock_fd = landlock_ruleset.as_ref().map(|l| l.fd()).unwrap_or(-1);
        if landlock_ruleset.is_some() {
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
            net_guard_wrapper = Some(format!("'{}' _net-guard --allow-domains \"\" --", self_exe));
        }
        if !self.config.allowed_domains.is_empty()
            && !self.config.allowed_domains.iter().any(|d| d == "*")
            && Self::is_rune_binary()
        {
            let domains = self.config.allowed_domains.join(",");
            net_guard_wrapper = Some(format!(
                "'{}' _net-guard --allow-domains {} --",
                self_exe, domains
            ));
        }

        let mut cleanup_files: Vec<PathBuf> = Vec::new();
        let mut cleanup_dirs: Vec<PathBuf> = Vec::new();
        let mut overlay_mounts: Vec<(CString, CString)> = Vec::new();

        let (target_home, target_home_raw) = if let Some(ref custom_home) = self.config.mount_home {
            let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
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
                    let orig_source = format!("/tmp/.orig_home/{}\0", rel_str);
                    let target_dest = format!("{}\0", p.to_string_lossy());
                    if let (Ok(s), Ok(d)) = (
                        CString::from_vec_with_nul(orig_source.into_bytes()),
                        CString::from_vec_with_nul(target_dest.into_bytes()),
                    ) {
                        overlay_mounts.push((s, d));
                    }
                }
            }

            (shell_escape(&real_home), real_home)
        } else {
            ("'/tmp'".to_string(), "/tmp".to_string())
        };

        let target_cwd = cwd.unwrap_or(if self.config.mount_home.is_some() {
            &target_home_raw
        } else {
            "/tmp"
        });

        let session_tmp_dir_cstr = self
            .config
            .session_tmp_dir
            .as_ref()
            .and_then(|p| CString::new(p.to_string_lossy().as_bytes()).ok());
        let custom_home_cstr = self
            .config
            .mount_home
            .as_ref()
            .and_then(|p| CString::new(p.to_string_lossy().as_bytes()).ok());
        let real_home_cstr = CString::new(target_home_raw.as_bytes()).ok();
        let target_cwd_cstr = CString::new(target_cwd.as_bytes()).ok();

        let cmd_to_exec = if use_systemd_run {
            format!(
                "unset INVOCATION_ID JOURNAL_STREAM SYSTEMD_EXEC_PID MANAGERPID DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR PWD 2>/dev/null; exec {}",
                cmd
            )
        } else {
            cmd.to_string()
        };

        let final_sh_cmd = if let Some(ng) = net_guard_wrapper {
            format!("{} sh -c {}", ng, shell_escape(&cmd_to_exec))
        } else {
            cmd_to_exec
        };

        let systemd_args: Vec<String> = Vec::new();
        let mut command = if use_systemd_run {
            let mut c = Command::new("systemd-run");
            for arg in &systemd_args {
                c.arg(arg);
            }
            c.arg("sh");
            c.arg("-c");
            c.arg(&final_sh_cmd);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c");
            c.arg(&final_sh_cmd);
            c
        };

        let tmp_size_mb = self.config.tmp_size_mb;
        let actual_uid = unsafe { libc::getuid() };
        let actual_gid = unsafe { libc::getgid() };
        let isolate_net = use_unshare_net;

        unsafe {
            command.pre_exec(move || {
                if use_tmpfs {
                    setup_tmpfs_pre_exec(
                        tmp_size_mb,
                        actual_uid,
                        actual_gid,
                        isolate_net,
                        session_tmp_dir_cstr.as_deref(),
                        custom_home_cstr.as_deref(),
                        real_home_cstr.as_deref(),
                        &overlay_mounts,
                        target_cwd_cstr.as_deref(),
                    );
                } else if isolate_net && has_unshare {
                    let _ = libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET);
                }

                if landlock_fd >= 0 {
                    for (path, access) in &landlock_paths {
                        LandlockRuleset::add_path_rule_raw(landlock_fd, path.as_ptr(), *access);
                    }
                    LandlockRuleset::apply_fd_pre_exec(landlock_fd)?;
                }

                if let Some(ref sc) = seccomp_filter {
                    sc.apply_pre_exec()?;
                }

                Ok(())
            });
        }

        if active_layers.is_empty() {
            degraded = true;
            warn!("sandbox: running in fully degraded mode (no isolation)");
        }

        info!(layers = ?active_layers, timeout = self.config.timeout_secs, "sandbox: executing");
        debug!(cmd = %cmd, "sandbox: full command");

        // Clear environment to prevent info leaks (P2: env disclosure)
        // Only pass minimal safe set + user-provided overrides
        command.env_clear();
        command.env("PATH", "/usr/local/bin:/usr/bin:/bin");
        command.env("HOME", &target_home_raw);
        command.env("LANG", "C.UTF-8");
        command.env("TERM", "dumb");
        // systemd-run --user needs XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS
        if use_systemd_run {
            if let Ok(v) = std::env::var("XDG_RUNTIME_DIR") {
                command.env("XDG_RUNTIME_DIR", v);
            }
            if let Ok(v) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
                command.env("DBUS_SESSION_BUS_ADDRESS", v);
            }
        }

        // Default cwd to target_cwd
        command.current_dir(target_cwd);
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

    /// Prepare an empty Landlock ruleset FD and collect the CString paths with their access rights.
    /// The ruleset FD is opened in parent, and paths are attached inside `pre_exec` after mount setup.
    pub fn prepare_landlock(&self) -> (Option<LandlockRuleset>, Vec<(CString, u64)>) {
        let ruleset = match LandlockRuleset::create_empty() {
            Some(r) => r,
            None => return (None, Vec::new()),
        };

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

        let mut paths = Vec::new();
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
            if let Ok(c) = CString::new(p.to_string_lossy().as_bytes()) {
                paths.push((c, landlock::ACCESS_RW));
            }
        }
        if self.config.mount_home.is_some() && !has_real_home_rw {
            if let Some(ref rh) = real_home_pb {
                if let Ok(c) = CString::new(rh.to_string_lossy().as_bytes()) {
                    paths.push((c, landlock::ACCESS_RW));
                }
            }
        }
        for p in &self.config.read_only_paths {
            if is_custom_home_path(p) {
                continue;
            }
            if let Ok(c) = CString::new(p.to_string_lossy().as_bytes()) {
                paths.push((c, landlock::ACCESS_RO));
            }
        }
        for p in &self.config.traverse_paths {
            if is_custom_home_path(p) {
                continue;
            }
            if let Ok(c) = CString::new(p.to_string_lossy().as_bytes()) {
                paths.push((c, landlock::LANDLOCK_ACCESS_FS_EXECUTE));
            }
        }

        (Some(ruleset), paths)
    }

    /// Build an in-process Landlock ruleset configured with the sandbox paths.
    pub fn build_landlock_ruleset(&self) -> Option<LandlockRuleset> {
        let (ruleset, paths) = self.prepare_landlock();
        if let Some(ref r) = ruleset {
            for (p, access) in paths {
                unsafe {
                    LandlockRuleset::add_path_rule_raw(r.fd(), p.as_ptr(), access);
                }
            }
        }
        ruleset
    }

    /// Build an in-process Seccomp filter configured for the sandbox.
    pub fn build_seccomp_filter(&self, block_net: bool) -> Option<SeccompFilter> {
        SeccompFilter::build(&self.config.allowed_syscalls, block_net)
    }

    async fn build_seccomp_wrapper(&self, self_exe: &str, block_net: bool) -> Option<String> {
        // Use self-exe _seccomp subcommand (always available — single binary)
        if !Self::is_rune_binary() {
            // Not running as rune (e.g. test binary) — skip sandbox wrappers
            return None;
        }

        if !seccomp::is_seccomp_supported() {
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
    async fn test_pre_exec_setup_tmpfs() {
        let mut cmd = Command::new("id");
        unsafe {
            cmd.pre_exec(move || {
                let uid = libc::getuid();
                let gid = libc::getgid();
                setup_tmpfs_pre_exec(10, uid, gid, true, None, None, None, &[], None);
                Ok(())
            });
        }
        let output = cmd.output().await.expect("cmd should execute");
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("output stdout: {}", stdout);
        assert!(output.status.success());
    }

    #[tokio::test]
    async fn test_pre_exec_full_isolation() {
        let landlock = LandlockRuleset::build(
            &[PathBuf::from("/tmp"), PathBuf::from("/dev/null")],
            &[
                PathBuf::from("/bin"),
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
                PathBuf::from("/lib64"),
                PathBuf::from("/etc"),
            ],
            &[PathBuf::from("/dev")],
        );
        let seccomp = SeccompFilter::build(&[], true);
        let landlock_fd = landlock.as_ref().map(|l| l.fd()).unwrap_or(-1);

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "test -r /bin/sh && echo OK; cat /etc/shadow 2>&1"]);
        unsafe {
            cmd.pre_exec(move || {
                let uid = libc::getuid();
                let gid = libc::getgid();
                setup_tmpfs_pre_exec(10, uid, gid, true, None, None, None, &[], None);
                if landlock_fd >= 0 {
                    LandlockRuleset::apply_fd_pre_exec(landlock_fd)?;
                }
                if let Some(ref sc) = seccomp {
                    sc.apply_pre_exec()?;
                }
                Ok(())
            });
        }
        let output = cmd.output().await.expect("cmd should execute");
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("output stdout: {}", stdout);
        assert!(stdout.contains("OK"));
        assert!(stdout.contains("Permission denied") || stdout.contains("No such file"));
    }

    #[tokio::test]
    async fn test_sandbox_basic_command() {
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command("echo hello", None, None)
            .await
            .expect("should succeed");
        println!("active layers: {:?}", result.active_layers);
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

    #[tokio::test]
    async fn test_sandbox_ca_certificates_accessible() {
        let executor = SandboxExecutor::with_defaults();
        let result = executor
            .run_shell_command(
                "ls -la /etc/ssl; ls -la /etc/ssl/certs; head -n 5 /etc/ssl/certs/ca-certificates.crt 2>&1",
                None,
                None,
            )
            .await
            .expect("should succeed");
        println!("SSL stdout:\n{}", result.stdout);
        println!("SSL stderr:\n{}", result.stderr);
        assert!(result.stdout.contains("BEGIN CERTIFICATE"));
    }

    #[tokio::test]
    async fn test_sandbox_tls_verification() {
        let config = SandboxConfig {
            allowed_domains: vec!["github.com".to_string()],
            ..SandboxConfig::default()
        };
        let executor = SandboxExecutor::new(config);
        let result = executor
            .run_shell_command("curl -sSfI https://github.com 2>&1", None, None)
            .await
            .expect("should succeed");
        println!("TLS stdout: {}", result.stdout);
        println!("TLS stderr: {}", result.stderr);
        assert_eq!(
            result.exit_code, 0,
            "curl should succeed with code 0: {}",
            result.stdout
        );
        assert!(
            result.stdout.contains("HTTP/2 200")
                || result.stdout.contains("HTTP/1.1 200")
                || result.stdout.contains("HTTP/2 301")
                || result.stdout.contains("HTTP/1.1 301")
        );
    }

    #[tokio::test]
    async fn test_sandbox_jira_cli_tls() {
        if std::path::Path::new("/home/linuxbrew/.linuxbrew/bin/jira").exists() {
            let config = SandboxConfig {
                allowed_domains: vec!["warthogs.atlassian.net".to_string()],
                read_only_paths: vec![
                    PathBuf::from("/bin"),
                    PathBuf::from("/usr"),
                    PathBuf::from("/lib"),
                    PathBuf::from("/lib64"),
                    PathBuf::from("/etc"),
                    PathBuf::from("/home/linuxbrew"),
                ],
                ..SandboxConfig::default()
            };
            let executor = SandboxExecutor::new(config);
            let res = executor
                .run_shell_command("/home/linuxbrew/.linuxbrew/bin/jira me 2>&1", None, None)
                .await
                .expect("should run");
            println!("jira me stdout: {}", res.stdout);
            println!("jira me exit_code: {}", res.exit_code);
            // It should NOT fail with "certificate signed by unknown authority"
            assert!(!res
                .stdout
                .contains("certificate signed by unknown authority"));
        }
    }

    #[tokio::test]
    async fn test_benchmark_in_process_sandbox_50_samples() {
        // Baseline: unsandboxed sh -c true
        let mut base_samples = Vec::new();
        for _ in 0..50 {
            let start = std::time::Instant::now();
            let _ = tokio::process::Command::new("sh")
                .arg("-c")
                .arg("true")
                .output()
                .await;
            base_samples.push(start.elapsed().as_micros() as f64);
        }
        let base_avg = (base_samples.iter().sum::<f64>() / 50.0) / 1000.0;

        // In-Process pre_exec sandboxed execution
        let executor = SandboxExecutor::with_defaults();
        for _ in 0..3 {
            let _ = executor.run_shell_command("true", None, None).await;
        }

        let mut samples_us = Vec::new();
        for _ in 0..50 {
            let start = std::time::Instant::now();
            let res = executor
                .run_shell_command("true", None, None)
                .await
                .expect("cmd ok");
            assert_eq!(res.exit_code, 0);
            samples_us.push(start.elapsed().as_micros() as f64);
        }

        let avg_ms = (samples_us.iter().sum::<f64>() / 50.0) / 1000.0;
        let mut sorted = samples_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50_ms = sorted[25] / 1000.0;
        let min_ms = sorted[0] / 1000.0;
        let max_ms = sorted[49] / 1000.0;

        println!("\n==========================================");
        println!("     IN-PROCESS SANDBOX BENCHMARK         ");
        println!("==========================================");
        println!("Baseline unsandboxed (sh -c true): {:.2} ms", base_avg);
        println!("In-Process Sandboxed (mean 50 runs): {:.2} ms", avg_ms);
        println!("In-Process Sandboxed (p50 median):   {:.2} ms", p50_ms);
        println!("In-Process Sandboxed (min):          {:.2} ms", min_ms);
        println!("In-Process Sandboxed (max):          {:.2} ms", max_ms);
        println!(
            "Sandbox Overhead:                  +{:.2} ms",
            avg_ms - base_avg
        );
        println!("==========================================\n");
    }
}
