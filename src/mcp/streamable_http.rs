use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
};
use futures::stream::Stream;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::warn;

use crate::mcp::mcp_session::{is_legacy_session_protocol_version, McpSessionStore};
use crate::mcp::resources;
use crate::mcp::server::{
    negotiate_initialize_protocol_version, validate_header_body_consistency,
    validate_header_body_consistency_lenient, McpJsonRpcRequest, McpJsonRpcResponse,
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
                let resp =
                    McpJsonRpcResponse::error(None, -32000, "Forbidden: Invalid Origin", None);
                return (
                    StatusCode::FORBIDDEN,
                    [("content-type", "application/json")],
                    serde_json::to_string(&resp).unwrap_or_default(),
                )
                    .into_response();
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
                } else if let Some((u, p)) = token.split_once(':') {
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
            let resp = McpJsonRpcResponse::error(
                None,
                -32001,
                "Unauthorized: Invalid or missing authentication",
                None,
            );
            return (
                StatusCode::UNAUTHORIZED,
                [("content-type", "application/json")],
                serde_json::to_string(&resp).unwrap_or_default(),
            )
                .into_response();
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
                serde_json::to_string(&resp).unwrap_or_default(),
            )
                .into_response();
        }
    };

    // 4. Validate Header-Body consistency (MCP-Protocol-Version, Mcp-Method, Mcp-Name)
    //
    // If a legacy session (2025-03-26 ~ 2025-11-25) is already established for this
    // request via `Mcp-Session-Id`, that legacy protocol doesn't define these headers
    // at all, so skip the check entirely for those requests (spec Appendix 1, point 3).
    // Otherwise, fall back to strict or Lenient Legacy Client Mode validation depending
    // on config (spec Appendix 2).
    let mcp_session_id = headers.get("mcp-session-id").and_then(|v| v.to_str().ok());
    let active_legacy_session = match mcp_session_id {
        Some(id) => state
            .mcp_sessions
            .touch(id)
            .await
            .filter(|s| is_legacy_session_protocol_version(&s.protocol_version)),
        None => None,
    };

    if active_legacy_session.is_none() {
        let header_protocol_version = headers
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok());
        let header_method = headers.get("mcp-method").and_then(|v| v.to_str().ok());
        let header_name = headers.get("mcp-name").and_then(|v| v.to_str().ok());

        let validation = if state.config.notes.mcp_lenient_legacy_clients {
            validate_header_body_consistency_lenient(
                header_protocol_version,
                header_method,
                header_name,
                &req,
            )
        } else {
            validate_header_body_consistency(
                header_protocol_version,
                header_method,
                header_name,
                &req,
            )
        };

        if let Err((code, err_msg)) = validation {
            warn!(error = %err_msg, code = code, "Header-Body validation failed in MCP request");
            let resp = if code == -32021 {
                McpJsonRpcResponse::unsupported_protocol_version(req.id, err_msg)
            } else {
                McpJsonRpcResponse::header_mismatch(req.id, err_msg)
            };
            return (
                StatusCode::BAD_REQUEST,
                [
                    ("content-type", "application/json"),
                    ("x-accel-buffering", "no"),
                ],
                serde_json::to_string(&resp).unwrap_or_default(),
            )
                .into_response();
        }
    }

    // 5. Dispatch MCP Methods
    match req.method.as_str() {
        "initialize" => {
            let requested_version = req
                .params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let legacy_session_id = if is_legacy_session_protocol_version(requested_version) {
                Some(state.mcp_sessions.create(requested_version).await)
            } else {
                None
            };

            let negotiated_version = negotiate_initialize_protocol_version(requested_version);

            let result = json!({
                "protocolVersion": negotiated_version,
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

            match legacy_session_id {
                Some(session_id) => (
                    StatusCode::OK,
                    [
                        ("content-type", "application/json"),
                        ("mcp-session-id", session_id.as_str()),
                    ],
                    serde_json::to_string(&resp).unwrap_or_default(),
                )
                    .into_response(),
                None => json_response(resp),
            }
        }
        "notifications/initialized" => (StatusCode::ACCEPTED).into_response(),
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
                    let resp = McpJsonRpcResponse::success(
                        req.id,
                        json!({
                            "content": [{ "type": "text", "text": format!("Error: {}", err) }],
                            "isError": true
                        }),
                    );
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
            let resp = McpJsonRpcResponse::error(
                req.id,
                -32601,
                format!("Method not found: {}", req.method),
                None,
            );
            (
                StatusCode::NOT_FOUND,
                [("content-type", "application/json")],
                serde_json::to_string(&resp).unwrap_or_default(),
            )
                .into_response()
        }
    }
}

/// A wrapper `Stream` that clears the session's `sse_open` flag as soon as the
/// underlying stream is dropped (client disconnect, server shutdown of the
/// connection, or normal completion) — not just via the periodic `touch()` loop.
///
/// This closes the gap described in MCP_Backward_Compat_GET_Stream_Spec.md Appendix 1
/// where a client that disconnects and immediately reconnects with the same
/// `Mcp-Session-Id` (e.g. the official Python MCP SDK's `handle_get_stream` retry loop)
/// would otherwise see a stale `sse_open = true` and get bounced with 409 Conflict,
/// which the SDK counts as a failed reconnection attempt and can exhaust its retry
/// budget, permanently losing the stream.
struct SseOpenGuardStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>,
    sessions: McpSessionStore,
    session_id: String,
}

