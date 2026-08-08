use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28", "2024-11-05"];

/// Legacy protocol versions that get a `Mcp-Session-Id` + GET stream compatibility path
/// (see MCP_Backward_Compat_GET_Stream_Spec.md, Appendix 1). These are accepted by
/// `initialize` in addition to `SUPPORTED_PROTOCOL_VERSIONS`. Per the MCP spec, `initialize`
/// must echo back the negotiated `protocolVersion` in its response (not the server's own
/// newest version) — MCP SDK clients (e.g. the official TypeScript SDK) reject the
/// `initialize` result outright if `protocolVersion` is a value they don't recognize.
pub const LEGACY_SESSION_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];

/// True if `version` is accepted at all by `initialize` (current + legacy session versions).
pub fn is_initialize_protocol_version_acceptable(version: &str) -> bool {
    SUPPORTED_PROTOCOL_VERSIONS.contains(&version) || LEGACY_SESSION_PROTOCOL_VERSIONS.contains(&version)
}

/// Compute the `protocolVersion` the server should echo back in an `initialize` response,
/// given the client's requested version. If the requested version is one we accept
/// (current or legacy), echo it back unchanged (per MCP spec: this is what the client
/// negotiated and understands). Otherwise (unrecognized/missing), fall back to our
/// latest supported version.
pub fn negotiate_initialize_protocol_version(requested_version: &str) -> &'static str {
    if let Some(v) = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .chain(LEGACY_SESSION_PROTOCOL_VERSIONS.iter())
        .find(|&&v| v == requested_version)
    {
        v
    } else {
        SUPPORTED_PROTOCOL_VERSIONS[0]
    }
}

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

/// Header-metadata presence classification used by the Lenient Legacy Client Mode
/// (see MCP_Backward_Compat_GET_Stream_Spec.md, Appendix 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderMetadataPresence {
    /// None of MCP-Protocol-Version / Mcp-Method / Mcp-Name are present.
    None,
    /// At least one but not all three are present.
    Partial,
    /// All three headers are present (full 2026-07-28 client behavior).
    Full,
}

/// Classify how much MCP Request Metadata header machinery a request is using.
pub fn classify_header_metadata_presence(
    header_protocol_version: Option<&str>,
    header_method: Option<&str>,
    header_name: Option<&str>,
) -> HeaderMetadataPresence {
    let present = [
        header_protocol_version.is_some(),
        header_method.is_some(),
        header_name.is_some(),
    ];
    let count = present.iter().filter(|p| **p).count();
    match count {
        0 => HeaderMetadataPresence::None,
        3 => HeaderMetadataPresence::Full,
        _ => HeaderMetadataPresence::Partial,
    }
}

