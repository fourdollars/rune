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
        Self { content: content.into(), is_error: false }
    }
    fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

/// Tool registry — all tools execute through the sandbox.
pub struct ToolRegistry {
    policy_mode: String,
    policy_allowed_commands: Vec<String>,
    policy_denied_syscalls: Vec<String>,
    allowed_dirs: Vec<PathBuf>,
    allowed_domains: Vec<String>,
}

impl ToolRegistry {
    pub fn new(allowed_dirs: Vec<PathBuf>) -> Self {
        Self { allowed_dirs, allowed_domains: Vec::new(), policy_mode: "confirm".to_string(), policy_allowed_commands: Vec::new(), policy_denied_syscalls: Vec::new() }
    }

    /// Set allowed network domains (for fetch_url / execute_cmd network access).
    pub fn set_allowed_domains(&mut self, domains: Vec<String>) {
        self.allowed_domains = domains;
    }

    /// Set command execution policy.
        pub fn set_policy(&mut self, policy: &crate::config::PolicyConfig) {
        self.policy_mode = policy.mode.clone();
        self.policy_allowed_commands = policy.allowed_commands.clone();
        self.policy_denied_syscalls = policy.denied_syscalls.clone();
        self.allowed_domains = policy.allowed_domains.clone();
    }

    /// Create a sandbox executor with the registry's config.
    fn sandbox(&self, timeout_secs: u64) -> SandboxExecutor {
        let config = SandboxConfig {
            timeout_secs,
            read_write_paths: self.allowed_dirs.clone(),
            allowed_domains: self.allowed_domains.clone(),
            ..SandboxConfig::default()
        };
        SandboxExecutor::new(config)
    }

    /// Run a command in sandbox and return ToolOutput.
    async fn sandboxed_cmd(&self, cmd: &str, timeout_secs: u64) -> ToolOutput {
        let executor = self.sandbox(timeout_secs);
        match executor.run_shell_command(cmd, None, None).await {
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
                    ToolOutput::err(format!("exit_code: {}\nstdout: {}\nstderr: {}", result.exit_code, result.stdout, result.stderr))
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
        info!(path = %path, "read_file (sandboxed)");
        let cmd = format!("head -c {} '{}'", MAX_FILE_SIZE, path.replace('\'', "'\\''"));
        let result = self.sandboxed_cmd(&cmd, 10).await;
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
        info!(path = %path, bytes = content.len(), "write_file (sandboxed)");
        let escaped_path = path.replace('\'', "'\\''");
        let escaped_content = content.replace('\'', "'\\''");
        let cmd = format!(
            "mkdir -p $(dirname '{}') && printf '%s' '{}' > '{}'",
            escaped_path, escaped_content, escaped_path
        );
        let result = self.sandboxed_cmd(&cmd, 10).await;
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
        self.sandboxed_cmd(&cmd, 10).await
    }

    async fn execute_cmd(&self, args: serde_json::Value) -> ToolOutput {
        let cmd = match args.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return ToolOutput::err("missing required argument: cmd"),
        };
        let timeout_secs = args.get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        info!(cmd = %cmd, timeout_secs, "execute_cmd (sandboxed)");

        // Command policy enforcement
        if self.policy_mode == "allowlist" {
            let first_token = cmd.split_whitespace().next().unwrap_or("");
            let binary = first_token.rsplit("/").next().unwrap_or(first_token);
            if !self.policy_allowed_commands.iter().any(|a| a == binary || a == "*") {
                return ToolOutput::err(format!(
                    "BLOCKED by policy: command '{}' is not in allowed_commands. Policy: allowlist",
                    binary
                ));
            }
        }
        self.sandboxed_cmd(&cmd, timeout_secs).await
    }

    async fn fetch_url(&self, args: serde_json::Value) -> ToolOutput {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return ToolOutput::err("missing required argument: url"),
        };

        // Check domain allowlist
        if let Some(domain) = extract_domain(url) {
            let executor = self.sandbox(35);
            if !executor.is_domain_allowed(&domain) {
                return ToolOutput::err(format!(
                    "BLOCKED: domain '{}' is not in allowed_domains. \
                     Network access requires explicit allowlist configuration.",
                    domain
                ));
            }
        }

        info!(url = %url, "fetch_url (sandboxed, domain allowed)");
        let cmd = format!("curl -sS -L --max-time 30 '{}'", url.replace('\'', "'\\''"));
        let result = self.sandboxed_cmd(&cmd, 35).await;
        if result.is_error {
            return result;
        }
        if result.content.len() > MAX_FILE_SIZE {
            ToolOutput::ok(format!("{}\n[Content Truncated at 32KB]", &result.content[..MAX_FILE_SIZE]))
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
        self.sandboxed_cmd(&cmd, 5).await
    }
}

/// Extract domain from a URL string.
fn extract_domain(url: &str) -> Option<String> {
    // Simple extraction: strip scheme, take host part
    let without_scheme = url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next()?;
    let domain = host.split(':').next()?; // strip port
    if domain.is_empty() { None } else { Some(domain.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://example.com/path"), Some("example.com".to_string()));
        assert_eq!(extract_domain("http://api.github.com:443/v1"), Some("api.github.com".to_string()));
        assert_eq!(extract_domain("https://"), None);
    }
}
