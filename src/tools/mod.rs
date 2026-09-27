use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::info;

use crate::sandbox::{SandboxConfig, SandboxExecutor};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Session-scoped temporary directory manager for sandbox /tmp sharing.
pub struct SessionTmp {
    path: PathBuf,
}

impl SessionTmp {
    pub fn new() -> Self {
        let count = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("rune-session-{}-{}", std::process::id(), count);
        let base_dir = if std::path::Path::new("/var/tmp").exists() {
            PathBuf::from("/var/tmp/rune-sessions")
        } else {
            std::env::temp_dir()
        };
        let path = base_dir.join(id);
        let _ = std::fs::create_dir_all(&path);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SessionTmp {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

const MAX_FILE_SIZE: usize = 32 * 1024; // 32KB

/// Tool execution result.
#[derive(Debug, Serialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Diagnostics about which sandbox layers were active (if available).
    pub active_layers: Option<Vec<String>>,
    /// Whether the sandbox fell back to degraded mode (if available).
    pub degraded: Option<bool>,
}

impl ToolOutput {
    fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            active_layers: None,
            degraded: None,
        }
    }
    fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            active_layers: None,
            degraded: None,
        }
    }
    fn with_sandbox(mut self, layers: Vec<String>, degraded: bool) -> Self {
        self.active_layers = Some(layers);
        self.degraded = Some(degraded);
        self
    }
}

/// Tool registry — all tools execute through the sandbox.
pub struct ToolRegistry {
    serve_mode: bool,
    agent_skills: bool,
    policy_mode: String,
    policy_allowed_tools: Vec<String>,
    fetch_max_size_kb: Option<usize>,
    policy_allowed_commands: Vec<String>,
    policy_allowed_syscalls: Vec<String>,
    policy_denied_paths: Vec<String>,
    policy_allowed_paths_rw: Vec<String>,
    policy_allowed_paths_ro: Vec<String>,
    policy_allowed_files_ro: Vec<String>,
    policy_allowed_files_rw: Vec<String>,
    allowed_dirs: Vec<PathBuf>,
    allowed_domains: Vec<String>,
    tmp_size_mb: u64,
    mount_pwd: bool,
    policy_mount_home: Option<String>,
    session_tmp: Option<Arc<SessionTmp>>,
}

impl ToolRegistry {
    pub fn new(allowed_dirs: Vec<PathBuf>) -> Self {
        Self {
            serve_mode: false,
            agent_skills: false,
            allowed_dirs,
            allowed_domains: Vec::new(),
            policy_mode: "allowlist".to_string(),
            policy_allowed_tools: Vec::new(),
            fetch_max_size_kb: None,
            policy_allowed_commands: Vec::new(),
            policy_allowed_syscalls: Vec::new(),
            policy_denied_paths: Vec::new(),
            policy_allowed_paths_rw: Vec::new(),
            policy_allowed_paths_ro: Vec::new(),
            policy_allowed_files_ro: Vec::new(),
            policy_allowed_files_rw: Vec::new(),
            tmp_size_mb: 100,
            mount_pwd: false,
            policy_mount_home: None,
            session_tmp: Some(Arc::new(SessionTmp::new())),
        }
    }

    /// Enable serve-mode tools (search_chat, list/read/write_markdown).
    pub fn set_serve_mode(&mut self, enabled: bool) {
        self.serve_mode = enabled;
    }

    /// Check if serve mode is enabled.
    pub fn is_serve_mode(&self) -> bool {
        self.serve_mode
    }

    /// Enable general agent tools and skills in serve mode.
    pub fn set_agent_skills(&mut self, enabled: bool) {
        self.agent_skills = enabled;
    }

    /// Check if agent_skills is enabled in serve mode.
    pub fn agent_skills(&self) -> bool {
        self.agent_skills
    }

    /// Set allowed tools list.
    pub fn set_allowed_tools(&mut self, tools: Vec<String>) {
        self.policy_allowed_tools = tools;
    }

    /// Check if a tool is permitted to be defined or executed.
    pub fn is_tool_allowed(&self, name: &str) -> bool {
        if self.policy_mode == "unrestricted" {
            return true;
        }
        if self.serve_mode {
            match name {
                "search_chat" | "list_markdown" | "read_markdown" | "write_markdown" => {
                    return true
                }
                _ => {}
            }
        }
        self.policy_allowed_tools
            .iter()
            .any(|t| t == "*" || t == name)
    }

    /// Set allowed network domains (for fetch_url / execute_cmd network access).
    pub fn set_allowed_domains(&mut self, domains: Vec<String>) {
        self.allowed_domains = domains;
    }

    /// Add a single domain to the runtime allowlist.
    pub fn add_allowed_domain(&mut self, domain: &str) {
        if !self.allowed_domains.iter().any(|d| d == domain) {
            self.allowed_domains.push(domain.to_string());
        }
    }

    /// Check if a domain is allowed under the current policy allowlist.
    pub fn is_domain_allowed(&self, domain: &str) -> bool {
        if self.allowed_domains.is_empty() {
            return false;
        }
        self.allowed_domains
            .iter()
            .any(|d| d == domain || d == "*" || (d.starts_with("*.") && domain.ends_with(&d[1..])))
    }

    /// Add a single command to the runtime allowlist.
    pub fn add_allowed_command(&mut self, cmd: &str) {
        if !self.policy_allowed_commands.iter().any(|c| c == cmd) {
            self.policy_allowed_commands.push(cmd.to_string());
        }
    }

    /// Add a path to the runtime read-only allowlist.
    pub fn add_allowed_path_ro(&mut self, path: &str) {
        if !self.policy_allowed_paths_ro.iter().any(|p| p == path) {
            self.policy_allowed_paths_ro.push(path.to_string());
        }
    }

    /// Add a path to the runtime read-write allowlist.
    pub fn add_allowed_path_rw(&mut self, path: &str) {
        if !self.policy_allowed_paths_rw.iter().any(|p| p == path) {
            self.policy_allowed_paths_rw.push(path.to_string());
        }
    }

    /// Add an individual file to the runtime read-only allowlist.
    pub fn add_allowed_file_ro(&mut self, path: &str) {
        if !self.policy_allowed_files_ro.iter().any(|p| p == path) {
            self.policy_allowed_files_ro.push(path.to_string());
        }
    }

    /// Add an individual file to the runtime read-write allowlist.
    pub fn add_allowed_file_rw(&mut self, path: &str) {
        if !self.policy_allowed_files_rw.iter().any(|p| p == path) {
            self.policy_allowed_files_rw.push(path.to_string());
        }
    }

    /// Set command execution policy.
    pub fn set_policy(&mut self, policy: &crate::config::PolicyConfig) {
        self.policy_mode = policy.mode.clone();
        self.policy_allowed_tools = policy.allowed_tools.clone();
        self.fetch_max_size_kb = policy.fetch_max_size_kb;
        self.policy_allowed_commands = policy.allowed_commands.clone();
        self.policy_allowed_syscalls = policy.allowed_syscalls.clone();
        self.policy_denied_paths = policy.denied_paths.clone();
        self.policy_allowed_paths_rw = policy.allowed_paths_rw.clone();
        self.policy_allowed_paths_ro = policy.allowed_paths_ro.clone();
        self.policy_allowed_files_ro = policy.allowed_files_ro.clone();
        self.policy_allowed_files_rw = policy.allowed_files_rw.clone();
        self.allowed_domains = policy.allowed_domains.clone();
        self.tmp_size_mb = policy.max_tmp_mb;
        self.mount_pwd = policy.mount_pwd;
        self.policy_mount_home = policy.mount_home.clone();
    }

