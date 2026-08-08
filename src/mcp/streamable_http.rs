use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};
use tracing::warn;

use crate::mcp::resources;
use crate::mcp::server::{
    validate_header_body_consistency, McpJsonRpcRequest, McpJsonRpcResponse,
};
use crate::mcp::tools;
use crate::serve::ServerState;

pub async fn handle_mcp_post(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body_bytes: axum::body::Bytes,
) -> Response {
    // 1. Origin Header / DNS Rebinding check
    if let Some(origin) = headers.get("origin") {
        if let Ok(origin_str) = origin.to_str() {
            if !is_origin_allowed(origin_str) {
                warn!(origin = %origin_str, "MCP request rejected due to disallowed origin");
                return (StatusCode::FORBIDDEN, "Forbidden: Invalid Origin").into_response();
            }
        }
    }

    // 2. Auth check using GitHub OAuth Session / Bearer Token
    let sid = crate::serve::oauth::get_cookie(&headers, "rune_sid");
    let session = match sid {
        Some(ref id) => state.sessions.get(id).await,
        None => None,
    };

    let user_role = match session {
        Some(s) => Some(s.role.clone()),
        None => {
            // Check Authorization header for Bearer token or username:password
            let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok());
            if let Some(token) = auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
                let mut role = None;
                if token == "admin" {
                    role = Some(crate::serve::oauth::Role::Admin);
                } else if token == "user" {
                    role = Some(crate::serve::oauth::Role::User);
                } else if token == "guest" {
                    role = Some(crate::serve::oauth::Role::Guest);
                } else if let Some((u, p)) = token.split_once(":") {
                    if let Some(ref local_cfg) = state.config.notes.local.as_ref() {
                        role = crate::serve::oauth::verify_local_credentials(u, p, local_cfg);
                    }
                }
                role
            } else {
                None
            }
        }
    };

    let role = match user_role {
        Some(r) => r,
        None => {
            return (StatusCode::UNAUTHORIZED, "Unauthorized: Invalid or missing authentication").into_response();
        }
    };



    // 3. Parse JSON-RPC Request
    let req: McpJsonRpcRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            let resp = McpJsonRpcResponse::error(None, -32700, format!("Parse error: {}", e), None);
            return (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                serde_json::to_string(&resp).unwrap(),
            )
                .into_response();
        }
    };

    // 4. Validate Header-Body consistency (Mcp-Method, Mcp-Name)
    let header_method = headers.get("mcp-method").and_then(|v| v.to_str().ok());
    let header_name = headers.get("mcp-name").and_then(|v| v.to_str().ok());

    if let Err(mismatch_msg) = validate_header_body_consistency(header_method, header_name, &req) {
        warn!(error = %mismatch_msg, "Header-Body mismatch in MCP request");
        let resp = McpJsonRpcResponse::header_mismatch(req.id, mismatch_msg);
        return (
            StatusCode::OK,
            [
                ("content-type", "application/json"),
                ("x-accel-buffering", "no"),
            ],
            serde_json::to_string(&resp).unwrap(),
        )
            .into_response();
    }

    // 5. Dispatch MCP Methods
    match req.method.as_str() {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false }
                },
                "serverInfo": {
                    "name": "rune-notes",
                    "version": env!("CARGO_PKG_VERSION")
                }
            });
            let resp = McpJsonRpcResponse::success(req.id, result);
            json_response(resp)
        }
        "notifications/initialized" => {
            (StatusCode::NO_CONTENT).into_response()
        }
        "ping" => {
            let resp = McpJsonRpcResponse::success(req.id, json!({}));
            json_response(resp)
        }
        "tools/list" => {
            let available_tools = tools::get_available_tools(role.clone());
            let resp = McpJsonRpcResponse::success(req.id, json!({ "tools": available_tools }));
            json_response(resp)
        }
        "tools/call" => {
            let params = req.params.clone().unwrap_or(Value::Null);
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let tool_args = params.get("arguments").cloned().unwrap_or(json!({}));

            match tools::handle_tool_call(&state, tool_name, tool_args, role.clone()).await {
                Ok(res) => {
                    let resp = McpJsonRpcResponse::success(req.id, res);
                    json_response(resp)
                }
                Err(err) => {
                    let resp = McpJsonRpcResponse::success(req.id, json!({
                        "content": [{ "type": "text", "text": format!("Error: {}", err) }],
                        "isError": true
                    }));
                    json_response(resp)
                }
            }
        }
        "resources/list" => {
            let res_list = resources::list_resources(&state).await;
            let resp = McpJsonRpcResponse::success(req.id, json!({ "resources": res_list }));
            json_response(resp)
        }
        "resources/read" => {
            let params = req.params.clone().unwrap_or(Value::Null);
            let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

            match resources::read_resource(&state, uri).await {
                Ok(res) => {
                    let resp = McpJsonRpcResponse::success(req.id, res);
                    json_response(resp)
                }
                Err(err) => {
                    let resp = McpJsonRpcResponse::error(req.id, -32602, err, None);
                    json_response(resp)
                }
            }
        }
        _ => {
            let resp = McpJsonRpcResponse::error(req.id, -32601, format!("Method not found: {}", req.method), None);
            json_response(resp)
        }
    }
}

pub async fn handle_mcp_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "POST")],
        "405 Method Not Allowed: MCP Streamable HTTP endpoint requires POST",
    )
        .into_response()
}

fn json_response(resp: McpJsonRpcResponse) -> Response {
    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("x-accel-buffering", "no"),
        ],
        serde_json::to_string(&resp).unwrap_or_default(),
    )
        .into_response()
}

fn is_origin_allowed(origin: &str) -> bool {
    origin.contains("localhost")
        || origin.contains("127.0.0.1")
        || origin.contains("rune.sylee.org")
        || origin.contains("tail0a1999.ts.net")
}
