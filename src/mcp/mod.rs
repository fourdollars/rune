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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_manager_new_empty() {
        let mgr = McpManager::new();
        assert_eq!(mgr.clients_count(), 0);
        assert!(mgr.all_tools().is_empty());
    }

    #[test]
    fn test_mcp_server_config_defaults() {
        let json = r#"{"name": "test", "command": "/bin/test"}"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.name, "test");
        assert_eq!(cfg.command, "/bin/test");
        assert!(cfg.args.is_empty());
        assert_eq!(cfg.timeout_secs, Some(30));
        assert!(!cfg.required);
        assert!(cfg.env.is_none());
    }

    #[test]
    fn test_mcp_server_config_full() {
        let json = r#"{
            "name": "zhtw-mcp",
            "command": "/usr/local/bin/zhtw-mcp",
            "args": ["--verbose"],
            "timeout_secs": 60,
            "required": true,
            "env": {"RUST_LOG": "info"}
        }"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.name, "zhtw-mcp");
        assert_eq!(cfg.command, "/usr/local/bin/zhtw-mcp");
        assert_eq!(cfg.args, vec!["--verbose"]);
        assert_eq!(cfg.timeout_secs, Some(60));
        assert!(cfg.required);
        assert_eq!(cfg.env.unwrap().get("RUST_LOG").unwrap(), "info");
    }

    #[test]
    fn test_mcp_tool_deserialize() {
        let json = r#"{
            "name": "zhtw",
            "description": "Lint zh-TW text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "profile": {"type": "string"}
                },
                "required": ["text"]
            }
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "zhtw");
        assert_eq!(tool.description.as_deref(), Some("Lint zh-TW text"));
        let schema = tool.input_schema.unwrap();
        let props = schema.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("text"));
        assert!(props.contains_key("profile"));
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].as_str().unwrap(), "text");
    }

    #[test]
    fn test_mcp_tool_deserialize_minimal() {
        let json = r#"{"name": "simple"}"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "simple");
        assert!(tool.description.is_none());
        assert!(tool.input_schema.is_none());
    }

    #[test]
    fn test_mcp_server_config_in_toml() {
        let toml_str = r#"
[[mcp_servers]]
name = "zhtw-mcp"
command = "/home/u/zhtw-mcp/target/release/zhtw-mcp"
args = []
timeout_secs = 30
required = false
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            mcp_servers: Vec<McpServerConfig>,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.mcp_servers.len(), 1);
        assert_eq!(w.mcp_servers[0].name, "zhtw-mcp");
        assert!(!w.mcp_servers[0].required);
    }

    #[test]
    fn test_mcp_multiple_servers_in_toml() {
        let toml_str = r#"
[[mcp_servers]]
name = "server-a"
command = "/bin/a"
required = true

[[mcp_servers]]
name = "server-b"
command = "/bin/b"
args = ["--flag"]
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            mcp_servers: Vec<McpServerConfig>,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.mcp_servers.len(), 2);
        assert_eq!(w.mcp_servers[0].name, "server-a");
        assert!(w.mcp_servers[0].required);
        assert_eq!(w.mcp_servers[1].name, "server-b");
        assert_eq!(w.mcp_servers[1].args, vec!["--flag"]);
    }
}