    /// Create a sandbox executor with the registry's config.
    fn sandbox(&self, timeout_secs: u64) -> SandboxExecutor {
        // Unrestricted mode: minimal sandbox (timeout only, no filesystem/network restrictions)
        if self.policy_mode == "unrestricted" {
            let config = SandboxConfig {
                timeout_secs,
                read_write_paths: vec![PathBuf::from("/")],
                read_only_paths: vec![],
                denied_paths: vec![],
                allowed_domains: vec!["*".to_string()],
                tmp_size_mb: 0, // no tmpfs isolation in unrestricted mode
                mount_home: self.policy_mount_home.as_ref().map(PathBuf::from),
                ..SandboxConfig::default()
            };
            return SandboxExecutor::new(config);
        }
        // Build read-only paths: user-configured + essential system paths
        let mut read_only: Vec<PathBuf> = if self.policy_allowed_paths_ro.is_empty() {
            SandboxConfig::default().read_only_paths
        } else {
            self.policy_allowed_paths_ro
                .iter()
                .map(PathBuf::from)
                .collect()
        };
        // Always ensure essential system paths are accessible (required for exec + DNS)
        // /etc is safe: sandbox mounts /tmp/.etc over /etc, hiding sensitive files.
        // /proc and /sys are denied. /run is traverse-only (for systemd-run scope).
        for p in &["/bin", "/usr", "/lib", "/lib64", "/etc"] {
            let pb = PathBuf::from(p);
            if pb.exists() && !read_only.contains(&pb) {
                read_only.push(pb);
            }
        }
        // Include the directory containing the rune binary
        // They may be in ~/.cargo/bin or /usr/local/bin — Landlock needs read+exec access.
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let exe_dir = exe_dir.to_path_buf();
                if !read_only.contains(&exe_dir) {
                    read_only.push(exe_dir);
                }
            }
        }
        // Always include CWD and its parents so sandboxed commands can access the working directory.
        // Landlock requires rules on parent directories for path traversal.
        if let Ok(cwd) = std::env::current_dir() {
            if !read_only.contains(&cwd) {
                read_only.push(cwd);
            }
        }
        // Canonicalize allowed_dirs (resolve "." to absolute path for Landlock)
        let mut rw_paths: Vec<PathBuf> = self
            .allowed_dirs
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect();
        for p in &self.policy_allowed_paths_rw {
            let pb = std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
            if !rw_paths.contains(&pb) {
                rw_paths.push(pb);
            }
        }
        if let Some(ref h) = self.policy_mount_home {
            let pb = std::fs::canonicalize(h).unwrap_or_else(|_| PathBuf::from(h));
            if !rw_paths.contains(&pb) {
                rw_paths.push(pb);
            }
            if let Ok(real_home) = std::env::var("HOME") {
                let real_pb = PathBuf::from(real_home);
                if !rw_paths.contains(&real_pb) {
                    rw_paths.push(real_pb);
                }
            }
        }
        // Essential device nodes that nearly all commands need
        rw_paths.push(PathBuf::from("/dev/null"));
        rw_paths.push(PathBuf::from("/dev/urandom"));
        // Add individual allowed files and their parent directories for traversal
        let mut traverse_paths: Vec<PathBuf> = vec![PathBuf::from("/dev")];
        for f in &self.policy_allowed_files_rw {
            let pb = PathBuf::from(f);
            rw_paths.push(pb.clone());
            // Parent needs traverse-only access for Landlock lookup
            if let Some(parent) = pb.parent() {
                let parent = parent.to_path_buf();
                if !read_only.contains(&parent)
                    && !rw_paths.contains(&parent)
                    && !traverse_paths.contains(&parent)
                {
                    traverse_paths.push(parent);
                }
            }
        }
        for f in &self.policy_allowed_files_ro {
            let pb = PathBuf::from(f);
            if !read_only.contains(&pb) {
                read_only.push(pb.clone());
            }
            // Parent needs traverse-only access for Landlock lookup
            if let Some(parent) = pb.parent() {
                let parent = parent.to_path_buf();
                if !read_only.contains(&parent)
                    && !rw_paths.contains(&parent)
                    && !traverse_paths.contains(&parent)
                {
                    traverse_paths.push(parent);
                }
            }
        }

        let session_tmp_dir = self.session_tmp.as_ref().map(|s| s.path().to_path_buf());
        if let Some(ref sdir) = session_tmp_dir {
            let pb = std::fs::canonicalize(sdir).unwrap_or_else(|_| sdir.clone());
            if !rw_paths.contains(&pb) {
                rw_paths.push(pb);
            }
        }

        let config = SandboxConfig {
            timeout_secs,
            read_write_paths: rw_paths,
            read_only_paths: read_only,
            traverse_paths,
            allowed_domains: self.allowed_domains.clone(),
            allowed_syscalls: self.policy_allowed_syscalls.clone(),
            tmp_size_mb: self.tmp_size_mb,
            session_tmp_dir,
            mount_home: self.policy_mount_home.as_ref().map(PathBuf::from),

            ..SandboxConfig::default()
        };
        SandboxExecutor::new(config)
    }

    /// Run a command in sandbox and return ToolOutput.
    async fn sandboxed_cmd(&self, cmd: &str, timeout_secs: u64, cwd: Option<&str>) -> ToolOutput {
        let effective_cwd = cwd.map(|s| s.to_string()).or_else(|| {
            if self.policy_mount_home.is_some() {
                if let Ok(real_home) = std::env::var("HOME") {
                    return Some(real_home);
                }
            }
            if let Ok(current) = std::env::current_dir() {
                let current_str = current.to_string_lossy().to_string();
                let is_allowed = self.mount_pwd
                    || self
                        .policy_allowed_paths_rw
                        .iter()
                        .any(|p| current_str.starts_with(p.trim_end_matches('/')))
                    || self
                        .policy_allowed_paths_ro
                        .iter()
                        .any(|p| current_str.starts_with(p.trim_end_matches('/')));

                if is_allowed {
                    return Some(current_str);
                }
            }
            None
        });
        let executor = self.sandbox(timeout_secs);

        // Build a PATH that includes directories from allowed_paths_ro where allowed
        // commands may reside. The sandbox chains multiple wrappers (systemd-run,
        // sandbox wrapper subcommands) before the inner `sh -c`,
        // and env vars set on the outer process don't propagate through all layers.
        // So we prepend `export PATH=...;` directly into the command string.
        let system_path =
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_string());
        let mut extra_paths: Vec<String> = Vec::new();
        for p in &self.policy_allowed_paths_ro {
            let path = std::path::Path::new(p);
            if path.is_dir() && !system_path.contains(p.as_str()) {
                extra_paths.push(p.clone());
            }
        }
        let effective_cmd = if !extra_paths.is_empty() {
            extra_paths.push(system_path);
            let path_val = extra_paths.join(":");
            format!("export PATH='{}'; {}", path_val, cmd)
        } else {
            cmd.to_string()
        };

        match executor
            .run_shell_command(&effective_cmd, effective_cwd.as_deref(), None)
            .await
        {
            Ok(result) => {
                // Include sandbox diagnostics in all returned ToolOutput values
                if result.timed_out {
                    return ToolOutput {
                        content: format!("command timed out after {}s", timeout_secs),
                        is_error: true,
                        active_layers: Some(result.active_layers),
                        degraded: Some(result.degraded),
                    };
                }
                let output = if !result.stderr.is_empty() && result.exit_code != 0 {
                    format!(
                        "{}
{}",
                        result.stdout, result.stderr
                    )
                } else {
                    result.stdout.clone()
                };
                if result.exit_code != 0 {
                    ToolOutput {
                        content: format!(
                            "exit_code: {}
stdout: {}
stderr: {}",
                            result.exit_code, result.stdout, result.stderr
                        ),
                        is_error: true,
                        active_layers: Some(result.active_layers),
                        degraded: Some(result.degraded),
                    }
                } else {
                    ToolOutput {
                        content: output,
                        is_error: false,
                        active_layers: Some(result.active_layers),
                        degraded: Some(result.degraded),
                    }
                }
            }
            Err(e) => ToolOutput::err(format!("sandbox error: {}", e)),
        }
    }

    /// Dispatch a tool call by name.
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> ToolOutput {
        info!(tool = name, "executing tool");
        if !self.is_tool_allowed(name) {
            return ToolOutput::err(format!("BLOCKED: tool '{}' is not in allowed_tools", name));
        }
        match name {
            "read_file" => self.read_file(args).await,
            "write_file" => self.write_file(args).await,
            "list_dir" => self.list_dir(args).await,
            "execute_cmd" => self.execute_cmd(args).await,
            "fetch_url" => self.fetch_url(args).await,
            other => ToolOutput::err(format!("unknown tool: {}", other)),
        }
    }

    /// Return tool definitions as JSON (for LLM function calling schema).
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        let mut tools = Vec::new();

        if self.is_tool_allowed("read_file") {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a file's contents (sandboxed). Truncates at 32KB.",
                    "parameters": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                }
            }));
        }
        if self.is_tool_allowed("write_file") {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write content to a file (sandboxed). Creates parent dirs.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    }
                }
            }));
        }
        if self.is_tool_allowed("list_dir") {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "list_dir",
                    "description": "List directory contents (sandboxed).",
                    "parameters": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                }
            }));
        }
        if self.is_tool_allowed("execute_cmd") {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "execute_cmd",
                    "description": "Execute a shell command (sandboxed, network isolated by default).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "cmd": { "type": "string" },
                            "cwd": { "type": "string" },
                            "timeout_secs": { "type": "integer" }
                        },
                        "required": ["cmd"]
                    }
                }
            }));
        }
        if self.is_tool_allowed("fetch_url") {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "fetch_url",
                    "description": "Fetch content from a URL via HTTP/HTTPS. Supports HTML-to-Markdown purification, direct file download (save_to), and domain allowlist enforcement.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "The HTTP or HTTPS URL to fetch." },
                            "save_to": { "type": "string", "description": "Optional relative or absolute file path to save the response body directly to disk instead of returning it to context (ideal for large JSON, CSV, or binary files)." },
                            "raw": { "type": "boolean", "description": "Optional flag (default: false). If true, disables HTML-to-Markdown conversion and returns the raw response body." }
                        },
                        "required": ["url"]
                    }
                }
            }));
        }

        // Serve-mode only tools (search_chat, markdown tools)
        if self.serve_mode {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "search_chat",
                    "description": "Search the conversation history (current session + archives) by keyword. Returns matching messages with timestamps and nicknames.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Keyword to search for in chat history" }
                        },
                        "required": ["query"]
                    }
                }
            }));
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "list_markdown",
                    "description": "List all .md files in the current notebook and show which one is active. Call this first whenever you are unsure which files exist.",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            }));
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_markdown",
                    "description": "Read a notebook file. Content is always plain text (markdown). Omit 'filename' to read the currently active file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "filename": { "type": "string", "description": "Bare .md filename only (e.g. 'notes.md'). No paths, no subdirectories, no other extensions. Omit to use the active file." }
                        },
                        "required": []
                    }
                }
            }));
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "write_markdown",
                    "description": "Write plain-text markdown content to a notebook file. Use 'content' to replace the whole file, or 'search'+'replace' for a targeted edit. Omit 'filename' to write to the active file. Creates the file if it does not exist.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "filename": { "type": "string", "description": "Bare .md filename only (e.g. 'notes.md'). No paths, no subdirectories, no other extensions. Omit to use the active file." },
                            "content": { "type": "string", "description": "Full new plain-text content (replaces entire file). Cannot be used together with 'search'/'replace'." },
                            "search": { "type": "string", "description": "Exact text to find in the current file (used with 'replace')." },
                            "replace": { "type": "string", "description": "Replacement text for the first match of 'search'." }
                        },
                        "required": []
                    }
                }
            }));
        }

        tools
    }

    // ── All tools go through sandbox ─────────────────────────────────

    async fn read_file(&self, args: serde_json::Value) -> ToolOutput {
        let path = match args
            .get("path")
            .or_else(|| args.get("filename"))
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("filepath"))
            .and_then(|v| v.as_str())
        {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        // Check path policy
        if self.is_path_denied(path) {
            return ToolOutput::err(format!(
                "BLOCKED by policy: path '{}' is in denied_paths",
                path
            ));
        }

        info!(path = %path, "read_file (sandboxed)");
        let cmd = format!(
            "head -c {} '{}'",
            MAX_FILE_SIZE,
            path.replace('\'', "'\\''")
        );
        let result = self.sandboxed_cmd(&cmd, 10, None).await;
        if result.is_error {
            return result;
        }
        if result.content.len() >= MAX_FILE_SIZE {
            ToolOutput::ok(format!("{}\n[Content Truncated at 32KB]", result.content))
        } else {
            result
        }
    }

    async fn write_file(&self, args: serde_json::Value) -> ToolOutput {
        let path = match args
            .get("path")
            .or_else(|| args.get("filename"))
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("filepath"))
            .and_then(|v| v.as_str())
        {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolOutput::err("missing required argument: content"),
        };
        // Check path policy
        if self.is_path_denied(path) {
            return ToolOutput::err(format!(
                "BLOCKED by policy: path '{}' is in denied_paths",
                path
            ));
        }
        if !self.policy_allowed_paths_rw.is_empty()
            && !self.is_path_in_list(path, &self.policy_allowed_paths_rw)
            && !self.is_file_in_list(path, &self.policy_allowed_files_rw)
        {
            return ToolOutput::err(format!(
                "BLOCKED by policy: path '{}' is not in allowed_paths_rw",
                path
            ));
        }

        info!(path = %path, bytes = content.len(), "write_file (sandboxed)");
        let escaped_path = path.replace('\'', "'\\''");
        let escaped_content = content.replace('\'', "'\\''");
        let cmd = format!(
            "mkdir -p $(dirname '{}') && printf '%s' '{}' > '{}'",
            escaped_path, escaped_content, escaped_path
        );
        let result = self.sandboxed_cmd(&cmd, 10, None).await;
        if result.is_error {
            return result;
        }
        {
            let mut o = ToolOutput::ok(format!("Written {} bytes to {}", content.len(), path));
            o.active_layers = result.active_layers.clone();
            o.degraded = result.degraded;
            o
        }
    }

    async fn list_dir(&self, args: serde_json::Value) -> ToolOutput {
        let path = args
            .get("path")
            .or_else(|| args.get("dir"))
            .or_else(|| args.get("directory"))
            .or_else(|| args.get("folder"))
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        info!(path = %path, "list_dir (sandboxed)");
        let cmd = format!("ls -1F '{}'", path.replace('\'', "'\\''"));
        self.sandboxed_cmd(&cmd, 10, None).await
    }

    async fn execute_cmd(&self, args: serde_json::Value) -> ToolOutput {
        let cmd = match args.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return ToolOutput::err("missing required argument: cmd"),
        };
        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        info!(cmd = %cmd, ?cwd, timeout_secs, "execute_cmd (sandboxed)");

        // Command policy enforcement: check ALL commands in the pipeline
        // Applies in both "allowlist" and "confirm" modes when allowed_commands is set
        // Skipped entirely in "unrestricted" mode
        if self.policy_mode != "unrestricted"
            && !self.policy_allowed_commands.iter().any(|a| a == "*")
        {
            let binaries = extract_command_binaries(&cmd);
            for binary in &binaries {
                if !self.policy_allowed_commands.iter().any(|a| a == binary) {
                    return ToolOutput::err(format!(
                        "BLOCKED by policy: command '{}' is not in allowed_commands",
                        binary
                    ));
                }
            }
        }
        self.sandboxed_cmd(&cmd, timeout_secs, cwd.as_deref()).await
    }

    async fn fetch_url(&self, args: serde_json::Value) -> ToolOutput {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.trim(),
            None => return ToolOutput::err("missing required argument: url"),
        };
        let save_to = args
            .get("save_to")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolOutput::err("invalid URL: must start with http:// or https://");
        }

        let domain = match extract_domain(url) {
            Some(d) => d,
            None => return ToolOutput::err("invalid URL: could not extract domain"),
        };

        // Check domain allowlist (skipped in unrestricted mode)
        if self.policy_mode != "unrestricted" {
            if !self.is_domain_allowed(&domain) {
                return ToolOutput::err(format!(
                    "BLOCKED: domain '{}' is not in allowed_domains. \
                     Network access requires explicit allowlist configuration.",
                    domain
                ));
            }

            // SSRF Check: resolve hostname and verify it is not a private/restricted IP
            let is_explicit_local =
                (domain == "localhost" || domain == "127.0.0.1" || domain == "::1")
                    && self.allowed_domains.iter().any(|d| d == &domain);

            if !is_explicit_local {
                let port = if url.starts_with("https://") { 443 } else { 80 };
                let host_port = if domain.contains(':') {
                    domain.clone()
                } else {
                    format!("{}:{}", domain, port)
                };

                if let Ok(mut addrs) = tokio::net::lookup_host(host_port).await {
                    while let Some(addr) = addrs.next() {
                        let ip = addr.ip();
                        if is_private_or_restricted_ip(&ip) {
                            return ToolOutput::err(format!(
                                "BLOCKED: domain '{}' resolves to private/restricted IP ({}) which is prohibited by SSRF protection.",
                                domain, ip
                            ));
                        }
                    }
                }
            }
        } // end unrestricted check

        info!(url = %url, "fetch_url (native in-process, domain allowed)");

        let allowed_domains = self.allowed_domains.clone();
        let is_unrestricted = self.policy_mode == "unrestricted";

        // Custom redirect policy: ensure redirects stay within allowed_domains
        let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects (max 10)");
            }
            if !is_unrestricted {
                let next_url = attempt.url().as_str();
                if !next_url.starts_with("http://") && !next_url.starts_with("https://") {
                    return attempt.error("redirect to non-http/https URL is blocked");
                }
                if let Some(next_domain) = extract_domain(next_url) {
                    let allowed = allowed_domains.iter().any(|d| {
                        d == &next_domain
                            || d == "*"
                            || (d.starts_with("*.") && next_domain.ends_with(&d[1..]))
                    });
                    if !allowed {
                        return attempt.error(format!(
                            "BLOCKED: redirect to unauthorized domain '{}'",
                            next_domain
                        ));
                    }
                } else {
                    return attempt.error("redirect URL has invalid domain");
                }
            }
            attempt.follow()
        });

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Rune/0.1.0")
            .redirect(redirect_policy)
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("failed to create HTTP client: {}", e)),
        };

        let response = match client.get(url).send().await {
            Ok(res) => res,
            Err(e) => {
                return ToolOutput::err(format!("HTTP request failed: {}", e));
            }
        };

        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        // If save_to is provided, stream directly to file
        if let Some(target_path_str) = save_to {
            if self.is_path_denied(&target_path_str) {
                return ToolOutput::err(format!(
                    "BLOCKED: cannot save to denied path '{}'",
                    target_path_str
                ));
            }

            let resolved_path = self.resolve_path(&target_path_str);
            let dest_path = PathBuf::from(&resolved_path);

            if self.policy_mode != "unrestricted" {
                let is_in_rw = self
                    .is_path_in_list(&target_path_str, &self.policy_allowed_paths_rw)
                    || self.is_file_in_list(&target_path_str, &self.policy_allowed_files_rw);
                let is_in_allowed_dirs = self.allowed_dirs.iter().any(|d| dest_path.starts_with(d));
                let is_cwd = std::env::current_dir()
                    .map(|cwd| dest_path.starts_with(cwd))
                    .unwrap_or(false);
                if !is_in_rw && !is_in_allowed_dirs && !is_cwd {
                    return ToolOutput::err(format!(
                        "BLOCKED: destination path '{}' is not in allowed write paths",
                        target_path_str
                    ));
                }
            }

            if let Some(parent) = dest_path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return ToolOutput::err(format!(
                        "failed to create parent directories for '{}': {}",
                        target_path_str, e
                    ));
                }
            }

            let mut file = match tokio::fs::File::create(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    return ToolOutput::err(format!(
                        "failed to create destination file '{}': {}",
                        target_path_str, e
                    ));
                }
            };

            let max_download_size: usize = 10 * 1024 * 1024; // 10MB limit for file download
            let mut total_bytes = 0usize;
            let mut stream = response.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        total_bytes += chunk.len();
                        if total_bytes > max_download_size {
                            let _ = tokio::fs::remove_file(&dest_path).await;
                            return ToolOutput::err(
                                "download exceeded maximum allowed size (10MB)".to_string(),
                            );
                        }
                        if let Err(e) = file.write_all(&chunk).await {
                            return ToolOutput::err(format!(
                                "failed to write to file '{}': {}",
                                target_path_str, e
                            ));
                        }
                    }
                    Err(e) => {
                        return ToolOutput::err(format!("error reading response stream: {}", e));
                    }
                }
            }

            if let Err(e) = file.flush().await {
                return ToolOutput::err(format!(
                    "failed to flush destination file '{}': {}",
                    target_path_str, e
                ));
            }

            let mut output = ToolOutput::ok(format!(
                "Successfully downloaded {} bytes to {}",
                total_bytes, target_path_str
            ));
            output.active_layers = Some(vec!["native-fetch".to_string()]);
            return output;
        }

        // Direct read to context
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let is_html = content_type.contains("text/html");

        let max_size = self.fetch_max_size_kb.unwrap_or(128) * 1024;
        let stream_limit = if is_html && !raw {
            1024 * 1024 // 1MB buffer for HTML purification
        } else {
            max_size
        };

        let mut stream = response.bytes_stream();
        let mut body_bytes = Vec::new();
        let mut stream_truncated = false;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let remaining = stream_limit.saturating_sub(body_bytes.len());
                    if chunk.len() > remaining {
                        body_bytes.extend_from_slice(&chunk[..remaining]);
                        stream_truncated = true;
                        break;
                    } else {
                        body_bytes.extend_from_slice(&chunk);
                    }
                }
                Err(e) => {
                    return ToolOutput::err(format!("error reading response stream: {}", e));
                }
            }
        }

        let raw_text = String::from_utf8_lossy(&body_bytes).to_string();
        let processed_text = if is_html && !raw {
            html_to_markdown(&raw_text)
        } else {
            raw_text
        };

        let final_content = if processed_text.len() > max_size {
            let truncated_slice = &processed_text[..max_size];
            format!(
                "{}\n[Content Truncated at {}KB]",
                truncated_slice,
                max_size / 1024
            )
        } else if stream_truncated && (raw || !is_html) {
            format!(
                "{}\n[Content Truncated at {}KB]",
                processed_text,
                max_size / 1024
            )
        } else {
            processed_text
        };

        let mut output = ToolOutput::ok(final_content);
        output.active_layers = Some(vec!["native-fetch".to_string()]);
        output
    }
}

