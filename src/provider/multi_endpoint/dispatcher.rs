//! Endpoint dispatcher and chat/completions payload builder.

use super::super::{parse_choices, LlmRequest, LlmResponse, LlmToolCall, TokenUsage};
use super::{build_anthropic_payload, parse_anthropic_response};
use super::{build_responses_payload, parse_responses_response};
use anyhow::{anyhow, Result};
use serde_json::Value;
use tracing::debug;

// All types (LlmRequest, LlmResponse, TokenUsage, etc.) are in scope when inlined.

pub fn build_chat_completions_payload(req: &LlmRequest, stream: bool) -> Result<Value> {
    let mut payload = serde_json::to_value(req)?;
    if let Some(obj) = payload.as_object_mut() {
        if let Some(s) = &req.thinking {
            if s != "none" && s != "off" {
                obj.insert("reasoning_effort".to_string(), Value::String(s.clone()));
            }
        }
        if stream {
            obj.insert("stream".to_string(), Value::Bool(true));
        }
    }
    Ok(payload)
}

pub fn build_request_payload_value(req: &LlmRequest, path: &str, stream: bool) -> Result<Value> {
    match path {
        "/chat/completions" => build_chat_completions_payload(req, stream),
        "/responses" => build_responses_payload(req, stream),
        "/v1/messages" => build_anthropic_payload(req, stream),
        _ => Err(anyhow!("unsupported endpoint path: {}", path)),
    }
}

pub fn build_request_payload(req: &LlmRequest, path: &str) -> Result<String> {
    let v = build_request_payload_value(req, path, false)?;
    Ok(serde_json::to_string(&v)?)
}

pub fn parse_response_by_endpoint(v: &Value, path: &str) -> Result<LlmResponse> {
    match path {
        "/chat/completions" => {
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let (content, tool_calls) = parse_choices(v);

            let usage = v
                .get("usage")
                .map(|u| TokenUsage {
                    prompt_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0)
                        as u32,
                    completion_tokens: u
                        .get("completion_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0) as u32,
                    total_tokens: u.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0)
                        as u32,
                })
                .unwrap_or(TokenUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                });

            Ok(LlmResponse {
                model,
                content,
                tool_calls,
                usage,
            })
        }
        "/responses" => parse_responses_response(v),
        "/v1/messages" => parse_anthropic_response(v),
        _ => Err(anyhow!("unsupported endpoint path: {}", path)),
    }
}

pub fn is_retriable_endpoint_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    let keywords = [
        "500",
        "502",
        "503",
        "504",
        "timeout",
        "connection",
        "tls",
        "reset by peer",
    ];
    keywords.iter().any(|&k| msg.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn test_is_retriable_endpoint_error() {
        assert!(is_retriable_endpoint_error(&anyhow!(
            "500 Internal Server Error"
        )));
        assert!(is_retriable_endpoint_error(&anyhow!(
            "request timeout after 30s"
        )));
        assert!(is_retriable_endpoint_error(&anyhow!(
            "connection reset by peer"
        )));

        assert!(!is_retriable_endpoint_error(&anyhow!("401 Unauthorized")));
        assert!(!is_retriable_endpoint_error(&anyhow!("bad json format")));
        assert!(!is_retriable_endpoint_error(&anyhow!("Model not found")));
    }

    #[test]
    fn test_parse_dispatch_unknown_path() {
        let v = Value::Null;
        let res = parse_response_by_endpoint(&v, "/invalid/path");
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("unsupported endpoint path"));
    }
}
