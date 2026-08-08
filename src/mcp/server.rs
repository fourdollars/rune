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
            Some(serde_json::json!({ "details": details.into() })),
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

/// Helper function to decode Mcp-Method / Mcp-Name header which may be raw string or Base64 encoded RFC 9110 sentinel
pub fn decode_mcp_header(header_val: &str) -> String {
    let trimmed = header_val.trim();
    if trimmed.starts_with("=?base64?") && trimmed.ends_with("?=") {
        let b64 = &trimmed[9..trimmed.len() - 2];
        if let Ok(decoded_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64) {
            if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                return decoded_str;
            }
        }
    }
    // Fallback: try direct base64 decode if valid base64 and not plain ascii text
    if !trimmed.contains('/') && !trimmed.contains(' ') && trimmed.contains('=') {
        if let Ok(decoded_bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, trimmed) {
            if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                if !decoded_str.is_empty() && decoded_str.chars().all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t') {
                    return decoded_str;
                }
            }
        }
    }
    trimmed.to_string()
}

/// Validate HTTP headers (MCP-Protocol-Version, Mcp-Method, Mcp-Name) against JSON-RPC request body according to 2026-07-28 spec
pub fn validate_header_body_consistency(
    header_protocol_version: Option<&str>,
    header_method: Option<&str>,
    header_name: Option<&str>,
    req: &McpJsonRpcRequest,
) -> Result<(), String> {
    // 1. MCP-Protocol-Version validation
    let proto_ver = match header_protocol_version {
        Some(v) => v.trim(),
        None => {
            return Err("Required MCP-Protocol-Version header is missing".to_string());
        }
    };

    if let Some(params) = &req.params {
        if let Some(meta) = params.get("_meta") {
            if let Some(body_proto) = meta.get("io.modelcontextprotocol/protocolVersion").and_then(|v| v.as_str()) {
                if proto_ver != body_proto {
                    return Err(format!(
                        "MCP-Protocol-Version header '{}' does not match body _meta protocolVersion '{}'",
                        proto_ver, body_proto
                    ));
                }
            }
        }
    }

    // 2. Mcp-Method validation
    let h_method = match header_method {
        Some(m) => m,
        None => {
            return Err("Required Mcp-Method header is missing".to_string());
        }
    };

    let decoded_method = decode_mcp_header(h_method);
    if decoded_method != req.method {
        return Err(format!(
            "Mcp-Method header '{}' (decoded: '{}') does not match body method '{}'",
            h_method, decoded_method, req.method
        ));
    }

    // 3. Mcp-Name validation
    let is_name_required = matches!(req.method.as_str(), "tools/call" | "resources/read" | "prompts/get");

    let body_name_or_uri = req.params.as_ref().and_then(|p| {
        p.get("name").or_else(|| p.get("uri")).and_then(|v| v.as_str())
    });

    match (header_name, body_name_or_uri) {
        (Some(h_name), Some(b_val)) => {
            let decoded_name = decode_mcp_header(h_name);
            if decoded_name != b_val {
                return Err(format!(
                    "Mcp-Name header '{}' (decoded: '{}') does not match body name/uri '{}'",
                    h_name, decoded_name, b_val
                ));
            }
        }
        (None, Some(_)) if is_name_required => {
            return Err(format!(
                "Required Mcp-Name header is missing for method '{}'",
                req.method
            ));
        }
        (Some(h_name), None) => {
            return Err(format!(
                "Mcp-Name header provided ('{}'), but body name/uri parameter is missing",
                h_name
            ));
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_decoding() {
        assert_eq!(decode_mcp_header("tools/call"), "tools/call");
        assert_eq!(decode_mcp_header("=?base64?dG9vbHMvY2FsbA==?="), "tools/call");
    }

    #[test]
    fn test_header_body_validation_success() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "read_note_file",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28"
                }
            })),
        };

        assert!(validate_header_body_consistency(
            Some("2026-07-28"),
            Some("tools/call"),
            Some("read_note_file"),
            &req
        ).is_ok());
    }

    #[test]
    fn test_header_body_validation_mismatch() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file"})),
        };

        // Missing protocol version header
        assert!(validate_header_body_consistency(None, Some("tools/call"), Some("read_note_file"), &req).is_err());

        // Mismatched method
        let err = validate_header_body_consistency(Some("2026-07-28"), Some("tools/list"), Some("read_note_file"), &req).unwrap_err();
        assert!(err.contains("does not match body method"));

        // Mismatched name
        let err2 = validate_header_body_consistency(Some("2026-07-28"), Some("tools/call"), Some("write_note_file"), &req).unwrap_err();
        assert!(err2.contains("does not match body"));
    }
}