// ─── HTML to Markdown Purification ──────────────────────────────────────────

/// Strip tag blocks like `<script ...>...</script>`, case-insensitively.
fn strip_tag_blocks(mut input: String, tag: &str) -> String {
    let open_prefix = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);
    loop {
        let lower = input.to_lowercase();
        if let Some(start_idx) = lower.find(&open_prefix) {
            let next_char = input[start_idx + open_prefix.len()..].chars().next();
            if let Some(c) = next_char {
                if c.is_whitespace() || c == '>' || c == '/' {
                    if let Some(end_rel) = lower[start_idx..].find(&close_tag) {
                        let end_idx = start_idx + end_rel + close_tag.len();
                        input.drain(start_idx..end_idx);
                        continue;
                    } else if let Some(tag_end) = lower[start_idx..].find('>') {
                        let end_idx = start_idx + tag_end + 1;
                        input.drain(start_idx..end_idx);
                        continue;
                    }
                }
            }
        }
        break;
    }
    input
}

/// Strip HTML comments `<!-- ... -->`.
fn strip_html_comments(mut input: String) -> String {
    while let Some(start) = input.find("<!--") {
        if let Some(end_rel) = input[start..].find("-->") {
            input.drain(start..start + end_rel + 3);
        } else {
            input.truncate(start);
            break;
        }
    }
    input
}

