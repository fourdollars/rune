use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28", "2024-11-05"];

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

    pub fn unsupported_protocol_version(id: Option<Value>, details: impl Into<String>) -> Self {
        Self::error(
            id,
            -32021,
            "UnsupportedProtocolVersionError: Unsupported MCP-Protocol-Version",
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
) -> Result<(), (i32, String)> {
    // 1. MCP-Protocol-Version validation
    let proto_ver = match header_protocol_version {
        Some(v) => v.trim(),
        None => {
            return Err((-32020, "Required MCP-Protocol-Version header is missing".to_string()));
        }
    };

    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&proto_ver) {
        return Err((
            -32021,
            format!(
                "MCP-Protocol-Version '{}' is not supported. Supported versions: {:?}",
                proto_ver, SUPPORTED_PROTOCOL_VERSIONS
            ),
        ));
    }

    if let Some(params) = &req.params {
        if let Some(meta) = params.get("_meta") {
            if let Some(body_proto) = meta.get("io.modelcontextprotocol/protocolVersion").and_then(|v| v.as_str()) {
                if proto_ver != body_proto {
                    return Err((
                        -32020,
                        format!(
                            "MCP-Protocol-Version header '{}' does not match body _meta protocolVersion '{}'",
                            proto_ver, body_proto
                        ),
                    ));
                }
            }
        }
    }

    // 2. Mcp-Method validation
    let h_method = match header_method {
        Some(m) => m,
        None => {
            return Err((-32020, "Required Mcp-Method header is missing".to_string()));
        }
    };

    let decoded_method = decode_mcp_header(h_method);
    if decoded_method != req.method {
        return Err((
            -32020,
            format!(
                "Mcp-Method header '{}' (decoded: '{}') does not match body method '{}'",
                h_method, decoded_method, req.method
            ),
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
                return Err((
                    -32020,
                    format!(
                        "Mcp-Name header '{}' (decoded: '{}') does not match body name/uri '{}'",
                        h_name, decoded_name, b_val
                    ),
                ));
            }
        }
        (None, Some(_)) if is_name_required => {
            return Err((
                -32020,
                format!(
                    "Required Mcp-Name header is missing for method '{}'",
                    req.method
                ),
            ));
        }
        (Some(h_name), None) => {
            return Err((
                -32020,
                format!(
                    "Mcp-Name header provided ('{}'), but body name/uri parameter is missing",
                    h_name
                ),
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
        let (code, msg) = validate_header_body_consistency(None, Some("tools/call"), Some("read_note_file"), &req).unwrap_err();
        assert_eq!(code, -32020);
        assert!(msg.contains("Required MCP-Protocol-Version header is missing"));

        // Unsupported protocol version
        let (code_unsupported, msg_unsupported) = validate_header_body_consistency(Some("1999-01-01"), Some("tools/call"), Some("read_note_file"), &req).unwrap_err();
        assert_eq!(code_unsupported, -32021);
        assert!(msg_unsupported.contains("is not supported"));

        // Mismatched method
        let (code_m, msg_m) = validate_header_body_consistency(Some("2026-07-28"), Some("tools/list"), Some("read_note_file"), &req).unwrap_err();
        assert_eq!(code_m, -32020);
        assert!(msg_m.contains("does not match body method"));

        // Mismatched name
        let (code_n, msg_n) = validate_header_body_consistency(Some("2026-07-28"), Some("tools/call"), Some("write_note_file"), &req).unwrap_err();
        assert_eq!(code_n, -32020);
        assert!(msg_n.contains("does not match body"));
    }
}
