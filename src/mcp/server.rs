use serde::{Deserialize, Serialize};
use serde_json::Value;

// JSON-RPC 2.0 Request according to MCP spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpJsonRpcError>,
}

impl McpJsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(McpJsonRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }

    pub fn header_mismatch(id: Option<Value>, details: impl Into<String>) -> Self {
        Self::error(
            id,
            -32020,
            "HeaderMismatch: HTTP headers do not match JSON-RPC body parameters",
            Some(serde_json::json!({"details": details.into()})),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Helper function to decode Mcp-Method / Mcp-Name header which may be raw string or Base64 encoded
pub fn decode_mcp_header(header_val: &str) -> String {
    let trimmed = header_val.trim();
    if let Ok(decoded_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, trimmed) {
        if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
            // Only accept if it looks valid UTF-8 string
            if !decoded_str.is_empty() && decoded_str.chars().all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t') {
                return decoded_str;
            }
        }
    }
    trimmed.to_string()
}

/// Validate HTTP headers (Mcp-Method, Mcp-Name) against JSON-RPC request body according to 2026-07-28 spec
pub fn validate_header_body_consistency(
    header_method: Option<&str>,
    header_name: Option<&str>,
    req: &McpJsonRpcRequest,
) -> Result<(), String> {
    if let Some(h_method) = header_method {
        let decoded_method = decode_mcp_header(h_method);
        if decoded_method != req.method {
            return Err(format!(
                "Mcp-Method header '{}' (decoded: '{}') does not match body method '{}'",
                h_method, decoded_method, req.method
            ));
        }
    }

    if let Some(h_name) = header_name {
        let decoded_name = decode_mcp_header(h_name);
        let body_name = req
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str());

        match body_name {
            Some(b_name) => {
                if decoded_name != b_name {
                    return Err(format!(
                        "Mcp-Name header '{}' (decoded: '{}') does not match body params.name '{}'",
                        h_name, decoded_name, b_name
                    ));
                }
            }
            None => {
                return Err(format!(
                    "Mcp-Name header provided ('{}'), but body params.name is missing",
                    h_name
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_decoding() {
        assert_eq!(decode_mcp_header("tools/call"), "tools/call");
        // base64 of "tools/call" is "dG9vbHMvY2FsbA=="
        assert_eq!(decode_mcp_header("dG9vbHMvY2FsbA=="), "tools/call");
    }

    #[test]
    fn test_header_body_validation_success() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file", "arguments": {}})),
        };

        assert!(validate_header_body_consistency(Some("tools/call"), Some("read_note_file"), &req).is_ok());
        assert!(validate_header_body_consistency(Some("dG9vbHMvY2FsbA=="), None, &req).is_ok());
    }

    #[test]
    fn test_header_body_validation_mismatch() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file"})),
        };

        let err = validate_header_body_consistency(Some("tools/list"), None, &req).unwrap_err();
        assert!(err.contains("does not match body method"));

        let err2 = validate_header_body_consistency(Some("tools/call"), Some("write_note_file"), &req).unwrap_err();
        assert!(err2.contains("does not match body params.name"));
    }
}