impl Stream for SseOpenGuardStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for SseOpenGuardStream {
    fn drop(&mut self) {
        let sessions = self.sessions.clone();
        let session_id = self.session_id.clone();
        // Drop can't be async; spawn a short-lived task to clear the flag. If the
        // runtime is already shutting down this is a best-effort no-op (session
        // will still be cleaned up by the TTL sweep).
        tokio::spawn(async move {
            sessions.set_sse_open(&session_id, false).await;
        });
    }
}

/// GET /mcp — legacy (2025-03-26 ~ 2025-11-25) SSE stream compatibility endpoint.
///
/// Per MCP_Backward_Compat_GET_Stream_Spec.md Appendix 1: a valid `Mcp-Session-Id` for
/// a legacy-protocol session is required, otherwise this remains 405 Method Not Allowed
/// (matching the current 2026-07-28-only behavior). Servers speaking only 2026-07-28
/// have no legacy sessions in the store, so this endpoint degrades to a plain 405 for them.
pub async fn handle_mcp_get(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    let session_id = match headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) {
        Some(id) => id.to_string(),
        None => return handle_mcp_not_allowed().await,
    };

    let session = match state.mcp_sessions.get(&session_id).await {
        Some(s) if is_legacy_session_protocol_version(&s.protocol_version) => s,
        _ => return handle_mcp_not_allowed().await,
    };

    if session.sse_open {
        // Same session already has an open GET stream.
        return (
            StatusCode::CONFLICT,
            "409 Conflict: session already has an open GET stream",
        )
            .into_response();
    }

    state.mcp_sessions.set_sse_open(&session_id, true).await;

    let sessions = state.mcp_sessions.clone();
    let sid_for_stream = session_id.clone();
    let inner_stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(20));
        loop {
            interval.tick().await;
            // Refresh last-seen and bail out once the session has been removed
            // (e.g. via DELETE) so the connection is closed server-side too.
            if sessions.touch(&sid_for_stream).await.is_none() {
                break;
            }
            yield Ok::<Event, Infallible>(Event::default().comment(""));
        }
    };

    // Wrap in the drop-guard so `sse_open` is cleared the moment this stream is torn
    // down for any reason (client disconnect, task cancellation, normal end), rather
    // than relying solely on the periodic touch()/TTL sweep.
    let stream = SseOpenGuardStream {
        inner: Box::pin(inner_stream),
        sessions: state.mcp_sessions.clone(),
        session_id: session_id.clone(),
    };

    sse_response(stream)
}

/// DELETE /mcp — terminate a legacy MCP session.
pub async fn handle_mcp_delete(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    let session_id = match headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) {
        Some(id) => id.to_string(),
        None => return handle_mcp_not_allowed().await,
    };

    if state.mcp_sessions.remove(&session_id).await {
        StatusCode::OK.into_response()
    } else {
        handle_mcp_not_allowed().await
    }
}

fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_origin_allowed() {
        assert!(is_origin_allowed("http://localhost:9527"));
        assert!(is_origin_allowed("http://127.0.0.1:9527"));
        assert!(is_origin_allowed("https://rune.sylee.org"));
        assert!(is_origin_allowed("https://foo.tail0a1999.ts.net"));
        assert!(!is_origin_allowed("https://evil.example.com"));
    }

    #[tokio::test]
    async fn test_sse_open_guard_stream_passes_through_items() {
        use futures::StreamExt;

        let sessions = McpSessionStore::new();
        let session_id = sessions.create("2025-11-25").await;
        sessions.set_sse_open(&session_id, true).await;

        let inner = async_stream::stream! {
            yield Ok::<Event, Infallible>(Event::default().comment("hello"));
        };
        let mut guarded = SseOpenGuardStream {
            inner: Box::pin(inner),
            sessions: sessions.clone(),
            session_id: session_id.clone(),
        };

        // Item passes through unchanged.
        assert!(guarded.next().await.is_some());
        // Stream ends (no more items).
        assert!(guarded.next().await.is_none());
    }

    #[tokio::test]
    async fn test_sse_open_guard_stream_clears_flag_on_drop() {
        let sessions = McpSessionStore::new();
        let session_id = sessions.create("2025-11-25").await;
        sessions.set_sse_open(&session_id, true).await;
        assert!(sessions.get(&session_id).await.unwrap().sse_open);

        let inner = async_stream::stream! {
            yield Ok::<Event, Infallible>(Event::default().comment("x"));
        };
        let guarded = SseOpenGuardStream {
            inner: Box::pin(inner),
            sessions: sessions.clone(),
            session_id: session_id.clone(),
        };

        // Dropping the stream (simulating client disconnect / task cancellation)
        // should schedule clearing of sse_open even though we never polled it to
        // completion.
        drop(guarded);

        // Drop spawns a fire-and-forget tokio task to clear the flag; give it a
        // chance to run.
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!sessions.get(&session_id).await.unwrap().sse_open);
    }

    #[tokio::test]
    async fn test_sse_open_guard_stream_clears_flag_after_natural_completion() {
        let sessions = McpSessionStore::new();
        let session_id = sessions.create("2025-11-25").await;
        sessions.set_sse_open(&session_id, true).await;

        {
            use futures::StreamExt;
            let inner = async_stream::stream! {
                yield Ok::<Event, Infallible>(Event::default().comment("x"));
            };
            let mut guarded = SseOpenGuardStream {
                inner: Box::pin(inner),
                sessions: sessions.clone(),
                session_id: session_id.clone(),
            };
            while guarded.next().await.is_some() {}
            // guarded dropped at end of this scope
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!sessions.get(&session_id).await.unwrap().sse_open);
    }
}