/// Extract and format HTML tables as Markdown tables.
fn format_html_tables(mut input: String) -> String {
    let open_tag = "<table";
    let close_tag = "</table>";
    loop {
        let lower = input.to_lowercase();
        if let Some(start_idx) = lower.find(open_tag) {
            if let Some(end_rel) = lower[start_idx..].find(close_tag) {
                let end_idx = start_idx + end_rel + close_tag.len();
                let table_html = &input[start_idx..end_idx];
                let md_table = convert_single_table(table_html);
                input.replace_range(start_idx..end_idx, &format!("\n\n{}\n\n", md_table));
                continue;
            }
        }
        break;
    }
    input
}

fn convert_single_table(html: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let lower = html.to_lowercase();
    let mut tr_start = 0;
    while let Some(tr_open) = lower[tr_start..].find("<tr") {
        let actual_tr_open = tr_start + tr_open;
        if let Some(tr_close) = lower[actual_tr_open..].find("</tr>") {
            let actual_tr_close = actual_tr_open + tr_close;
            let tr_content = &html[actual_tr_open..actual_tr_close];
            let cells = extract_table_cells(tr_content);
            if !cells.is_empty() {
                rows.push(cells);
            }
            tr_start = actual_tr_close + 5;
        } else {
            break;
        }
    }

    if rows.is_empty() {
        return String::new();
    }

    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if max_cols == 0 {
        return String::new();
    }

    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let mut padded = row.clone();
        while padded.len() < max_cols {
            padded.push(String::new());
        }
        out.push_str("| ");
        out.push_str(&padded.join(" | "));
        out.push_str(" |\n");
        if i == 0 {
            out.push_str("| ");
            let sep: Vec<&str> = (0..max_cols).map(|_| "---").collect();
            out.push_str(&sep.join(" | "));
            out.push_str(" |\n");
        }
    }
    out.trim_end().to_string()
}

fn extract_table_cells(tr_html: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let lower = tr_html.to_lowercase();
    let mut pos = 0;
    while pos < tr_html.len() {
        let th_pos = lower[pos..].find("<th");
        let td_pos = lower[pos..].find("<td");
        let (tag_type, found_pos) = match (th_pos, td_pos) {
            (Some(th), Some(td)) => {
                if th < td {
                    ("th", pos + th)
                } else {
                    ("td", pos + td)
                }
            }
            (Some(th), None) => ("th", pos + th),
            (None, Some(td)) => ("td", pos + td),
            (None, None) => break,
        };
        let tag_close_sym = format!("</{}>", tag_type);
        if let Some(open_end) = lower[found_pos..].find('>') {
            let content_start = found_pos + open_end + 1;
            if let Some(close_pos) = lower[content_start..].find(&tag_close_sym) {
                let cell_content = &tr_html[content_start..content_start + close_pos];
                let clean_text = clean_html_cell(cell_content);
                cells.push(clean_text);
                pos = content_start + close_pos + tag_close_sym.len();
            } else {
                pos = content_start;
            }
        } else {
            break;
        }
    }
    cells
}

fn clean_html_cell(html: &str) -> String {
    let stripped = strip_tags(html);
    let decoded = decode_html_entities(&stripped);
    decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out
}

fn decode_html_entities(input: &str) -> String {
    let mut res = input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&copy;", "©")
        .replace("&reg;", "®")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–");

    if res.contains("&#") {
        let mut final_res = String::with_capacity(res.len());
        let mut chars = res.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '&' && chars.peek() == Some(&'#') {
                chars.next(); // consume '#'
                let mut num_str = String::new();
                let is_hex = if chars.peek() == Some(&'x') || chars.peek() == Some(&'X') {
                    chars.next();
                    true
                } else {
                    false
                };
                while let Some(&nc) = chars.peek() {
                    if nc == ';' {
                        chars.next();
                        break;
                    } else if (is_hex && nc.is_ascii_hexdigit()) || (!is_hex && nc.is_ascii_digit())
                    {
                        num_str.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let code_point = if is_hex {
                    u32::from_str_radix(&num_str, 16).ok()
                } else {
                    num_str.parse::<u32>().ok()
                };
                if let Some(ch) = code_point.and_then(char::from_u32) {
                    final_res.push(ch);
                } else {
                    final_res.push_str("&#");
                    if is_hex {
                        final_res.push('x');
                    }
                    final_res.push_str(&num_str);
                }
            } else {
                final_res.push(c);
            }
        }
        res = final_res;
    }
    res
}

fn extract_href(tag: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    if let Some(href_pos) = lower.find("href=") {
        let after_href = tag[href_pos + 5..].trim_start();
        let quote = after_href.chars().next()?;
        if quote == '"' || quote == '\'' {
            let rest = &after_href[1..];
            if let Some(end_quote) = rest.find(quote) {
                return Some(rest[..end_quote].to_string());
            }
        } else {
            let end_pos = after_href
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(after_href.len());
            return Some(after_href[..end_pos].to_string());
        }
    }
    None
}

fn clean_markdown_whitespace(input: &str) -> String {
    let mut lines = Vec::new();
    let mut empty_count = 0;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            empty_count += 1;
            if empty_count <= 1 && !lines.is_empty() {
                lines.push("");
            }
        } else {
            empty_count = 0;
            lines.push(trimmed);
        }
    }
    lines.join("\n").trim().to_string()
}

