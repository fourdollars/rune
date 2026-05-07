use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, error, info, warn};

// ── Config ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub required: bool,
}

fn default_timeout() -> Option<u64> {
    Some(30)
}

// ── JSON-RPC ─────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<serde_json::Value>,
}

// ── MCP Tool ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<serde_json::Value>,
}

// ── McpClient ────────────────────────────────────────────────────

pub struct McpClient {
    config: McpServerConfig,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout_reader: Option<BufReader<ChildStdout>>,
    next_id: u64,
    tools: Vec<McpTool>,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            child: None,
            stdin: None,
            stdout_reader: None,
            next_id: 1,
            tools: Vec::new(),
        }
    }

    /// Start the MCP server subprocess (stdio transport).
    pub async fn start(&mut self) -> Result<()> {
        info!(server = %self.config.name, cmd = %self.config.command, "starting MCP server");

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        if let Some(env) = &self.config.env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to start MCP server '{}'", self.config.name))?;

        self.stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("no stdout for MCP server '{}'", self.config.name))?;
        self.stdout_reader = Some(BufReader::new(stdout));
        self.child = Some(child);

        info!(server = %self.config.name, "MCP server process started");
        Ok(())
    }

    /// Send a JSON-RPC request and wait for response.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        let mut payload =
            serde_json::to_string(&request).context("failed to serialize JSON-RPC request")?;
        payload.push('\n');

        debug!(server = %self.config.name, method, id, "sending JSON-RPC request");

        let stdin = self.stdin.as_mut().ok_or_else(|| {
            anyhow::anyhow!("MCP server '{}' stdin not available", self.config.name)
        })?;
        stdin
            .write_all(payload.as_bytes())
            .await
            .context("failed to write to MCP server stdin")?;
        stdin.flush().await?;

        // Read response line
        let reader = self.stdout_reader.as_mut().ok_or_else(|| {
            anyhow::anyhow!("MCP server '{}' stdout not available", self.config.name)
        })?;

        let timeout = std::time::Duration::from_secs(self.config.timeout_secs.unwrap_or(30));

        let line = tokio::time::timeout(timeout, async {
            let mut buf = String::new();
            loop {
                buf.clear();
                let n = reader.read_line(&mut buf).await?;
                if n == 0 {
                    bail!("MCP server '{}' stdout closed", self.config.name);
                }
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue; // skip empty lines / notifications
                }
                // Try to parse as response
                if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    return Ok(resp);
                }
                // Skip non-response lines (notifications, etc.)
                debug!(server = %self.config.name, line = trimmed, "skipping non-response line");
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("MCP server '{}' request timed out", self.config.name))??;

        if let Some(ref err) = line.error {
            warn!(server = %self.config.name, code = err.code, msg = %err.message, "JSON-RPC error");
        }

        Ok(line)
    }

    /// Send initialize request.
    pub async fn initialize(&mut self) -> Result<serde_json::Value> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "rune",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let resp = self.send_request("initialize", Some(params)).await?;

        if let Some(err) = resp.error {
            bail!("MCP initialize error: {} (code {})", err.message, err.code);
        }

        // Send initialized notification (no response expected, but write it)
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let mut payload = serde_json::to_string(&notif)?;
        payload.push('\n');
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.flush().await;
        }

        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// List available tools via tools/list.
    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let resp = self.send_request("tools/list", None).await?;

        if let Some(err) = resp.error {
            bail!("tools/list error: {} (code {})", err.message, err.code);
        }

        let result = resp.result.unwrap_or(serde_json::Value::Null);
        let tools: Vec<McpTool> = result
            .get("tools")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        self.tools = tools.clone();
        info!(server = %self.config.name, count = tools.len(), "listed MCP tools");
        Ok(tools)
    }

    /// Call a tool via tools/call.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", Some(params)).await?;

        if let Some(err) = resp.error {
            bail!(
                "tools/call '{}' error: {} (code {})",
                name,
                err.message,
                err.code
            );
        }

        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// Shutdown the server gracefully.
    pub async fn shutdown(&mut self) -> Result<()> {
        info!(server = %self.config.name, "shutting down MCP server");

        // Drop stdin to signal EOF
        self.stdin.take();

        if let Some(mut child) = self.child.take() {
            let timeout = std::time::Duration::from_secs(5);
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(status)) => {
                    info!(server = %self.config.name, code = ?status.code(), "MCP server exited");
                }
                Ok(Err(e)) => {
                    warn!(server = %self.config.name, error = %e, "error waiting for MCP server");
                }
                Err(_) => {
                    warn!(server = %self.config.name, "MCP server didn't exit in time, killing");
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }

    /// Check if the server process is still running.
    pub fn is_running(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => true, // still running
                Ok(Some(_)) => false,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub fn server_name(&self) -> &str {
        &self.config.name
    }
}

// ── McpManager ───────────────────────────────────────────────────

pub struct McpManager {
    clients: Vec<McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Vec::new(),
        }
    }

    /// Start all configured MCP servers.
    pub async fn start_all(&mut self, configs: Vec<McpServerConfig>) -> Result<()> {
        for cfg in configs {
            let required = cfg.required;
            let name = cfg.name.clone();
            let mut client = McpClient::new(cfg);

            match client.start().await {
                Ok(()) => {
                    // Try to initialize
                    match client.initialize().await {
                        Ok(_) => {
                            // List tools
                            match client.list_tools().await {
                                Ok(tools) => {
                                    info!(server = %name, tools = tools.len(), "MCP server ready");
                                }
                                Err(e) => {
                                    warn!(server = %name, error = %e, "failed to list tools");
                                }
                            }
                            self.clients.push(client);
                        }
                        Err(e) => {
                            if required {
                                error!(server = %name, error = %e, "required MCP server failed to initialize");
                                bail!("required MCP server '{}' failed: {}", name, e);
                            }
                            warn!(server = %name, error = %e, "optional MCP server failed to initialize, skipping");
                        }
                    }
                }
                Err(e) => {
                    if required {
                        error!(server = %name, error = %e, "required MCP server failed to start");
                        bail!("required MCP server '{}' failed to start: {}", name, e);
                    }
                    warn!(server = %name, error = %e, "optional MCP server failed to start, skipping");
                }
            }
        }
        Ok(())
    }

    /// Return the number of connected MCP clients.
    pub fn clients_count(&self) -> usize {
        self.clients.len()
    }

    /// List all tools from all connected servers.
    pub fn all_tools(&self) -> Vec<(String, McpTool)> {
        let mut result = Vec::new();
        for client in &self.clients {
            for tool in &client.tools {
                result.push((client.config.name.clone(), tool.clone()));
            }
        }
        result
    }

    /// Call a tool, routing to the server that owns it.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Find which client owns this tool
        let idx = self
            .clients
            .iter()
            .position(|c| c.tools.iter().any(|t| t.name == name));

        match idx {
            Some(i) => self.clients[i].call_tool(name, arguments).await,
            None => bail!("no MCP server provides tool '{}'", name),
        }
    }

    /// Shutdown all servers.
    pub async fn shutdown_all(&mut self) {
        for client in &mut self.clients {
            if let Err(e) = client.shutdown().await {
                warn!(server = %client.server_name(), error = %e, "error shutting down MCP server");
            }
        }
        self.clients.clear();
    }

    /// Health summary: (server_name, is_running).
    pub fn health_summary(&mut self) -> Vec<(String, bool)> {
        self.clients
            .iter_mut()
            .map(|c| (c.config.name.clone(), c.is_running()))
            .collect()
    }
}