// ── Additional tests to boost coverage ──────────────────────────────────────
#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization_no_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["jsonrpc"].as_str().unwrap(), "2.0");
        assert_eq!(v["id"].as_u64().unwrap(), 1);
        assert_eq!(v["method"].as_str().unwrap(), "tools/list");
        // params should be omitted when None
        assert!(v.get("params").is_none() || v["params"].is_null());
    }

    #[test]
    fn test_json_rpc_request_serialization_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 42,
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "lint", "arguments": {}})),
        };
        let s = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"].as_u64().unwrap(), 42);
        assert_eq!(v["params"]["name"].as_str().unwrap(), "lint");
    }

    #[test]
    fn test_json_rpc_request_params_field_absent_when_none() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "ping".to_string(),
            params: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        // skip_serializing_if = "Option::is_none" — key must be absent
        assert!(!s.contains("\"params\""));
    }

    #[test]
    fn test_json_rpc_response_success() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]},"error":null}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let json = r#"{"jsonrpc":"2.0","id":2,"result":null,"error":{"code":-32601,"message":"Method not found","data":null}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(2));
        assert!(resp.result.is_none() || resp.result == Some(serde_json::Value::Null));
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_json_rpc_response_notification_no_id() {
        let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        // Notifications won't parse as JsonRpcResponse correctly — testing tolerance
        let result = serde_json::from_str::<JsonRpcResponse>(json);
        // May succeed or fail; if it succeeds, id should be None
        if let Ok(resp) = result {
            assert!(resp.id.is_none());
        }
    }

    #[test]
    fn test_mcp_client_new_and_server_name() {
        let cfg = McpServerConfig {
            name: "my-server".to_string(),
            command: "/bin/test".to_string(),
            args: vec![],
            env: None,
            timeout_secs: Some(30),
            required: false,
        };
        let client = McpClient::new(cfg);
        assert_eq!(client.server_name(), "my-server");
    }

    #[test]
    fn test_mcp_client_not_running_before_start() {
        let cfg = McpServerConfig {
            name: "test".to_string(),
            command: "/bin/test".to_string(),
            args: vec![],
            env: None,
            timeout_secs: Some(30),
            required: false,
        };
        let mut client = McpClient::new(cfg);
        assert!(!client.is_running());
    }

    #[test]
    fn test_mcp_manager_clients_count_zero() {
        let mgr = McpManager::new();
        assert_eq!(mgr.clients_count(), 0);
    }

    #[test]
    fn test_mcp_manager_all_tools_empty() {
        let mgr = McpManager::new();
        let tools = mgr.all_tools();
        assert!(tools.is_empty());
    }

    #[test]
    fn test_mcp_tool_with_all_fields() {
        let json = r#"{
            "name": "translate",
            "description": "Translate text to zh-TW",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "source": {"type": "string"}
                }
            }
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "translate");
        assert_eq!(tool.description.as_deref(), Some("Translate text to zh-TW"));
        let schema = tool.input_schema.unwrap();
        assert_eq!(schema["type"].as_str().unwrap(), "object");
    }

    #[test]
    fn test_json_rpc_request_id_increment_logic() {
        // Verify that sequential IDs work (simulating next_id logic)
        let ids: Vec<u64> = (1..=5).collect();
        for (i, id) in ids.iter().enumerate() {
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: *id,
                method: "test".to_string(),
                params: None,
            };
            assert_eq!(req.id, (i + 1) as u64);
        }
    }

    #[test]
    fn test_mcp_server_config_with_env() {
        let json = r#"{
            "name": "env-server",
            "command": "/usr/bin/server",
            "env": {
                "API_KEY": "secret",
                "LOG_LEVEL": "debug"
            }
        }"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        let env = cfg.env.unwrap();
        assert_eq!(env.get("API_KEY").unwrap(), "secret");
        assert_eq!(env.get("LOG_LEVEL").unwrap(), "debug");
    }

    #[test]
    fn test_default_timeout_value() {
        // Verify default timeout is 30 seconds
        let json = r#"{"name": "t", "command": "/bin/t"}"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.timeout_secs, Some(30));
    }

    #[test]
    fn test_mcp_server_config_zero_timeout() {
        let json = r#"{"name": "t", "command": "/bin/t", "timeout_secs": 0}"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.timeout_secs, Some(0));
    }

    #[test]
    fn test_mcp_tool_clone() {
        let tool = McpTool {
            name: "my-tool".to_string(),
            description: Some("desc".to_string()),
            input_schema: Some(serde_json::json!({"type": "object"})),
        };
        let cloned = tool.clone();
        assert_eq!(cloned.name, tool.name);
        assert_eq!(cloned.description, tool.description);
    }

    #[test]
    fn test_json_rpc_error_deserialization() {
        let json = r#"{"code": -32700, "message": "Parse error"}"#;
        let err: JsonRpcError = serde_json::from_str(json).unwrap();
        assert_eq!(err.code, -32700);
        assert_eq!(err.message, "Parse error");
        assert!(err.data.is_none());
    }

    #[test]
    fn test_json_rpc_error_with_data() {
        let json = r#"{"code": -32000, "message": "Server error", "data": {"detail": "crash"}}"#;
        let err: JsonRpcError = serde_json::from_str(json).unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.data.is_some());
    }

    #[test]
    fn test_mcp_server_config_null_timeout() {
        let json = r#"{"name": "t", "command": "/bin/t", "timeout_secs": null}"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.timeout_secs.is_none());
    }
}