/// Convert HTML text to purified Markdown.
pub fn html_to_markdown(html: &str) -> String {
    let mut s = html.to_string();

    // 1. Strip comments
    s = strip_html_comments(s);

    // 2. Strip noise blocks
    for tag in &[
        "script", "style", "svg", "noscript", "nav", "footer", "header", "iframe",
    ] {
        s = strip_tag_blocks(s, tag);
    }

    // 3. Format tables
    s = format_html_tables(s);

    // 4. Format headings h1..h6
    for level in (1..=6).rev() {
        let open_tag = format!("<h{}", level);
        let close_tag = format!("</h{}>", level);
        let hash = "#".repeat(level);
        loop {
            let lower = s.to_lowercase();
            if let Some(start_idx) = lower.find(&open_tag) {
                if let Some(close_rel) = lower[start_idx..].find(&close_tag) {
                    let end_idx = start_idx + close_rel + close_tag.len();
                    if let Some(tag_end) = lower[start_idx..].find('>') {
                        let inner = &s[start_idx + tag_end + 1..start_idx + close_rel];
                        let clean_inner =
                            decode_html_entities(&strip_tags(inner)).trim().to_string();
                        let replacement = format!("\n\n{} {}\n\n", hash, clean_inner);
                        s.replace_range(start_idx..end_idx, &replacement);
                        continue;
                    }
                }
            }
            break;
        }
    }

    // 5. Format links `<a ...href="...">text</a>`
    loop {
        let lower = s.to_lowercase();
        if let Some(start_idx) = lower.find("<a") {
            if let Some(close_rel) = lower[start_idx..].find("</a>") {
                let end_idx = start_idx + close_rel + 4;
                if let Some(tag_end) = lower[start_idx..].find('>') {
                    let a_tag = &s[start_idx..start_idx + tag_end + 1];
                    let inner = &s[start_idx + tag_end + 1..start_idx + close_rel];
                    let href = extract_href(a_tag);
                    let clean_inner = decode_html_entities(&strip_tags(inner)).trim().to_string();
                    let replacement = if let Some(href_url) = href {
                        if clean_inner.is_empty() {
                            format!(" {}", href_url)
                        } else {
                            format!(" [{}]({}) ", clean_inner, href_url)
                        }
                    } else {
                        clean_inner
                    };
                    s.replace_range(start_idx..end_idx, &replacement);
                    continue;
                }
            }
        }
        break;
    }

    // 6. Format lists
    loop {
        let lower = s.to_lowercase();
        if let Some(start_idx) = lower.find("<li") {
            if let Some(close_rel) = lower[start_idx..].find("</li>") {
                let end_idx = start_idx + close_rel + 5;
                if let Some(tag_end) = lower[start_idx..].find('>') {
                    let inner = &s[start_idx + tag_end + 1..start_idx + close_rel];
                    let clean_inner = decode_html_entities(&strip_tags(inner)).trim().to_string();
                    let replacement = format!("\n- {}\n", clean_inner);
                    s.replace_range(start_idx..end_idx, &replacement);
                    continue;
                }
            }
        }
        break;
    }

    // 7. Format block breaks & paragraphs
    s = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<p>", "\n\n")
        .replace("</p>", "\n\n")
        .replace("<hr>", "\n\n---\n\n")
        .replace("<hr/>", "\n\n---\n\n")
        .replace("<hr />", "\n\n---\n\n")
        .replace("<b>", "**")
        .replace("</b>", "**")
        .replace("<strong>", "**")
        .replace("</strong>", "**")
        .replace("<i>", "*")
        .replace("</i>", "*")
        .replace("<em>", "*")
        .replace("</em>", "*")
        .replace("<code>", "`")
        .replace("</code>", "`");

    // 8. Strip remaining tags
    s = strip_tags(&s);

    // 9. Decode HTML entities
    s = decode_html_entities(&s);

    // 10. Clean up whitespace
    clean_markdown_whitespace(&s)
}

impl ToolRegistry {
    /// Check if a path is in the denied_paths list.
    fn is_path_denied(&self, path: &str) -> bool {
        if self.policy_mode == "unrestricted" {
            return false;
        }
        let resolved = self.resolve_path(path);
        self.policy_denied_paths
            .iter()
            .any(|d| resolved.starts_with(d))
    }

    /// Check if a path starts with any entry in a given list.
    fn is_path_in_list(&self, path: &str, list: &[String]) -> bool {
        let resolved = self.resolve_path(path);
        list.iter().any(|p| {
            let norm_p = p.trim_end_matches('/');
            let norm_p = norm_p.trim_end_matches("/.");
            resolved == norm_p || resolved.starts_with(&format!("{}/", norm_p))
        })
    }

    /// Check if a resolved file path exactly matches any entry in the list.
    fn is_file_in_list(&self, path: &str, list: &[String]) -> bool {
        if list.is_empty() {
            return false;
        }
        let resolved = self.resolve_path(path);
        list.iter().any(|f| resolved == *f)
    }

    /// Resolve a relative path to absolute for policy matching.
    fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            Self::normalize_path(path)
        } else {
            let abs = std::env::current_dir()
                .map(|cwd| format!("{}/{}", cwd.display(), path))
                .unwrap_or_else(|_| path.to_string());
            Self::normalize_path(&abs)
        }
    }

    /// Simple path normalization: collapse . and .. components, remove trailing /.
    fn normalize_path(path: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for component in path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                _ => parts.push(component),
            }
        }
        format!("/{}", parts.join("/"))
    }
}

/// Extract all command binaries from a shell command string.
/// Splits on unquoted shell separators (; | && ||) while respecting
/// single quotes, double quotes, and backslash escapes.
pub fn extract_command_binaries_pub(cmd: &str) -> Vec<String> {
    extract_command_binaries(cmd)
}

fn extract_command_binaries(cmd: &str) -> Vec<String> {
    let mut binaries = Vec::new();
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            current.push(ch);
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            current.push(ch);
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            current.push(ch);
            continue;
        }
        if !in_single && !in_double {
            match ch {
                ';' => {
                    segments.push(std::mem::take(&mut current));
                    continue;
                }
                '|' => {
                    // || is a separator too, consume second |
                    if chars.peek() == Some(&'|') {
                        chars.next();
                    }
                    segments.push(std::mem::take(&mut current));
                    continue;
                }
                '&' => {
                    // Check if this is part of a redirect (>&, &>, 2>&1, etc.)
                    let prev_is_redirect = current.ends_with('>') || current.ends_with('<');
                    let next_is_redirect = chars.peek() == Some(&'>');
                    if prev_is_redirect || next_is_redirect {
                        // Part of a redirect operator, not a separator
                        current.push(ch);
                        continue;
                    }
                    // && is a separator, single & (background) is also a separator
                    if chars.peek() == Some(&'&') {
                        chars.next();
                    }
                    segments.push(std::mem::take(&mut current));
                    continue;
                }
                _ => {}
            }
        }
        current.push(ch);
    }
    if !current.is_empty() {
        segments.push(current);
    }

    for seg in &segments {
        let trimmed = seg.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(binary) = extract_primary_binary(trimmed) {
            binaries.push(binary);
        }
    }
    binaries
}

fn extract_primary_binary(segment: &str) -> Option<String> {
    for token in segment.split_whitespace() {
        let token = token
            .trim_start_matches(|c: char| matches!(c, '(' | ')' | '{' | '}' | '[' | ']' | '!'));
        let token =
            token.trim_end_matches(|c: char| matches!(c, '(' | ')' | '{' | '}' | '[' | ']'));
        if token.is_empty() {
            continue;
        }
        if is_shell_assignment(token) || is_shell_keyword(token) {
            continue;
        }
        let binary = token.rsplit('/').next().unwrap_or(token);
        let binary = binary.trim_matches(|c: char| matches!(c, '(' | ')' | '{' | '}' | '[' | ']'));
        if !binary.is_empty() {
            return Some(binary.to_string());
        }
    }
    None
}

fn is_shell_assignment(token: &str) -> bool {
    let Some(eq) = token.find('=') else {
        return false;
    };
    let (name, value) = token.split_at(eq);
    !name.is_empty()
        && !value[1..].is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_shell_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "while"
            | "until"
            | "for"
            | "in"
            | "time"
    )
}

/// Extract domain from a URL string.
fn extract_domain(url: &str) -> Option<String> {
    // Simple extraction: strip scheme, take host part
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next()?;
    let domain = host.split(':').next()?; // strip port
    if domain.is_empty() {
        None
    } else {
        Some(domain.to_string())
    }
}

