use serde::Serialize;
use std::path::PathBuf;
use tracing::info;

use crate::sandbox::{SandboxConfig, SandboxExecutor};

const MAX_FILE_SIZE: usize = 32 * 1024; // 32KB

/// Tool execution result.
#[derive(Debug, Serialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Tool registry — all tools execute through the sandbox.
pub struct ToolRegistry {
    policy_mode: String,
    policy_allowed_commands: Vec<String>,
    policy_denied_syscalls: Vec<String>,
    policy_denied_paths: Vec<String>,
    policy_allowed_paths_rw: Vec<String>,
    policy_allowed_paths_ro: Vec<String>,
    allowed_dirs: Vec<PathBuf>,
    allowed_domains: Vec<String>,
}

impl ToolRegistry {
    pub fn new(allowed_dirs: Vec<PathBuf>) -> Self {
        Self {
            allowed_dirs,
            allowed_domains: Vec::new(),
            policy_mode: "confirm".to_string(),
            policy_allowed_commands: Vec::new(),
            policy_denied_syscalls: Vec::new(),
            policy_denied_paths: Vec::new(),
            policy_allowed_paths_rw: Vec::new(),
            policy_allowed_paths_ro: Vec::new(),
        }
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

    /// Set command execution policy.
    pub fn set_policy(&mut self, policy: &crate::config::PolicyConfig) {
        self.policy_mode = policy.mode.clone();
        self.policy_allowed_commands = policy.allowed_commands.clone();
        self.policy_denied_syscalls = policy.denied_syscalls.clone();
        self.policy_denied_paths = policy.denied_paths.clone();
        self.policy_allowed_paths_rw = policy.allowed_paths_rw.clone();
        self.policy_allowed_paths_ro = policy.allowed_paths_ro.clone();
        self.allowed_domains = policy.allowed_domains.clone();
    }

    /// Create a sandbox executor with the registry's config.
    fn sandbox(&self, timeout_secs: u64, block_network: bool) -> SandboxExecutor {
        // Unrestricted mode: minimal sandbox (timeout only, no filesystem/network restrictions)
        if self.policy_mode == "unrestricted" {
            let config = SandboxConfig {
                timeout_secs,
                read_write_paths: vec![PathBuf::from("/")],
                read_only_paths: vec![],
                denied_paths: vec![],
                allowed_domains: vec!["*".to_string()],
                ..SandboxConfig::default()
            };
            return SandboxExecutor::new(config);
        }
        let config = SandboxConfig {
            timeout_secs,
            read_write_paths: self.allowed_dirs.clone(),
            read_only_paths: self
                .policy_allowed_paths_ro
                .iter()
                .map(PathBuf::from)
                .collect(),
            allowed_domains: self.allowed_domains.clone(),
            block_network,
            ..SandboxConfig::default()
        };
        SandboxExecutor::new(config)
    }

    /// Run a command in sandbox and return ToolOutput.
    async fn sandboxed_cmd(&self, cmd: &str, timeout_secs: u64, cwd: Option<&str>, block_network: bool) -> ToolOutput {
        let executor = self.sandbox(timeout_secs, block_network);
        match executor.run_shell_command(cmd, cwd, None).await {
            Ok(result) => {
                if result.timed_out {
                    return ToolOutput::err(format!("command timed out after {}s", timeout_secs));
                }
                let output = if !result.stderr.is_empty() && result.exit_code != 0 {
                    format!("{}\n{}", result.stdout, result.stderr)
                } else {
                    result.stdout.clone()
                };
                if result.exit_code != 0 {
                    ToolOutput::err(format!(
                        "exit_code: {}\nstdout: {}\nstderr: {}",
                        result.exit_code, result.stdout, result.stderr
                    ))
                } else {
                    ToolOutput::ok(output)
                }
            }
            Err(e) => ToolOutput::err(format!("sandbox error: {}", e)),
        }
    }

    /// Dispatch a tool call by name.
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> ToolOutput {
        info!(tool = name, "executing tool (sandboxed)");
        match name {
            "read_file" => self.read_file(args).await,
            "write_file" => self.write_file(args).await,
            "list_dir" => self.list_dir(args).await,
            "execute_cmd" => self.execute_cmd(args).await,
            "fetch_url" => self.fetch_url(args).await,
            "inspect_process" => self.inspect_process(args).await,
            other => ToolOutput::err(format!("unknown tool: {}", other)),
        }
    }

    /// Return tool definitions as JSON (for LLM function calling schema).
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
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
            }),
            serde_json::json!({
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
            }),
            serde_json::json!({
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
            }),
            serde_json::json!({
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
            }),
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "fetch_url",
                    "description": "Fetch content from a URL (sandboxed, requires domain in allowlist).",
                    "parameters": {
                        "type": "object",
                        "properties": { "url": { "type": "string" } },
                        "required": ["url"]
                    }
                }
            }),
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "inspect_process",
                    "description": "Inspect a running process by PID (sandboxed).",
                    "parameters": {
                        "type": "object",
                        "properties": { "pid": { "type": "integer" } },
                        "required": ["pid"]
                    }
                }
            }),
        ]
    }

    // ── All tools go through sandbox ─────────────────────────────────

    async fn read_file(&self, args: serde_json::Value) -> ToolOutput {
        let path = match args.get("path").and_then(|v| v.as_str()) {
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
        let result = self.sandboxed_cmd(&cmd, 10, None, false).await;
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
        let path = match args.get("path").and_then(|v| v.as_str()) {
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
        let result = self.sandboxed_cmd(&cmd, 10, None, false).await;
        if result.is_error {
            return result;
        }
        ToolOutput::ok(format!("Written {} bytes to {}", content.len(), path))
    }

    async fn list_dir(&self, args: serde_json::Value) -> ToolOutput {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        info!(path = %path, "list_dir (sandboxed)");
        let cmd = format!("ls -1F '{}'", path.replace('\'', "'\\''"));
        self.sandboxed_cmd(&cmd, 10, None, false).await
    }

    async fn execute_cmd(&self, args: serde_json::Value) -> ToolOutput {
        let cmd = match args.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return ToolOutput::err("missing required argument: cmd"),
        };
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        info!(cmd = %cmd, ?cwd, timeout_secs, "execute_cmd (sandboxed)");

        // Command policy enforcement: check ALL commands in the pipeline
        // Applies in both "allowlist" and "confirm" modes when allowed_commands is set
        // Skipped entirely in "unrestricted" mode
        if self.policy_mode != "unrestricted"
            && !self.policy_allowed_commands.is_empty()
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
        self.sandboxed_cmd(&cmd, timeout_secs, cwd.as_deref(), true).await
    }

    async fn fetch_url(&self, args: serde_json::Value) -> ToolOutput {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return ToolOutput::err("missing required argument: url"),
        };

        // Check domain allowlist (skipped in unrestricted mode)
        if self.policy_mode != "unrestricted" {
            if let Some(domain) = extract_domain(url) {
                let executor = self.sandbox(35, false);
                if !executor.is_domain_allowed(&domain) {
                    return ToolOutput::err(format!(
                        "BLOCKED: domain '{}' is not in allowed_domains. \
                     Network access requires explicit allowlist configuration.",
                        domain
                    ));
                }
            }
        } // end unrestricted check

        info!(url = %url, "fetch_url (sandboxed, domain allowed)");
        let cmd = format!("curl -sS -L --max-time 30 '{}'", url.replace('\'', "'\\''"));
        let result = self.sandboxed_cmd(&cmd, 35, None, false).await;
        if result.is_error {
            return result;
        }
        if result.content.len() > MAX_FILE_SIZE {
            ToolOutput::ok(format!(
                "{}\n[Content Truncated at 32KB]",
                &result.content[..MAX_FILE_SIZE]
            ))
        } else {
            result
        }
    }

    async fn inspect_process(&self, args: serde_json::Value) -> ToolOutput {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: pid"),
        };
        let cmd = format!("ps -p {} -o pid,comm,%cpu,%mem,stat,etime --no-headers 2>/dev/null || echo process_not_found", pid);
        self.sandboxed_cmd(&cmd, 5, None, false).await
    }
}

impl ToolRegistry {
    /// Check if a path is in the denied_paths list.
    fn is_path_denied(&self, path: &str) -> bool {
        if self.policy_mode == "unrestricted" {
            return false;
        }
        self.policy_denied_paths.iter().any(|d| path.starts_with(d))
    }

    /// Check if a path starts with any entry in a given list.
    fn is_path_in_list(&self, path: &str, list: &[String]) -> bool {
        list.iter().any(|p| path.starts_with(p))
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
        let first_token = trimmed.split_whitespace().next().unwrap_or("");
        let binary = first_token.rsplit('/').next().unwrap_or(first_token);
        if !binary.is_empty() {
            binaries.push(binary.to_string());
        }
    }
    binaries
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
        // env var prefix before command
        assert_eq!(
            extract_command_binaries("FOO=bar baz"),
            // Extracts FOO=bar as first token
            vec!["FOO=bar"]
        );
    }
}
