//! GitHub Copilot /v1/messages endpoint (Anthropic Messages API).
//! Used by Claude family models when supported_endpoints includes "/v1/messages".

use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;
use tracing::{debug, warn};

use super::super::{LlmFunction, LlmMessage, LlmRequest, LlmResponse, LlmToolCall, TokenUsage};

/// Map thinking level string to Anthropic effort value (for output_config).
/// Returns None for "off"/"none"/unknown → no thinking block sent.
fn thinking_effort(level: &str) -> Option<&'static str> {
    match level.to_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        _ => None,
    }
}

/// Convert OpenAI-style tool spec to Anthropic format.
fn convert_tool(tool: &Value) -> Value {
    if let Some(func) = tool.get("function") {
        json!({
            "name": func.get("name").cloned().unwrap_or(Value::Null),
            "description": func.get("description").cloned().unwrap_or(json!("")),
            "input_schema": func.get("parameters").cloned().unwrap_or(json!({"type":"object","properties":{}}))
        })
    } else {
        tool.clone()
    }
}

/// Build content array for an assistant message that has tool_calls.
fn build_assistant_content(msg: &LlmMessage) -> Vec<Value> {
    let mut parts = Vec::new();
    if let Some(ref text) = msg.content {
        if !text.is_empty() {
            parts.push(json!({"type": "text", "text": text}));
        }
    }
    if let Some(ref calls) = msg.tool_calls {
        for tc in calls {
            let input: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or_else(|_| json!({"_raw": tc.function.arguments}));
            parts.push(json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.function.name,
                "input": input
            }));
        }
    }
    parts
}

/// Collapse consecutive same-role messages by merging content blocks.
fn collapse_messages(msgs: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for msg in msgs {
        let dominated = out.last().and_then(|prev| {
            if prev.get("role") == msg.get("role") {
                Some(true)
            } else {
                None
            }
        });
        if dominated.is_some() {
            let last = out.last_mut().unwrap();
            // Merge content
            let prev_content = last.get("content").cloned().unwrap_or(Value::Null);
            let new_content = msg.get("content").cloned().unwrap_or(Value::Null);
            let merged = merge_content(prev_content, new_content);
            last.as_object_mut()
                .unwrap()
                .insert("content".to_string(), merged);
        } else {
            out.push(msg);
        }
    }
    out
}

fn merge_content(a: Value, b: Value) -> Value {
    let mut parts = to_content_array(a);
    parts.extend(to_content_array(b));
    Value::Array(parts)
}

fn to_content_array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(arr) => arr,
        Value::String(s) => vec![json!({"type": "text", "text": s})],
        Value::Null => vec![],
        other => vec![other],
    }
}

pub fn build_anthropic_payload(req: &LlmRequest, stream: bool) -> Result<Value> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            if let Some(ref c) = msg.content {
                system_parts.push(c.clone());
            }
            continue;
        }

        if msg.role == "tool" {
            let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
            let content_val = msg.content.clone().unwrap_or_default();
            messages.push(json!({
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": tool_call_id, "content": content_val}]
            }));
            continue;
        }

        if msg.role == "assistant"
            && msg.tool_calls.is_some()
            && !msg.tool_calls.as_ref().unwrap().is_empty()
        {
            let content = build_assistant_content(msg);
            messages.push(json!({"role": "assistant", "content": content}));
            continue;
        }

        // Default: pass content as string or array
        let content: Value = if let Some(ref parts) = msg.content_parts {
            Value::Array(
                parts
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
                    .collect(),
            )
        } else {
            msg.content
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null)
        };

        messages.push(json!({"role": msg.role, "content": content}));
    }

    let messages = collapse_messages(messages);

    let max_tokens = req.max_tokens.unwrap_or(8192);

    let mut payload = json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": stream
    });

    if !system_parts.is_empty() {
        payload
            .as_object_mut()
            .unwrap()
            .insert("system".to_string(), json!(system_parts.join("\n")));
    }

    if let Some(ref tools) = req.tools {
        if !tools.is_empty() {
            let converted: Vec<Value> = tools.iter().map(|t| convert_tool(t)).collect();
            payload
                .as_object_mut()
                .unwrap()
                .insert("tools".to_string(), Value::Array(converted));
        }
    }

    if let Some(ref thinking) = req.thinking {
        if let Some(effort) = thinking_effort(thinking) {
            let obj = payload.as_object_mut().unwrap();
            // New Claude API: thinking.type="adaptive" + output_config.effort
            obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
            obj.insert("output_config".to_string(), json!({ "effort": effort }));
        }
    }

    Ok(payload)
}

