//! GitHub Copilot /responses endpoint (OpenAI Responses API).
//! Used by GPT-5.x family models.

use super::super::{
    parse_choices, LlmFunction, LlmMessage, LlmRequest, LlmResponse, LlmToolCall, TokenUsage,
};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::sync::mpsc::Sender;
use tracing::{debug, warn};

/// Build the JSON payload for a /responses API call.
pub fn build_responses_payload(req: &LlmRequest, stream: bool) -> Result<Value> {
    let mut instructions = String::new();
    let mut input: Vec<Value> = Vec::new();

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" => {
                if let Some(ref c) = msg.content {
                    if !instructions.is_empty() {
                        instructions.push('\n');
                    }
                    instructions.push_str(c);
                }
            }
            "user" => {
                let content = msg.content.clone().unwrap_or_default();
                input.push(json!({"role": "user", "content": content}));
            }
            "assistant" => {
                if let Some(ref tool_calls) = msg.tool_calls {
                    // Emit function_call items for each tool call
                    for tc in tool_calls {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.function.name,
                            "arguments": tc.function.arguments
                        }));
                    }
                } else {
                    let content = msg.content.clone().unwrap_or_default();
                    input.push(json!({"role": "assistant", "content": content}));
                }
            }
            "tool" => {
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let output = msg.content.clone().unwrap_or_default();
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            }
            other => {
                warn!("Unknown message role in responses payload: {}", other);
            }
        }
    }

    let mut payload = json!({
        "model": req.model,
        "input": input,
        "stream": stream
    });

    if !instructions.is_empty() {
        payload["instructions"] = Value::String(instructions);
    }

    // Tools: flatten from chat format to responses format
    if let Some(ref tools) = req.tools {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                // Input format: {"type":"function","function":{"name":...,"description":...,"parameters":...}}
                let func = t.get("function")?;
                Some(json!({
                    "type": "function",
                    "name": func.get("name")?,
                    "description": func.get("description").unwrap_or(&Value::Null),
                    "parameters": func.get("parameters").unwrap_or(&Value::Null)
                }))
            })
            .collect();
        if !converted.is_empty() {
            payload["tools"] = Value::Array(converted);
        }
    }

    // Reasoning effort
    if let Some(ref thinking) = req.thinking {
        let t = thinking.to_lowercase();
        if t != "none" && t != "off" && !t.is_empty() {
            // Pass through low/medium/high
            let effort = match t.as_str() {
                "low" | "medium" | "high" => t.clone(),
                _ => "medium".to_string(),
            };
            payload["reasoning"] = json!({"effort": effort});
        }
    }

    if let Some(max) = req.max_tokens {
        payload["max_output_tokens"] = json!(max);
    }

    Ok(payload)
}

/// Parse a non-streaming /responses JSON response into LlmResponse.
pub fn parse_responses_response(v: &Value) -> Result<LlmResponse> {
    let mut content_text = String::new();
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();

    if let Some(output) = v.get("output").and_then(|o| o.as_array()) {
        for item in output {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                        for part in parts {
                            if part.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    content_text.push_str(text);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    tool_calls.push(LlmToolCall {
                        id: call_id,
                        call_type: "function".to_string(),
                        function: LlmFunction { name, arguments },
                    });
                }
                _ => {}
            }
        }
    }

    let usage = if let Some(u) = v.get("usage") {
        TokenUsage {
            prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        }
    } else {
        // Estimate usage
        let prompt_est = 0u32;
        let comp_est = ((content_text.len() + 3) / 4) as u32;
        TokenUsage {
            prompt_tokens: prompt_est,
            completion_tokens: comp_est,
            total_tokens: comp_est,
        }
    };

    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();

    let content = if content_text.is_empty() {
        None
    } else {
        Some(content_text)
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage,
        model,
    })
}

/// Stream a /responses API call, forwarding text deltas via tx and returning the final LlmResponse.
pub async fn stream_responses_api(
    builder: reqwest::RequestBuilder,
    tx: Sender<String>,
) -> Result<LlmResponse> {
    let response = builder.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Responses API returned {}: {}", status, body));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    let mut content_text = String::new();
    // Track function calls by item_id
    let mut fc_meta: BTreeMap<String, (String, String)> = BTreeMap::new(); // item_id -> (call_id, name)
    let mut fc_args: BTreeMap<String, String> = BTreeMap::new(); // item_id -> accumulated arguments
    let mut usage: Option<TokenUsage> = None;
    let mut model = String::from("unknown");

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if line.starts_with("event:") || line.starts_with("event: ") {
                // We parse event type from the subsequent data line contextually
                continue;
            }

            if !line.starts_with("data:") && !line.starts_with("data: ") {
                continue;
            }

            let data = if line.starts_with("data: ") {
                &line[6..]
            } else {
                &line[5..]
            };

            let data = data.trim();

            if data == "[DONE]" {
                break;
            }

            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => {
                    debug!("Skipping unparseable SSE data");
                    continue;
                }
            };

            let event_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match event_type {
                "response.output_text.delta" => {
                    if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str()) {
                        content_text.push_str(delta);
                        let _ = tx.send(delta.to_string()).await;
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(item_id) = parsed.get("item_id").and_then(|v| v.as_str()) {
                        if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str()) {
                            fc_args
                                .entry(item_id.to_string())
                                .or_default()
                                .push_str(delta);
                        }
                    }
                }
                "response.output_item.added" => {
                    if let Some(item) = parsed.get("item") {
                        if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                            let item_id = item
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            fc_meta.insert(item_id.clone(), (call_id, name));
                            fc_args.entry(item_id).or_default();
                        }
                    }
                }
                "response.completed" => {
                    if let Some(resp) = parsed.get("response") {
                        if let Some(u) = resp.get("usage") {
                            usage = Some(TokenUsage {
                                prompt_tokens: u
                                    .get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                completion_tokens: u
                                    .get("output_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                total_tokens: u
                                    .get("total_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                            });
                        }
                        if let Some(m) = resp.get("model").and_then(|m| m.as_str()) {
                            model = m.to_string();
                        }
                    }
                }
                _ => {
                    debug!("Ignoring SSE event type: {}", event_type);
                }
            }
        }
    }

    // Assemble tool calls
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();
    for (item_id, (call_id, name)) in &fc_meta {
        let arguments = fc_args.get(item_id).cloned().unwrap_or_default();
        tool_calls.push(LlmToolCall {
            id: call_id.clone(),
            call_type: "function".to_string(),
            function: LlmFunction {
                name: name.clone(),
                arguments,
            },
        });
    }

    let usage = usage.unwrap_or_else(|| {
        let comp_est = ((content_text.len() + 3) / 4) as u32;
        TokenUsage {
            prompt_tokens: 0,
            completion_tokens: comp_est,
            total_tokens: comp_est,
        }
    });

    let content = if content_text.is_empty() {
        None
    } else {
        Some(content_text)
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage,
        model,
    })
}
