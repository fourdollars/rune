use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::info;

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

/// Tool registry with sandbox path restrictions.
pub struct ToolRegistry {
    allowed_dirs: Vec<PathBuf>,
}

impl ToolRegistry {
    pub fn new(allowed_dirs: Vec<PathBuf>) -> Self {
        Self { allowed_dirs }
    }

    /// Dispatch a tool call by name.
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> ToolOutput {
        info!(tool = name, "executing tool");
        match name {
            "read_file" => self.read_file(args).await,
            "write_file" => self.write_file(args).await,
            "list_dir" => self.list_dir(args).await,
            "run_terminal_cmd" => self.run_terminal_cmd(args).await,
            "fetch_url" => self.fetch_url(args).await,
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
                    "description": "Read a file's contents. Truncates at 32KB.",
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
                    "description": "Write content to a file. Creates parent dirs.",
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
                    "description": "List directory contents.",
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
                    "name": "run_terminal_cmd",
                    "description": "Execute a shell command. Streams output to stderr in real-time.",
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
                    "description": "Fetch content from a URL (via curl).",
                    "parameters": {
                        "type": "object",
                        "properties": { "url": { "type": "string" } },
                        "required": ["url"]
                    }
                }
            }),
        ]
    }

    // ── Path validation ──────────────────────────────────────────────

    fn validate_path(&self, raw: &str) -> Result<PathBuf, String> {
        let p = Path::new(raw);
        // Canonicalize parent to resolve symlinks / ..
        let canonical = if p.exists() {
            p.canonicalize().map_err(|e| format!("canonicalize failed: {}", e))?
        } else {
            // For new files, canonicalize parent
            let parent = p.parent().ok_or("invalid path: no parent")?;
            if !parent.exists() {
                // Will be created by write_file; check the earliest existing ancestor
                let mut ancestor = parent.to_path_buf();
                while !ancestor.exists() {
                    ancestor = ancestor.parent()
                        .ok_or("invalid path: cannot resolve ancestor")?
                        .to_path_buf();
                }
                let resolved = ancestor.canonicalize()
                    .map_err(|e| format!("canonicalize ancestor failed: {}", e))?;
                resolved.join(p.strip_prefix(&ancestor).unwrap_or(p))
            } else {
                let resolved_parent = parent.canonicalize()
                    .map_err(|e| format!("canonicalize parent failed: {}", e))?;
                resolved_parent.join(p.file_name().unwrap_or_default())
            }
        };

        if self.allowed_dirs.is_empty() {
            return Ok(canonical);
        }

        for allowed in &self.allowed_dirs {
            if let Ok(allowed_canon) = allowed.canonicalize() {
                if canonical.starts_with(&allowed_canon) {
                    return Ok(canonical);
                }
            }
        }
        Err(format!("path '{}' is outside allowed directories", raw))
    }

    // ── Tools ────────────────────────────────────────────────────────

    async fn read_file(&self, args: serde_json::Value) -> ToolOutput {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        let path = match self.validate_path(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };
        info!(path = %path.display(), "read_file");
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                if content.len() > MAX_FILE_SIZE {
                    let truncated = &content[..MAX_FILE_SIZE];
                    ToolOutput::ok(format!("{}\n[Content Truncated at 32KB]", truncated))
                } else {
                    ToolOutput::ok(content)
                }
            }
            Err(e) => ToolOutput::err(format!("read error: {}", e)),
        }
    }

    async fn write_file(&self, args: serde_json::Value) -> ToolOutput {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolOutput::err("missing required argument: content"),
        };
        let path = match self.validate_path(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };
        info!(path = %path.display(), bytes = content.len(), "write_file");
        // Create parent directories
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolOutput::err(format!("create dirs failed: {}", e));
            }
        }
        match tokio::fs::write(&path, content).await {
            Ok(_) => ToolOutput::ok(format!("Written {} bytes to {}", content.len(), path.display())),
            Err(e) => ToolOutput::err(format!("write error: {}", e)),
        }
    }

    async fn list_dir(&self, args: serde_json::Value) -> ToolOutput {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::err("missing required argument: path"),
        };
        let path = match self.validate_path(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };
        info!(path = %path.display(), "list_dir");
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(e) => return ToolOutput::err(format!("read_dir error: {}", e)),
        };
        let mut lines = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let suffix = if entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            lines.push(format!("{}{}", name, suffix));
        }
        lines.sort();
        ToolOutput::ok(lines.join("\n"))
    }

    async fn run_terminal_cmd(&self, args: serde_json::Value) -> ToolOutput {
        let cmd = match args.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return ToolOutput::err("missing required argument: cmd"),
        };
        let cwd = args.get("cwd").and_then(|v| v.as_str()).map(String::from);
        let timeout_secs = args.get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        info!(cmd = %cmd, cwd = ?cwd, timeout_secs, "run_terminal_cmd");

        let mut command = Command::new("sh");
        command.arg("-c").arg(&cmd);
        if let Some(dir) = &cwd {
            command.current_dir(dir);
        }
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("spawn error: {}", e)),
        };

        // Wait with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output(),
        ).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);
                // Stream stderr to developer (eprintln for real-time visibility)
                if !stderr.is_empty() {
                    eprintln!("[run_terminal_cmd stderr]\n{}", stderr);
                }
                let combined = format!(
                    "exit_code: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                    code, stdout, stderr
                );
                if code != 0 {
                    ToolOutput { content: combined, is_error: true }
                } else {
                    ToolOutput::ok(combined)
                }
            }
            Ok(Err(e)) => ToolOutput::err(format!("command error: {}", e)),
            Err(_) => ToolOutput::err(format!("command timed out after {}s", timeout_secs)),
        }
    }

    async fn fetch_url(&self, args: serde_json::Value) -> ToolOutput {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => return ToolOutput::err("missing required argument: url"),
        };
        info!(url = %url, "fetch_url");
        let output = Command::new("curl")
            .args(["-sS", "-L", "--max-time", "30", &url])
            .output()
            .await;
        match output {
            Ok(o) => {
                let body = String::from_utf8_lossy(&o.stdout).to_string();
                if body.len() > MAX_FILE_SIZE {
                    ToolOutput::ok(format!("{}\n[Content Truncated at 32KB]", &body[..MAX_FILE_SIZE]))
                } else {
                    ToolOutput::ok(body)
                }
            }
            Err(e) => ToolOutput::err(format!("curl error: {}", e)),
        }
    }
}