pub fn parse_anthropic_response(v: &Value) -> Result<LlmResponse> {
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();

    let usage = if let Some(u) = v.get("usage") {
        TokenUsage {
            prompt_tokens: u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            total_tokens: (u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0)
                + u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0))
                as u32,
        }
    } else {
        TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    };

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();

    if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    tool_calls.push(LlmToolCall {
                        id,
                        call_type: "function".to_string(),
                        function: LlmFunction {
                            name,
                            arguments: serde_json::to_string(&input).unwrap_or_default(),
                        },
                    });
                }
                _ => {}
            }
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage,
        model,
    })
}

pub async fn stream_anthropic_messages(
    builder: reqwest::RequestBuilder,
    tx: Sender<String>,
) -> Result<LlmResponse> {
    let response = builder.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Anthropic API error {}: {}", status, body));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    let mut model = String::new();
    let mut text_content = String::new();
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();
    // Track in-progress tool calls by index
    let mut tool_call_map: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE events (terminated by \n\n)
        while let Some(end) = buffer.find("\n\n") {
            let event_block = buffer[..end].to_string();
            buffer = buffer[end + 2..].to_string();

            let mut event_type = String::new();
            let mut data_str = String::new();

            for line in event_block.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    event_type = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    data_str = rest.to_string();
                } else if line.starts_with("data:") {
                    data_str = line[5..].trim().to_string();
                }
            }

            if data_str == "[DONE]" {
                break;
            }

            let data: Value = match serde_json::from_str(&data_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            match event_type.as_str() {
                "message_start" => {
                    if let Some(msg) = data.get("message") {
                        if let Some(m) = msg.get("model").and_then(|x| x.as_str()) {
                            model = m.to_string();
                        }
                        if let Some(u) = msg.get("usage") {
                            input_tokens =
                                u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        }
                    }
                }
                "content_block_start" => {
                    let idx = data.get("index").and_then(|x| x.as_u64()).unwrap_or(0);
                    if let Some(block) = data.get("content_block") {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            let id = block
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            tool_call_map.insert(idx, (id, name, String::new()));
                        }
                    }
                }
                "content_block_delta" => {
                    let idx = data.get("index").and_then(|x| x.as_u64()).unwrap_or(0);
                    if let Some(delta) = data.get("delta") {
                        match delta.get("type").and_then(|t| t.as_str()) {
                            Some("text_delta") => {
                                if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                    text_content.push_str(text);
                                    let _ = tx.send(text.to_string()).await;
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(partial) =
                                    delta.get("partial_json").and_then(|t| t.as_str())
                                {
                                    if let Some(entry) = tool_call_map.get_mut(&idx) {
                                        entry.2.push_str(partial);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "message_delta" => {
                    if let Some(u) = data.get("usage") {
                        if let Some(ot) = u.get("output_tokens").and_then(|x| x.as_u64()) {
                            output_tokens = ot as u32;
                        }
                    }
                }
                "message_stop" => {}
                _ => {
                    debug!("Unknown SSE event: {}", event_type);
                }
            }
        }
    }

    // Finalize tool calls
    for (_idx, (id, name, args)) in tool_call_map {
        tool_calls.push(LlmToolCall {
            id,
            call_type: "function".to_string(),
            function: LlmFunction {
                name,
                arguments: args,
            },
        });
    }

    let content = if text_content.is_empty() {
        None
    } else {
        Some(text_content)
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage: TokenUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
        },
        model,
    })
}