/// Check if an IP address is a private, loopback, link-local, or otherwise restricted address.
fn is_private_or_restricted_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 0.0.0.0/8 (This host)
            if octets[0] == 0 {
                return true;
            }
            // 127.0.0.0/8 (Loopback)
            if v4.is_loopback() {
                return true;
            }
            // 10.0.0.0/8 (Private RFC 1918)
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12 (Private RFC 1918)
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16 (Private RFC 1918)
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            // 169.254.0.0/16 (Link Local / Cloud Metadata 169.254.169.254)
            if v4.is_link_local() || (octets[0] == 169 && octets[1] == 254) {
                return true;
            }
            // 100.64.0.0/10 (Carrier-Grade NAT)
            if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                return true;
            }
            // 192.0.0.0/24 (IETF Protocol)
            if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
                return true;
            }
            // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24 (TEST-NET)
            if (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
            {
                return true;
            }
            // 198.18.0.0/15 (Benchmarking)
            if octets[0] == 198 && (18..=19).contains(&octets[1]) {
                return true;
            }
            // 224.0.0.0/4 (Multicast)
            if v4.is_multicast() || octets[0] >= 224 {
                return true;
            }
            // 255.255.255.255/32 (Broadcast)
            if v4.is_broadcast() {
                return true;
            }
            false
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            let segments = v6.segments();
            // IPv4-mapped IPv6: ::ffff:a.b.c.d
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_or_restricted_ip(&std::net::IpAddr::V4(v4));
            }
            // Unique Local Address (fc00::/7)
            if (segments[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // Link-local unicast (fe80::/10)
            if (segments[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // Documentation (2001:db8::/32)
            if segments[0] == 0x2001 && segments[1] == 0x0db8 {
                return true;
            }
            false
        }
    }
}

/// Atomically writes bytes or string content to a file by writing to a sibling temporary file first
/// and then renaming it over the destination path. This guarantees atomic, torn-read-free file updates.
pub async fn atomic_write_file<P: AsRef<std::path::Path>, C: AsRef<[u8]>>(
    file_path: P,
    content: C,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let file_path = file_path.as_ref();
    let parent = file_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    tokio::fs::create_dir_all(parent).await?;

    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let tmp_path = parent.join(format!(".{}.{}_{}.tmp", filename, nanos, pid));

    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
        .await
    {
        Ok(f) => f,
        Err(e) => return Err(e),
    };

    if let Err(e) = file.write_all(content.as_ref()).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    if let Err(e) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    drop(file);

    if let Err(e) = tokio::fs::rename(&tmp_path, file_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            extract_domain("https://example.com/path"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_domain("http://api.github.com:443/v1"),
            Some("api.github.com".to_string())
        );
        assert_eq!(extract_domain("https://"), None);
    }

    #[test]
    fn test_extract_binaries_simple() {
        assert_eq!(extract_command_binaries("ls"), vec!["ls"]);
        assert_eq!(extract_command_binaries("/usr/bin/ls -la"), vec!["ls"]);
    }

    #[test]
    fn test_extract_binaries_pipeline() {
        assert_eq!(
            extract_command_binaries("cat file | grep foo | wc -l"),
            vec!["cat", "grep", "wc"]
        );
    }

    #[test]
    fn test_extract_binaries_chained() {
        assert_eq!(
            extract_command_binaries("make && make test"),
            vec!["make", "make"]
        );
        assert_eq!(
            extract_command_binaries("cmd1 ; cmd2 || cmd3"),
            vec!["cmd1", "cmd2", "cmd3"]
        );
    }

    #[test]
    fn test_extract_binaries_quoted_pipes() {
        // Pipes inside double quotes should NOT split
        assert_eq!(
            extract_command_binaries(r#"grep -r "todo!\|unimplemented!" src/"#),
            vec!["grep"]
        );
        // Pipes inside single quotes should NOT split
        assert_eq!(
            extract_command_binaries("grep 'a|b|c' file.txt"),
            vec!["grep"]
        );
    }

    #[test]
    fn test_extract_binaries_mixed_quotes_and_pipes() {
        // Real pipe after quoted argument
        assert_eq!(
            extract_command_binaries(r#"grep "pattern" file | wc -l"#),
            vec!["grep", "wc"]
        );
    }

    #[test]
    fn test_extract_binaries_escaped_pipe() {
        // Backslash-escaped pipe inside double quotes (common in grep)
        assert_eq!(
            extract_command_binaries(
                r#"grep -r "todo!\|unimplemented!\|TODO\|FIXME" src/ --include="*.rs" -l"#
            ),
            vec!["grep"]
        );
    }

    #[test]
    fn test_extract_binaries_empty() {
        assert_eq!(extract_command_binaries(""), Vec::<String>::new());
        assert_eq!(extract_command_binaries("   "), Vec::<String>::new());
    }

    #[test]
    fn test_extract_binaries_redirect_not_separator() {
        // 2>&1 should NOT split on &
        assert_eq!(
            extract_command_binaries("cargo build 2>&1 | head -50"),
            vec!["cargo", "head"]
        );
        // &> is redirect, not separator
        assert_eq!(extract_command_binaries("make &> /dev/null"), vec!["make"]);
        // Multiple redirects
        assert_eq!(
            extract_command_binaries("cmd 2>&1 1>/dev/null"),
            vec!["cmd"]
        );
    }

    #[test]
    fn test_extract_binaries_background_ampersand() {
        // Single & at end is background, should split
        assert_eq!(
            extract_command_binaries("sleep 10 & echo done"),
            vec!["sleep", "echo"]
        );
    }

    #[test]
    fn test_extract_binaries_complex_real_world() {
        // Real-world: build + test with redirect
        assert_eq!(
            extract_command_binaries("cargo build 2>&1 && cargo test 2>&1 | tail -20"),
            vec!["cargo", "cargo", "tail"]
        );
        // grep with complex pattern + pipe
        assert_eq!(
            extract_command_binaries(r#"grep -rn "TODO\|FIXME\|HACK" src/ | sort | uniq -c"#),
            vec!["grep", "sort", "uniq"]
        );
        // Subshell-like: semicolons + pipes
        assert_eq!(
            extract_command_binaries("echo start ; ls -la | grep rs ; echo done"),
            vec!["echo", "ls", "grep", "echo"]
        );
    }

    #[test]
    fn test_extract_binaries_nested_quotes() {
        // Single quotes inside double quotes
        assert_eq!(
            extract_command_binaries(r#"echo "it's a pipe | not""#),
            vec!["echo"]
        );
        // Double quotes inside single quotes
        assert_eq!(
            extract_command_binaries(r#"echo 'he said "hello | world"'"#),
            vec!["echo"]
        );
    }

    #[test]
    fn test_extract_binaries_pub_matches_internal() {
        // Ensure the pub wrapper returns same results as internal fn
        assert_eq!(
            extract_command_binaries_pub("cargo build 2>&1 | head -50"),
            vec!["cargo", "head"]
        );
        assert_eq!(
            extract_command_binaries_pub(r#"grep "a|b" file | wc -l"#),
            vec!["grep", "wc"]
        );
    }

    #[test]
    fn test_extract_binaries_pipeline_all_must_be_checked() {
        // A pipeline where only the first cmd is allowed should still
        // extract ALL binaries -- the caller must check each one
        let bins = extract_command_binaries("allowed_cmd | not_allowed | also_not");
        assert_eq!(bins, vec!["allowed_cmd", "not_allowed", "also_not"]);
    }

    #[test]
    fn test_extract_binaries_here_string() {
        // <<< here-string should not confuse the parser
        assert_eq!(extract_command_binaries("cat <<< hello"), vec!["cat"]);
    }

    #[test]
    fn test_extract_binaries_env_prefix() {
        // env var prefix before command should be skipped
        assert_eq!(extract_command_binaries("FOO=bar baz"), vec!["baz"]);
        assert_eq!(
            extract_command_binaries("FOO=bar BAR=baz /usr/bin/ls -la"),
            vec!["ls"]
        );
    }

    #[test]
    fn test_extract_binaries_subshell_marker() {
        assert_eq!(
            extract_command_binaries("git clone repo || (rm -rf /tmp/foo)"),
            vec!["git", "rm"]
        );
    }

    #[test]
    fn test_normalize_path_basic() {
        assert_eq!(
            ToolRegistry::normalize_path("/home/u/project/."),
            "/home/u/project"
        );
        assert_eq!(
            ToolRegistry::normalize_path("/home/u/project/./src"),
            "/home/u/project/src"
        );
        assert_eq!(
            ToolRegistry::normalize_path("/home/u/project/../other"),
            "/home/u/other"
        );
        assert_eq!(
            ToolRegistry::normalize_path("/home/u/./project/"),
            "/home/u/project"
        );
        assert_eq!(ToolRegistry::normalize_path("/"), "/");
    }

    #[test]
    fn test_is_path_in_list_relative_cwd() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let registry = ToolRegistry::new(vec![]);
        let list = vec![cwd.clone()];
        assert!(registry.is_path_in_list("test.md", &list));
        assert!(registry.is_path_in_list("subdir/file.txt", &list));
    }

    #[test]
    fn test_is_path_in_list_dot_dir() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let allowed = format!("{}/.", cwd);
        let registry = ToolRegistry::new(vec![]);
        let list = vec![allowed];
        assert!(registry.is_path_in_list("myfile.rs", &list));
    }

    #[test]
    fn test_is_path_in_list_absolute_match() {
        let registry = ToolRegistry::new(vec![]);
        let list = vec!["/tmp".to_string()];
        assert!(registry.is_path_in_list("/tmp/test.txt", &list));
        assert!(!registry.is_path_in_list("/home/other.txt", &list));
    }

    #[test]
    fn test_is_path_in_list_no_partial_prefix() {
        let registry = ToolRegistry::new(vec![]);
        let list = vec!["/tmp".to_string()];
        assert!(!registry.is_path_in_list("/tmpfoo/file.txt", &list));
    }

    #[test]
    fn test_resolve_path_relative() {
        let registry = ToolRegistry::new(vec![]);
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let resolved = registry.resolve_path("hello.txt");
        assert_eq!(resolved, format!("{}/hello.txt", cwd));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let registry = ToolRegistry::new(vec![]);
        assert_eq!(registry.resolve_path("/usr/bin/test"), "/usr/bin/test");
    }

    #[test]
    fn test_resolve_path_dotdot() {
        let registry = ToolRegistry::new(vec![]);
        assert_eq!(
            registry.resolve_path("/home/u/project/../other/file.txt"),
            "/home/u/other/file.txt"
        );
    }

    #[test]
    fn test_is_file_in_list_exact_match() {
        let registry = ToolRegistry::new(vec![]);
        let list = vec!["/home/user/.netrc".to_string(), "/etc/hosts".to_string()];
        assert!(registry.is_file_in_list("/home/user/.netrc", &list));
        assert!(registry.is_file_in_list("/etc/hosts", &list));
    }

    #[test]
    fn test_is_file_in_list_no_prefix_match() {
        // File list should NOT do prefix matching like path list
        let registry = ToolRegistry::new(vec![]);
        let list = vec!["/home/user/.netrc".to_string()];
        assert!(!registry.is_file_in_list("/home/user/.netrc.bak", &list));
        assert!(!registry.is_file_in_list("/home/user/.netrcfoo", &list));
    }

    #[test]
    fn test_is_file_in_list_empty() {
        let registry = ToolRegistry::new(vec![]);
        let list: Vec<String> = vec![];
        assert!(!registry.is_file_in_list("/any/path", &list));
    }

    #[test]
    fn test_is_file_in_list_relative_resolves() {
        // Relative path should be resolved before matching
        let registry = ToolRegistry::new(vec![]);
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let list = vec![format!("{}/myfile.txt", cwd)];
        assert!(registry.is_file_in_list("myfile.txt", &list));
    }

    #[test]
    fn test_sandbox_includes_allowed_files_ro() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.policy_allowed_files_ro = vec!["/home/user/.netrc".to_string()];
        registry.policy_allowed_paths_ro =
            vec!["/bin".to_string(), "/usr".to_string(), "/lib".to_string()];
        registry.policy_mode = "confirm".to_string();

        // The sandbox() method is private, so we test via the public interface
        // Verify that the files are stored in the registry
        assert!(registry
            .policy_allowed_files_ro
            .contains(&"/home/user/.netrc".to_string()));
    }

    #[test]
    fn test_sandbox_includes_allowed_files_rw() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.policy_allowed_files_rw = vec!["/tmp/data.json".to_string()];
        registry.policy_mode = "confirm".to_string();

        assert!(registry
            .policy_allowed_files_rw
            .contains(&"/tmp/data.json".to_string()));
    }

    #[test]
    fn test_add_allowed_file_ro() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.add_allowed_file_ro("/home/user/.netrc");
        registry.add_allowed_file_ro("/home/user/.netrc"); // duplicate
        assert_eq!(registry.policy_allowed_files_ro.len(), 1);
        assert_eq!(registry.policy_allowed_files_ro[0], "/home/user/.netrc");
    }

    #[test]
    fn test_write_file_allowed_by_file_list() {
        let mut registry = ToolRegistry::new(vec![]);
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        registry.policy_mode = "allowlist".to_string();
        registry.policy_allowed_paths_rw = vec![]; // empty - would normally block
        registry.policy_allowed_files_rw = vec![format!("{}/special.txt", cwd)];

        // is_file_in_list should match
        assert!(registry.is_file_in_list("special.txt", &registry.policy_allowed_files_rw.clone()));
    }

    #[test]
    fn test_file_parents_go_to_traverse_not_readonly() {
        // When allowed_files_ro has a file, its parent should go to
        // traverse_paths, not read_only_paths
        let mut registry = ToolRegistry::new(vec![]);
        registry.policy_allowed_files_ro = vec!["/home/user/.netrc".to_string()];
        registry.policy_allowed_paths_ro = vec!["/bin".to_string(), "/usr".to_string()];
        registry.policy_mode = "confirm".to_string();

        // Verify the file itself is in files_ro
        assert!(registry
            .policy_allowed_files_ro
            .contains(&"/home/user/.netrc".to_string()));
        // Verify parent /home/user is NOT in allowed_paths_ro
        // (it should go to traverse_paths, verified at sandbox build time)
        assert!(!registry
            .policy_allowed_paths_ro
            .contains(&"/home/user".to_string()));
    }

    #[test]
    fn test_file_parents_deduplication() {
        // Multiple files in same directory should only produce one traverse entry
        let mut registry = ToolRegistry::new(vec![]);
        registry.policy_allowed_files_ro = vec![
            "/home/user/.netrc".to_string(),
            "/home/user/.config".to_string(),
        ];
        registry.policy_allowed_paths_ro = vec!["/bin".to_string()];
        registry.policy_mode = "confirm".to_string();

        // Both files have same parent /home/user
        let parent = std::path::Path::new("/home/user/.netrc")
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let parent2 = std::path::Path::new("/home/user/.config")
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(parent, parent2);
        assert_eq!(parent, "/home/user");
    }

    #[test]
    fn test_file_parent_not_added_if_already_in_ro() {
        // If the parent directory is already in read_only_paths, don't duplicate in traverse
        let mut registry = ToolRegistry::new(vec![]);
        registry.policy_allowed_files_ro = vec!["/usr/share/data.txt".to_string()];
        registry.policy_allowed_paths_ro = vec!["/usr".to_string()];
        registry.policy_mode = "confirm".to_string();

        // /usr/share's parent is /usr, which is already in ro
        // The sandbox builder should detect this and skip adding to traverse
        let parent = std::path::Path::new("/usr/share/data.txt")
            .parent()
            .unwrap();
        let already_covered = registry
            .policy_allowed_paths_ro
            .iter()
            .any(|p| parent.starts_with(p));
        assert!(already_covered);
    }

    #[test]
    fn test_sandbox_path_includes_allowed_paths_ro() {
        // Simulate the PATH building logic from sandboxed_cmd
        let policy_allowed_paths_ro = vec!["/home/u/bin".to_string(), "/opt/tools".to_string()];
        let system_path = "/usr/local/bin:/usr/bin:/bin".to_string();

        let mut extra_paths: Vec<String> = Vec::new();
        for p in &policy_allowed_paths_ro {
            if !system_path.contains(p.as_str()) {
                extra_paths.push(p.clone());
            }
        }

        assert_eq!(extra_paths.len(), 2);
        assert!(extra_paths.contains(&"/home/u/bin".to_string()));
        assert!(extra_paths.contains(&"/opt/tools".to_string()));

        extra_paths.push(system_path.clone());
        let final_path = extra_paths.join(":");
        assert!(final_path.starts_with("/home/u/bin:"));
        assert!(final_path.contains("/opt/tools:"));
        assert!(final_path.ends_with("/usr/local/bin:/usr/bin:/bin"));
    }

    #[test]
    fn test_sandbox_path_no_duplicates_with_system() {
        // If an allowed_path is already in system PATH, don't add it again
        let policy_allowed_paths_ro = vec!["/usr/bin".to_string(), "/home/u/bin".to_string()];
        let system_path = "/usr/local/bin:/usr/bin:/bin".to_string();

        let mut extra_paths: Vec<String> = Vec::new();
        for p in &policy_allowed_paths_ro {
            if !system_path.contains(p.as_str()) {
                extra_paths.push(p.clone());
            }
        }

        // /usr/bin is already in system_path, so only /home/u/bin should be added
        assert_eq!(extra_paths.len(), 1);
        assert_eq!(extra_paths[0], "/home/u/bin");
    }

    #[test]
    fn test_sandbox_path_empty_when_no_extra_dirs() {
        // When all allowed paths are already in system PATH, env_map should be empty
        let policy_allowed_paths_ro = vec!["/usr/bin".to_string()];
        let system_path = "/usr/local/bin:/usr/bin:/bin".to_string();

        let mut extra_paths: Vec<String> = Vec::new();
        for p in &policy_allowed_paths_ro {
            if !system_path.contains(p.as_str()) {
                extra_paths.push(p.clone());
            }
        }

        assert!(extra_paths.is_empty());
    }

    /// Ensure the tool schema exposed to the LLM never contains legacy read_spec/edit_spec names.
    /// This test prevents regression of the rename to list_markdown/read_markdown/write_markdown.
    #[test]
    fn test_tool_schema_no_legacy_spec_tools() {
        let registry = ToolRegistry::new(vec![]);
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            !schema.contains("read_spec"),
            "tool schema still contains legacy 'read_spec'"
        );
        assert!(
            !schema.contains("edit_spec"),
            "tool schema still contains legacy 'edit_spec'"
        );
    }

    /// Ensure the new markdown tools are present in the schema (serve mode only).
    #[test]
    fn test_tool_schema_has_markdown_tools() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_serve_mode(true);
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            schema.contains("list_markdown"),
            "tool schema missing 'list_markdown'"
        );
        assert!(
            schema.contains("read_markdown"),
            "tool schema missing 'read_markdown'"
        );
        assert!(
            schema.contains("write_markdown"),
            "tool schema missing 'write_markdown'"
        );
    }

    #[test]
    fn test_tool_schema_has_search_chat() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_serve_mode(true);
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            schema.contains("search_chat"),
            "tool schema missing 'search_chat'"
        );
        assert!(
            schema.contains("\"query\""),
            "search_chat schema missing 'query' param"
        );
    }

    /// CLI mode (serve_mode=false) should NOT expose serve-only tools.
    #[test]
    fn test_cli_mode_no_serve_tools() {
        let registry = ToolRegistry::new(vec![]);
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            !schema.contains("search_chat"),
            "CLI mode should not have search_chat"
        );
        assert!(
            !schema.contains("list_markdown"),
            "CLI mode should not have list_markdown"
        );
        assert!(
            !schema.contains("read_markdown"),
            "CLI mode should not have read_markdown"
        );
        assert!(
            !schema.contains("write_markdown"),
            "CLI mode should not have write_markdown"
        );
    }

    #[test]
    fn test_serve_mode_default_no_cli_tools() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_serve_mode(true);
        // agent_skills is false by default
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            !schema.contains("read_file"),
            "serve mode should not have read_file by default"
        );
        assert!(
            !schema.contains("write_file"),
            "serve mode should not have write_file by default"
        );
        assert!(
            !schema.contains("execute_cmd"),
            "serve mode should not have execute_cmd by default"
        );
        assert!(
            !schema.contains("fetch_url"),
            "serve mode should not have fetch_url by default"
        );
        assert!(
            schema.contains("list_markdown"),
            "serve mode should have list_markdown"
        );
    }

    #[test]
    fn test_serve_mode_with_allowed_tools_has_cli_tools() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_serve_mode(true);
        registry.set_allowed_tools(vec!["read_file".to_string(), "fetch_url".to_string()]);
        let schema = serde_json::to_string(&registry.tool_definitions()).unwrap();
        assert!(
            schema.contains("read_file"),
            "allowed_tools should enable read_file in serve mode"
        );
        assert!(
            schema.contains("fetch_url"),
            "allowed_tools should enable fetch_url in serve mode"
        );
        assert!(
            !schema.contains("write_file"),
            "unallowed tool write_file should not be in schema"
        );
        assert!(
            schema.contains("list_markdown"),
            "serve mode should keep list_markdown"
        );
    }

    #[tokio::test]
    async fn test_serve_mode_rejects_unallowed_tools() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_serve_mode(true);
        // policy_allowed_tools is empty by default
        let out = registry
            .execute("execute_cmd", serde_json::json!({"cmd": "echo hi"}))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("not in allowed_tools"));
    }

    #[test]
    fn test_cli_allowed_tools_filtering() {
        let mut registry = ToolRegistry::new(vec![]);
        // default: empty
        let defs_empty = registry.tool_definitions();
        assert!(
            defs_empty.is_empty(),
            "default allowed_tools should produce 0 tools"
        );

        // allow specific
        registry.set_allowed_tools(vec!["fetch_url".to_string()]);
        let defs_partial = registry.tool_definitions();
        assert_eq!(defs_partial.len(), 1);
        let schema = serde_json::to_string(&defs_partial).unwrap();
        assert!(schema.contains("fetch_url"));
        assert!(!schema.contains("read_file"));

        // allow wildcard
        registry.set_allowed_tools(vec!["*".to_string()]);
        let defs_all = registry.tool_definitions();
        assert_eq!(defs_all.len(), 5);
    }

    #[test]
    fn test_html_to_markdown_purification() {
        let html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Demo</title><style>body { font: red; }</style></head>
            <body>
                <script>console.log("noisy script");</script>
                <nav><a href="/home">Home</a></nav>
                <h1>Main Heading</h1>
                <p>Hello <b>World</b>! Here is a <a href="https://example.com">link</a>.</p>
                <table>
                    <tr><th>ID</th><th>Symbol</th></tr>
                    <tr><td>2330</td><td>TSMC</td></tr>
                </table>
                <footer>Copyright 2026</footer>
            </body>
            </html>
        "#;
        let md = html_to_markdown(html);
        assert!(!md.contains("noisy script"));
        assert!(!md.contains("font: red"));
        assert!(!md.contains("Copyright 2026"));
        assert!(md.contains("# Main Heading"));
        assert!(md.contains("Hello **World**!"));
        assert!(md.contains("[link](https://example.com)"));
        assert!(md.contains("| ID | Symbol |"));
        assert!(md.contains("| 2330 | TSMC |"));
    }

    #[tokio::test]
    async fn test_fetch_url_save_to_denied_path() {
        let mut registry = ToolRegistry::new(vec![]);
        let mut policy = crate::config::PolicyConfig::default();
        policy.denied_paths = vec!["/etc/shadow".to_string()];
        policy.allowed_tools = vec!["fetch_url".to_string()];
        policy.allowed_domains = vec!["example.com".to_string()];
        registry.set_policy(&policy);

        let out = registry
            .fetch_url(serde_json::json!({
                "url": "https://example.com",
                "save_to": "/etc/shadow"
            }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("BLOCKED: cannot save to denied path"));
    }

    #[test]
    fn test_mount_pwd_policy_in_tool_registry() {
        let mut registry = ToolRegistry::new(vec![]);
        let mut policy = crate::config::PolicyConfig::default();
        assert!(!registry.mount_pwd);

        policy.mount_pwd = true;
        registry.set_policy(&policy);
        assert!(registry.mount_pwd);
    }

    #[test]
    fn test_sandbox_includes_policy_mount_home() {
        let mut registry = ToolRegistry::new(vec![]);
        let mut policy = crate::config::PolicyConfig::default();
        policy.mount_home = Some("/tmp/test_home_dir".to_string());
        registry.set_policy(&policy);

        let executor = registry.sandbox(10);
        assert!(
            executor
                .config()
                .mount_home
                .as_ref()
                .map_or(false, |p| p.to_string_lossy().contains("test_home_dir")),
            "SandboxConfig mount_home should match policy.mount_home"
        );
        assert!(
            executor
                .config()
                .read_write_paths
                .iter()
                .any(|p| p.to_string_lossy().contains("test_home_dir")),
            "Landlock read_write_paths should include policy.mount_home"
        );
    }

    #[tokio::test]
    async fn test_policy_mount_home_sets_effective_cwd_to_home() {
        let temp_home = tempfile::tempdir_in("/var/tmp").expect("tempdir");
        let mut registry = ToolRegistry::new(vec![]);
        let mut policy = crate::config::PolicyConfig::default();
        policy.mount_home = Some(temp_home.path().to_string_lossy().to_string());
        policy.mode = "unrestricted".to_string();
        registry.set_policy(&policy);

        let res = registry
            .execute_cmd(serde_json::json!({
                "cmd": "pwd"
            }))
            .await;
        assert_eq!(res.is_error, false, "Execution failed: {}", res.content);
        let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        assert!(
            res.content.contains(&real_home),
            "Expected pwd to be real HOME ({}), got: {}",
            real_home,
            res.content
        );
    }

    #[test]
    fn test_session_tmp_lifecycle() {
        let path = {
            let session = SessionTmp::new();
            let path = session.path().to_path_buf();
            assert!(
                path.exists(),
                "Session tmp directory should exist upon creation"
            );
            path
        };
        assert!(
            !path.exists(),
            "Session tmp directory should be removed after drop"
        );
    }

    #[test]
    fn test_tool_registry_creates_session_tmp() {
        let registry = ToolRegistry::new(vec![]);
        let executor = registry.sandbox(10);
        assert!(
            executor.config().session_tmp_dir.is_some(),
            "ToolRegistry should populate session_tmp_dir in SandboxConfig"
        );
    }

    #[tokio::test]
    async fn test_session_tmp_cross_tool_persistence_in_sandbox() {
        let mut registry = ToolRegistry::new(vec![]);
        let mut policy = crate::config::PolicyConfig::default();
        policy.mode = "unrestricted".to_string();
        registry.set_policy(&policy);

        // Tool 1: write to /tmp/test_session_file.txt inside sandbox
        let res1 = registry
            .execute_cmd(serde_json::json!({
                "cmd": "echo 'session_data_123' > /tmp/test_session_file.txt"
            }))
            .await;
        assert_eq!(
            res1.is_error, false,
            "Tool 1 execution failed: {}",
            res1.content
        );

        // Tool 2: read /tmp/test_session_file.txt in a subsequent tool invocation
        let res2 = registry
            .execute_cmd(serde_json::json!({
                "cmd": "cat /tmp/test_session_file.txt"
            }))
            .await;
        assert_eq!(
            res2.is_error, false,
            "Tool 2 execution failed: {}",
            res2.content
        );
        assert!(
            res2.content.contains("session_data_123"),
            "Expected /tmp content to persist across tool calls within session, got: {}",
            res2.content
        );
    }

    #[tokio::test]
    async fn test_tool_parameter_aliases_read_write_list() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();

        let mut registry = ToolRegistry::new(vec![tmp_path.clone()]);
        let mut policy = crate::config::PolicyConfig::default();
        policy.mode = "unrestricted".to_string();
        registry.set_policy(&policy);

        let file_path = tmp_path.join("alias_test.txt");
        let file_str = file_path.to_string_lossy().to_string();

        // write_file using "filename" alias
        let w_res = registry
            .execute(
                "write_file",
                serde_json::json!({
                    "filename": file_str,
                    "content": "hello alias"
                }),
            )
            .await;
        assert_eq!(
            w_res.is_error, false,
            "write_file failed: {}",
            w_res.content
        );

        // read_file using "filename" alias and extra "path" key
        let r_res = registry
            .execute(
                "read_file",
                serde_json::json!({
                    "filename": file_str,
                    "path": file_str
                }),
            )
            .await;
        assert_eq!(r_res.is_error, false, "read_file failed: {}", r_res.content);
        assert!(r_res.content.contains("hello alias"));

        // list_dir using "dir" alias
        let l_res = registry
            .execute(
                "list_dir",
                serde_json::json!({
                    "dir": tmp_path.to_string_lossy().to_string()
                }),
            )
            .await;
        assert_eq!(l_res.is_error, false, "list_dir failed: {}", l_res.content);
        assert!(l_res.content.contains("alias_test.txt"));
    }

    #[test]
    fn test_is_private_or_restricted_ip() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        // Private / Loopback / Restricted IPv4
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            127, 0, 0, 1
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            10, 0, 0, 1
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            172, 16, 0, 1
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            172, 31, 255, 255
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            0, 0, 0, 0
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            224, 0, 0, 1
        ))));
        assert!(is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            255, 255, 255, 255
        ))));

        // Public IPv4
        assert!(!is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
        assert!(!is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            1, 1, 1, 1
        ))));
        assert!(!is_private_or_restricted_ip(&IpAddr::V4(Ipv4Addr::new(
            203, 66, 1, 1
        ))));

        // IPv6
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            Ipv6Addr::LOCALHOST
        )));
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            Ipv6Addr::UNSPECIFIED
        )));
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            "fc00::1".parse().unwrap()
        )));
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            "fe80::1".parse().unwrap()
        )));
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            "::ffff:127.0.0.1".parse().unwrap()
        )));
        assert!(is_private_or_restricted_ip(&IpAddr::V6(
            "::ffff:10.0.0.1".parse().unwrap()
        )));
        assert!(!is_private_or_restricted_ip(&IpAddr::V6(
            "::ffff:8.8.8.8".parse().unwrap()
        )));
        assert!(!is_private_or_restricted_ip(&IpAddr::V6(
            "2606:4700:4700::1111".parse().unwrap()
        )));
    }

    #[tokio::test]
    async fn test_fetch_url_policy_checks() {
        let mut registry = ToolRegistry::new(vec![]);
        registry.set_allowed_domains(vec!["example.com".to_string()]);

        // Invalid scheme
        let res = registry
            .fetch_url(serde_json::json!({
                "url": "file:///etc/passwd"
            }))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("must start with http:// or https://"));

        // Unauthorized domain
        let res = registry
            .fetch_url(serde_json::json!({
                "url": "https://unauthorized.org/data"
            }))
            .await;
        assert!(res.is_error);
        assert!(res
            .content
            .contains("BLOCKED: domain 'unauthorized.org' is not in allowed_domains"));

        // SSRF attempt to local metadata
        let mut ssrf_registry = ToolRegistry::new(vec![]);
        ssrf_registry.set_allowed_domains(vec!["169.254.169.254".to_string()]);
        let res = ssrf_registry
            .fetch_url(serde_json::json!({
                "url": "http://169.254.169.254/latest/meta-data"
            }))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("SSRF protection"));
    }
}