/// Validate a request against the Lenient Legacy Client Mode rules:
///
/// - No header metadata at all → skip header-body consistency validation entirely
///   (body-only dispatch; legacy/unaware client).
/// - Some but not all header metadata → still enforce strict validation (client
///   attempted partial support; do not mask incomplete implementations).
/// - All header metadata present → strict validation (current 2026-07-28 behavior).
///
/// This must only be called when Lenient Legacy Client Mode is enabled by config.
pub fn validate_header_body_consistency_lenient(
    header_protocol_version: Option<&str>,
    header_method: Option<&str>,
    header_name: Option<&str>,
    req: &McpJsonRpcRequest,
) -> Result<(), (i32, String)> {
    match classify_header_metadata_presence(header_protocol_version, header_method, header_name) {
        HeaderMetadataPresence::None => Ok(()),
        HeaderMetadataPresence::Partial | HeaderMetadataPresence::Full => {
            validate_header_body_consistency(header_protocol_version, header_method, header_name, req)
        }
    }
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

    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&proto_ver) && !LEGACY_SESSION_PROTOCOL_VERSIONS.contains(&proto_ver) {
        return Err((
            -32021,
            format!(
                "MCP-Protocol-Version '{}' is not supported. Supported versions: {:?} (plus legacy session versions {:?})",
                proto_ver, SUPPORTED_PROTOCOL_VERSIONS, LEGACY_SESSION_PROTOCOL_VERSIONS
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
    fn test_is_initialize_protocol_version_acceptable() {
        assert!(is_initialize_protocol_version_acceptable("2026-07-28"));
        assert!(is_initialize_protocol_version_acceptable("2024-11-05"));
        assert!(is_initialize_protocol_version_acceptable("2025-03-26"));
        assert!(is_initialize_protocol_version_acceptable("2025-06-18"));
        assert!(is_initialize_protocol_version_acceptable("2025-11-25"));
        assert!(!is_initialize_protocol_version_acceptable("1999-01-01"));
        assert!(!is_initialize_protocol_version_acceptable(""));
    }

    #[test]
    fn test_negotiate_initialize_protocol_version_echoes_current() {
        assert_eq!(negotiate_initialize_protocol_version("2026-07-28"), "2026-07-28");
        assert_eq!(negotiate_initialize_protocol_version("2024-11-05"), "2024-11-05");
    }

    #[test]
    fn test_negotiate_initialize_protocol_version_echoes_legacy() {
        assert_eq!(negotiate_initialize_protocol_version("2025-03-26"), "2025-03-26");
        assert_eq!(negotiate_initialize_protocol_version("2025-06-18"), "2025-06-18");
        assert_eq!(negotiate_initialize_protocol_version("2025-11-25"), "2025-11-25");
    }

    #[test]
    fn test_negotiate_initialize_protocol_version_falls_back_for_unknown() {
        assert_eq!(negotiate_initialize_protocol_version("1999-01-01"), "2026-07-28");
        assert_eq!(negotiate_initialize_protocol_version(""), "2026-07-28");
    }

    #[test]
    fn test_classify_header_metadata_presence_none() {
        assert_eq!(
            classify_header_metadata_presence(None, None, None),
            HeaderMetadataPresence::None
        );
    }

    #[test]
    fn test_classify_header_metadata_presence_full() {
        assert_eq!(
            classify_header_metadata_presence(Some("2026-07-28"), Some("tools/call"), Some("read_note_file")),
            HeaderMetadataPresence::Full
        );
    }

    #[test]
    fn test_classify_header_metadata_presence_partial() {
        assert_eq!(
            classify_header_metadata_presence(Some("2026-07-28"), None, None),
            HeaderMetadataPresence::Partial
        );
        assert_eq!(
            classify_header_metadata_presence(None, Some("tools/call"), None),
            HeaderMetadataPresence::Partial
        );
        assert_eq!(
            classify_header_metadata_presence(Some("2026-07-28"), Some("tools/call"), None),
            HeaderMetadataPresence::Partial
        );
    }

    #[test]
    fn test_lenient_validation_no_headers_bypasses_check() {
        // Fully legacy/unaware client: no header metadata at all, body-only dispatch.
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file", "arguments": {}})),
        };
        assert!(validate_header_body_consistency_lenient(None, None, None, &req).is_ok());
    }

    #[test]
    fn test_lenient_validation_partial_headers_still_strict() {
        // Only MCP-Protocol-Version present (e.g. user manually added header, SDK didn't
        // add Mcp-Method) — must still fail with HeaderMismatch, not be silently accepted.
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file", "arguments": {}})),
        };
        let (code, msg) = validate_header_body_consistency_lenient(Some("2026-07-28"), None, None, &req).unwrap_err();
        assert_eq!(code, -32020);
        assert!(msg.contains("Required Mcp-Method header is missing"));
    }

    #[test]
    fn test_lenient_validation_full_headers_matches_strict() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file", "arguments": {}})),
        };
        assert!(validate_header_body_consistency_lenient(
            Some("2026-07-28"),
            Some("tools/call"),
            Some("read_note_file"),
            &req
        ).is_ok());

        // Mismatched method still rejected even with full headers.
        let (code, msg) = validate_header_body_consistency_lenient(
            Some("2026-07-28"),
            Some("tools/list"),
            Some("read_note_file"),
            &req,
        ).unwrap_err();
        assert_eq!(code, -32020);
        assert!(msg.contains("does not match body method"));
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

    #[test]
    fn test_header_body_validation_accepts_legacy_session_protocol_version() {
        let req = McpJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read_note_file"})),
        };

        // A client that sends full headers but negotiated a legacy session protocol
        // version should NOT be rejected as "unsupported protocol version".
        for v in LEGACY_SESSION_PROTOCOL_VERSIONS {
            assert!(validate_header_body_consistency(Some(v), Some("tools/call"), Some("read_note_file"), &req).is_ok());
        }
    }
}
