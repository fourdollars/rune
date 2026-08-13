//! Native JSON-RPC Protocol types for Rune Vim integration.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

/// Standard JSON-RPC Request structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Standard JSON-RPC Response structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// Standard JSON-RPC Notification structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: Option<Value>,
        code: i32,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }
}

// Custom Error Codes per §5.7
pub const ERR_PROTOCOL_VERSION_UNSUPPORTED: i32 = -32001;
pub const ERR_PROVIDER_TIMEOUT: i32 = -32002;
pub const ERR_RATE_LIMITED: i32 = -32003;
pub const ERR_PROVIDER_AUTH_FAILURE: i32 = -32004;
pub const ERR_REQUEST_CANCELLED: i32 = -32005;
pub const ERR_POLICY_EXCLUDED: i32 = -32006;

// Params DTOs

#[derive(Debug, Clone, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: u32,
    #[serde(default)]
    pub client: Option<ClientInfo>,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub capabilities: Option<ClientCapabilities>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub editor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub multiline_ghost_text: bool,
    #[serde(default)]
    pub cycling: bool,
    #[serde(default)]
    pub render_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_version: String,
    pub provider: String,
    pub model: String,
    pub available_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub available_levels: Vec<String>,
    pub fim: String,
    pub max_candidates: usize,
    pub rate_limit: RateLimitInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub capacity: u32,
    pub refill_per_min: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TruncatedInfo {
    #[serde(default)]
    pub prefix: bool,
    #[serde(default)]
    pub suffix: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompletionParams {
    pub buffer_id: u64,
    pub version: u64,
    pub filepath: String,
    pub language: String,
    pub prefix: String,
    pub suffix: String,
    #[serde(default)]
    pub truncated: Option<TruncatedInfo>,
    pub line: u32,
    pub character: u32,
    #[serde(default = "default_max_candidates")]
    pub max_candidates: usize,
}

fn default_max_candidates() -> usize {
    1
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub text: String,
    pub display_lines: usize,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionResult {
    pub buffer_id: u64,
    pub version: u64,
    pub cached: bool,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CancelParams {
    pub id: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatParams {
    pub buffer_id: u64,
    pub prompt: String,
    #[serde(default)]
    pub selection: Option<SelectionRange>,
    pub filepath: String,
    pub language: String,
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditParams {
    pub buffer_id: u64,
    pub prompt: String,
    #[serde(default)]
    pub selection: Option<SelectionRange>,
    pub filepath: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditItem {
    pub start_line: usize,
    pub end_line: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditResult {
    pub edits: Vec<EditItem>,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelParams {
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResult {
    pub active_model: String,
    pub available_models: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThinkingParams {
    #[serde(default)]
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingResult {
    pub active_thinking: String,
    pub available_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub state: String,
    pub provider: String,
    pub model: String,
    pub available_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub available_levels: Vec<String>,
    pub enabled: bool,
    pub in_flight: u32,
    pub rate_budget: RateBudgetInfo,
    pub last_error: Option<String>,
    pub last_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateBudgetInfo {
    pub remaining: u32,
    pub capacity: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_initialize_params_deserialization() {
        let raw = json!({
            "protocol_version": 1,
            "client": { "name": "rune.vim", "version": "0.1.0" },
            "workspace_root": "/home/user/project"
        });
        let params: InitializeParams = serde_json::from_value(raw).unwrap();
        assert_eq!(params.protocol_version, 1);
        assert_eq!(params.client.unwrap().name, "rune.vim");
    }

    #[test]
    fn test_completion_params_deserialization() {
        let raw = json!({
            "buffer_id": 4,
            "version": 187,
            "filepath": "src/main.rs",
            "language": "rust",
            "prefix": "fn main() {",
            "suffix": "}",
            "line": 1,
            "character": 12
        });
        let params: CompletionParams = serde_json::from_value(raw).unwrap();
        assert_eq!(params.buffer_id, 4);
        assert_eq!(params.version, 187);
    }
}
