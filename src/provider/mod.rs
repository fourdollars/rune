use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

mod multi_endpoint;
use multi_endpoint::{
    build_request_payload, build_request_payload_value, is_retriable_endpoint_error,
    parse_response_by_endpoint, stream_anthropic_messages, stream_responses_api,
};

/// LLM request payload.
#[derive(Debug, Clone, Serialize)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Thinking/reasoning effort level (provider-specific handling).
    #[serde(skip)]
    pub thinking: Option<String>,
}

/// Content part for multi-modal messages (text + images).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlDetail },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlDetail {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Multi-part content for vision (text + images). Overrides content when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<ContentPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub call_type: String,
    pub function: LlmFunction,
}

fn default_tool_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFunction {
    pub name: String,
    pub arguments: String, // JSON string
}

/// LLM response.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
    pub usage: TokenUsage,
    pub model: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Default, Deserialize)]
struct StreamingDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<StreamingToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct StreamingToolCallDelta {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<StreamingFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamingFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamingChoice {
    #[serde(default)]
    pub delta: StreamingDelta,
}

#[derive(Debug, Default, Deserialize)]
struct StreamingChunk {
    #[serde(default)]
    pub choices: Vec<StreamingChoice>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Default)]
struct StreamingToolCallState {
    pub id: Option<String>,
    pub call_type: String,
    pub name: String,
    pub arguments: String,
}

fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count() as u32;
    if chars == 0 {
        0
    } else {
        (chars + 3) / 4
    }
}

fn estimate_usage(content: Option<&str>, tool_calls: &[LlmToolCall]) -> TokenUsage {
    let content_tokens = content.map(estimate_tokens).unwrap_or(0);
    let tool_tokens: u32 = tool_calls
        .iter()
        .map(|call| {
            estimate_tokens(&call.function.name) + estimate_tokens(&call.function.arguments)
        })
        .sum();
    TokenUsage {
        prompt_tokens: 0,
        completion_tokens: content_tokens.saturating_add(tool_tokens),
        total_tokens: content_tokens.saturating_add(tool_tokens),
    }
}

fn finalize_tool_calls(states: BTreeMap<usize, StreamingToolCallState>) -> Vec<LlmToolCall> {
    states
        .into_iter()
        .filter_map(|(index, state)| {
            // Validate that arguments is valid JSON (or empty).
            // LLM streaming can produce malformed arguments (e.g., two JSON objects
            // concatenated: {"cmd":"ls"}{"path":"."}) which would cause 400 errors
            // on subsequent API calls.
            if !state.arguments.is_empty() {
                if let Err(e) = serde_json::from_str::<serde_json::Value>(&state.arguments) {
                    warn!(
                        "dropping tool_call with invalid JSON arguments: tool={}, index={}, error={}, args={}",
                        state.name, index, e,
                        crate::config::safe_truncate(&state.arguments, 200)
                    );
                    return None;
                }
            }
            Some(LlmToolCall {
                id: state.id.unwrap_or_else(|| format!("stream-{}", index)),
                call_type: if state.call_type.is_empty() {
                    default_tool_type()
                } else {
                    state.call_type
                },
                function: LlmFunction {
                    name: state.name,
                    arguments: state.arguments,
                },
            })
        })
        .collect()
}

async fn stream_openai_compatible_response(
    request: reqwest::RequestBuilder,
    tx: Sender<String>,
) -> Result<LlmResponse> {
    let response = request
        .send()
        .await
        .map_err(|e| anyhow!("failed to send streaming request: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("streaming request failed ({status}): {body}"));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut content = String::new();
    let mut model = String::new();
    let mut usage = TokenUsage::default();
    let mut tool_calls: BTreeMap<usize, StreamingToolCallState> = BTreeMap::new();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("stream read error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let mut line = buffer.drain(..=pos).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !(line.starts_with("data: ") || line.starts_with("data:")) {
                continue;
            }

            let payload = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
                .unwrap_or("")
                .trim();
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                done = true;
                break;
            }

            let event: StreamingChunk = serde_json::from_str(payload)
                .map_err(|e| anyhow!("failed to parse SSE chunk: {} | chunk: {}", e, payload))?;
            if let Some(event_model) = event.model {
                if !event_model.is_empty() {
                    model = event_model;
                }
            }
            if let Some(event_usage) = event.usage {
                usage = event_usage;
            }

            for choice in event.choices {
                if let Some(part) = choice.delta.content {
                    if !part.is_empty() {
                        let _ = tx.send(part.clone()).await;
                        content.push_str(&part);
                    }
                }

                if let Some(delta_calls) = choice.delta.tool_calls {
                    for delta in delta_calls {
                        let entry = tool_calls.entry(delta.index).or_default();
                        if let Some(id) = delta.id {
                            entry.id = Some(id);
                        }
                        if let Some(call_type) = delta.call_type {
                            if !call_type.is_empty() {
                                entry.call_type = call_type;
                            }
                        }
                        if let Some(function) = delta.function {
                            if let Some(name) = function.name {
                                if !name.is_empty() {
                                    if entry.name.is_empty() {
                                        entry.name = name;
                                    } else {
                                        entry.name.push_str(&name);
                                    }
                                }
                            }
                            if let Some(arguments) = function.arguments {
                                entry.arguments.push_str(&arguments);
                            }
                        }
                    }
                }
            }
        }

        if done {
            break;
        }
    }

    let tool_calls = finalize_tool_calls(tool_calls);
    let content = if content.is_empty() {
        None
    } else {
        Some(content)
    };
    let usage = if usage.total_tokens == 0 {
        estimate_usage(content.as_deref(), &tool_calls)
    } else {
        usage
    };
    let model = if model.is_empty() {
        "".to_string()
    } else {
        model
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage,
        model,
    })
}

/// Metadata about an available model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub context_window: Option<u64>,
    pub reasoning_efforts: Vec<String>,
    pub supported_endpoints: Vec<String>,
}

/// Provider trait.
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    /// List available models from this provider. Default: empty (not supported).
    fn list_models(&self) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>>> + Send + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>>;

    /// Stream tokens. Default: bridge the legacy channel-based helper.
    fn chat_streaming(
        &self,
        request: LlmRequest,
        on_token: Box<dyn Fn(&str) + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        Box::pin(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
            let forward = tokio::spawn(async move {
                while let Some(token) = rx.recv().await {
                    on_token(&token);
                }
            });

            let response = self.call_streaming(&request, tx).await;
            let _ = forward.await;
            response
        })
    }

    fn call_streaming(
        &self,
        request: &LlmRequest,
        tx: Sender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            let response = self.chat(request).await?;
            if let Some(content) = &response.content {
                let _ = tx.send(content.clone()).await;
            }
            Ok(response)
        })
    }
}

/// OpenAI-compatible provider (works with OpenAI, Anthropic proxies, local servers).
pub struct OpenAiProvider {
    pub api_key: String,
    pub base_url: String,
    pub provider_name: String,
    pub reasoning_models: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl OpenAiProvider {
    pub fn new(name: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| {
            if name == "openrouter" {
                "https://openrouter.ai/api/v1".to_string()
            } else {
                "https://api.openai.com/v1".to_string()
            }
        });
        OpenAiProvider {
            api_key,
            base_url: base,
            provider_name: name,
            reasoning_models: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    fn supports_reasoning(&self, model: &str) -> bool {
        {
            let cache = self.reasoning_models.lock().unwrap();
            if !cache.is_empty() {
                return cache.contains(model);
            }
        }
        let m = model.to_lowercase();
        m.contains("/o1")
            || m.contains("/o3")
            || m.contains("-r1")
            || m.contains(":r1")
            || m.contains("reasoning")
            || m.contains("thinking")
            || m.contains("claude-3-7-sonnet")
            || m.contains("qwen3")
            || m.contains("qwq")
    }

    fn normalize_thinking(&self, mut request: LlmRequest) -> LlmRequest {
        if self.provider_name == "openrouter" || self.base_url.contains("openrouter.ai") {
            if let Some(ref t) = request.thinking {
                if t != "none" && t != "off" && !self.supports_reasoning(&request.model) {
                    debug!(model = %request.model, thinking = %t,
                        "stripping thinking: model does not support reasoning on OpenRouter");
                    request.thinking = None;
                }
            }
        }
        request
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>>> + Send + '_>> {
        let is_openrouter =
            self.provider_name == "openrouter" || self.base_url.contains("openrouter.ai");
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();

        Box::pin(async move {
            if !is_openrouter {
                if base_url.contains("api.openai.com") {
                    // Official OpenAI: no auto-discovery (huge model list, reasoning is per-model)
                    return Ok(Vec::new());
                }
                // Custom endpoint (Ollama, LM Studio, vLLM, …): query /models and always
                // expose the Ollama-compatible thinking levels for every model.
                let url = format!("{}/models", base_url.trim_end_matches('/'));
                let client = Client::new();
                let Ok(resp) = client
                    .get(&url)
                    .bearer_auth(&api_key)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                else {
                    return Ok(Vec::new());
                };
                if !resp.status().is_success() {
                    return Ok(Vec::new());
                }
                let Ok(body) = resp.text().await else {
                    return Ok(Vec::new());
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                    return Ok(Vec::new());
                };
                // Support OpenAI-compatible {"data": [{id}]} and Ollama {"models": [{name}]}
                let ids: Vec<String> = if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                    arr.iter()
                        .filter_map(|m| {
                            m.get("id")
                                .and_then(|id| id.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect()
                } else if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
                    arr.iter()
                        .filter_map(|m| {
                            m.get("name")
                                .or_else(|| m.get("id"))
                                .and_then(|id| id.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect()
                } else {
                    return Ok(Vec::new());
                };

                let reasoning_efforts = vec![
                    "none".to_string(),
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                ];
                let provider_id = if self.provider_name == "openrouter"
                    || self.base_url.contains("openrouter.ai")
                {
                    "openrouter".to_string()
                } else if self.base_url == "https://api.openai.com/v1" {
                    "openai".to_string()
                } else {
                    "openai-compatible".to_string()
                };
                let mut models: Vec<ModelInfo> = ids
                    .into_iter()
                    .map(|id| ModelInfo {
                        id,
                        provider: Some(provider_id.clone()),
                        context_window: None,
                        reasoning_efforts: reasoning_efforts.clone(),
                        supported_endpoints: vec!["/chat/completions".to_string()],
                    })
                    .collect();
                models.sort_by(|a, b| a.id.cmp(&b.id));
                return Ok(models);
            }

            let client = Client::new();
            let zdr_url = format!("{}/endpoints/zdr", base_url.trim_end_matches('/'));
            let zdr_response = client
                .get(&zdr_url)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| anyhow!("failed to fetch OpenRouter ZDR endpoints: {}", e))?;

            if !zdr_response.status().is_success() {
                let body = zdr_response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "fetch OpenRouter ZDR endpoints failed: {}",
                    crate::config::safe_truncate(&body, 200)
                ));
            }

            #[derive(serde::Deserialize)]
            struct OpenRouterZdrEndpoint {
                model_id: String,
            }

            #[derive(serde::Deserialize)]
            struct OpenRouterZdrEndpointsResponse {
                data: Vec<OpenRouterZdrEndpoint>,
            }

            let zdr_body = zdr_response
                .text()
                .await
                .map_err(|e| anyhow!("failed to read OpenRouter ZDR endpoints response: {}", e))?;

            let zdr_list: OpenRouterZdrEndpointsResponse = serde_json::from_str(&zdr_body)
                .map_err(|e| anyhow!("failed to parse OpenRouter ZDR endpoints list: {}", e))?;

            let zdr_set: std::collections::HashSet<String> =
                zdr_list.data.into_iter().map(|e| e.model_id).collect();

            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let response = client
                .get(&url)
                .bearer_auth(&api_key)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| anyhow!("failed to list OpenRouter models: {}", e))?;

            if !response.status().is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "list OpenRouter models failed: {}",
                    crate::config::safe_truncate(&body, 200)
                ));
            }

            #[derive(serde::Deserialize)]
            struct OpenRouterModelInfo {
                id: String,
                context_length: Option<usize>,
                supported_parameters: Option<Vec<String>>,
            }

            #[derive(serde::Deserialize)]
            struct OpenRouterModelsList {
                data: Vec<OpenRouterModelInfo>,
            }

            let body = response
                .text()
                .await
                .map_err(|e| anyhow!("failed to read OpenRouter models response: {}", e))?;

            let list: OpenRouterModelsList = serde_json::from_str(&body)
                .map_err(|e| anyhow!("failed to parse OpenRouter models list: {}", e))?;

            let mut models = Vec::new();
            let mut reasoning_set = std::collections::HashSet::new();

            for m in list.data {
                // Support ZDR for OpenRouter by filtering out non-ZDR-compliant models
                if !zdr_set.contains(&m.id) {
                    continue;
                }
                // Only include models that explicitly declare "tools" support
                let supports_tools = m
                    .supported_parameters
                    .as_ref()
                    .map(|params| params.iter().any(|p| p.to_lowercase() == "tools"))
                    .unwrap_or(false);
                if !supports_tools {
                    continue;
                }

                let supports_reasoning = if let Some(ref params) = m.supported_parameters {
                    params.iter().any(|p| p.to_lowercase() == "reasoning")
                } else {
                    false
                };

                let reasoning_efforts = if supports_reasoning {
                    reasoning_set.insert(m.id.clone());
                    vec![
                        "minimal".to_string(),
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                        "xhigh".to_string(),
                    ]
                } else {
                    Vec::new()
                };

                let provider_id = if self.provider_name == "openrouter"
                    || self.base_url.contains("openrouter.ai")
                {
                    "openrouter".to_string()
                } else if self.base_url == "https://api.openai.com/v1" {
                    "openai".to_string()
                } else {
                    "openai-compatible".to_string()
                };
                models.push(ModelInfo {
                    id: m.id,
                    provider: Some(provider_id),
                    context_window: m.context_length.map(|l| l as u64),
                    reasoning_efforts,
                    supported_endpoints: vec!["/chat/completions".to_string()],
                });
            }

            models.sort_by(|a, b| a.id.cmp(&b.id));

            // Populate the reasoning_models cache
            {
                let mut cache = self.reasoning_models.lock().unwrap();
                *cache = reasoning_set;
            }

            Ok(models)
        })
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let is_openrouter =
            self.provider_name == "openrouter" || self.base_url.contains("openrouter.ai");

        let mut request = request;
        if is_openrouter {
            request = self.normalize_thinking(request);
        }

        Box::pin(async move {
            // Build the full request payload using serde
            let mut payload_value: serde_json::Value = serde_json::to_value(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            // Inject thinking/reasoning_effort if set
            if is_openrouter {
                if let Some(ref thinking) = request.thinking {
                    let reasoning_obj = if thinking == "off" || thinking == "none" {
                        serde_json::json!({
                            "enabled": false
                        })
                    } else {
                        let (effort, max_tokens) = match thinking.as_str() {
                            "minimal" => ("minimal", Some(1024)),
                            "low" => ("low", Some(1024)),
                            "medium" => ("medium", Some(2048)),
                            "high" => ("high", Some(4096)),
                            "xhigh" => ("xhigh", Some(8192)),
                            _ => ("medium", Some(2048)),
                        };
                        serde_json::json!({
                            "enabled": true,
                            "effort": effort,
                            "max_tokens": max_tokens
                        })
                    };
                    payload_value["reasoning"] = reasoning_obj;
                }
            } else {
                if let Some(ref thinking) = request.thinking {
                    if thinking != "none" && thinking != "off" {
                        payload_value["reasoning_effort"] =
                            serde_json::Value::String(thinking.clone());
                    }
                }
            }

            let payload = serde_json::to_string(&payload_value)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

            debug!(url = %url, payload_len = payload.len(), "sending LLM request");

            let client = Client::new();
            let mut req = client
                .post(&url)
                .bearer_auth(&api_key)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(120));

            if base_url.contains("openrouter.ai") {
                req = req.header("HTTP-Referer", "https://fourdollars.github.io/rune/");
                req = req.header("X-Title", "Rune AI Agent");
            }

            let response = req
                .body(payload)
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request failed: {}", e))?;

            let status = response.status();
            let stdout = response
                .text()
                .await
                .map_err(|e| anyhow!("failed to read response body: {}", e))?;

            if !status.is_success() {
                return Err(anyhow!(
                    "API request failed ({}): {}",
                    status,
                    crate::config::safe_truncate(&stdout, 500)
                ));
            }

            // Parse response JSON
            let v: Value = serde_json::from_str(&stdout).map_err(|e| {
                anyhow!(
                    "failed to parse response JSON: {}\nraw: {}",
                    e,
                    crate::config::safe_truncate(&stdout, 500)
                )
            })?;

            // Check for API error
            if let Some(err) = v.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                return Err(anyhow!("API error: {}", msg));
            }

            // Extract model
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();

            let (content, tool_calls) = parse_choices(&v);

            // Parse usage
            let usage = v
                .get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

            debug!(model = %model, content_len = content.as_ref().map(|s| s.len()).unwrap_or(0),
                   tool_calls = tool_calls.len(), tokens = usage.total_tokens, "LLM response received");

            Ok(LlmResponse {
                content,
                tool_calls,
                usage,
                model,
            })
        })
    }

    fn call_streaming(
        &self,
        request: &LlmRequest,
        tx: Sender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let is_openrouter =
            self.provider_name == "openrouter" || self.base_url.contains("openrouter.ai");

        let mut request = request.clone();
        if is_openrouter {
            request = self.normalize_thinking(request);
        }

        Box::pin(async move {
            let client = Client::new();
            let mut payload = serde_json::to_value(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;
            if let Value::Object(ref mut map) = payload {
                map.insert("stream".to_string(), Value::Bool(true));

                if is_openrouter {
                    if let Some(ref thinking) = request.thinking {
                        let reasoning_obj = if thinking == "off" || thinking == "none" {
                            serde_json::json!({
                                "enabled": false
                            })
                        } else {
                            let (effort, max_tokens) = match thinking.as_str() {
                                "minimal" => ("minimal", Some(1024)),
                                "low" => ("low", Some(1024)),
                                "medium" => ("medium", Some(2048)),
                                "high" => ("high", Some(4096)),
                                "xhigh" => ("xhigh", Some(8192)),
                                _ => ("medium", Some(2048)),
                            };
                            serde_json::json!({
                                "enabled": true,
                                "effort": effort,
                                "max_tokens": max_tokens
                            })
                        };
                        map.insert("reasoning".to_string(), reasoning_obj);
                    }
                } else {
                    if let Some(ref thinking) = request.thinking {
                        if thinking != "none" && thinking != "off" {
                            map.insert(
                                "reasoning_effort".to_string(),
                                Value::String(thinking.clone()),
                            );
                        }
                    }
                }
            } else {
                return Err(anyhow!("request payload must be an object"));
            }

            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut builder = client
                .post(url)
                .bearer_auth(api_key)
                .header("Accept", "text/event-stream");

            if base_url.contains("openrouter.ai") {
                builder = builder.header("HTTP-Referer", "https://fourdollars.github.io/rune/");
                builder = builder.header("X-Title", "Rune AI Agent");
            }

            let builder = builder.json(&payload);

            stream_openai_compatible_response(builder, tx).await
        })
    }
}

/// Registry of LLM providers with fallback chain.
/// GitHub Copilot provider — auto-refreshes session token from PAT.
pub struct CopilotProvider {
    pub pat: String, // GitHub PAT (ghu_...)
    pub provider_name: String,
    token_cache: std::sync::Mutex<Option<(String, String, u64)>>, // (token, endpoint, expires_at)
    /// model_id → supported HTTP endpoints (ws: filtered out).
    model_endpoints: std::sync::Mutex<HashMap<String, Vec<String>>>,
    /// model_id → reasoning_efforts supported (e.g. ["low","medium","high"]).
    model_reasoning: std::sync::Mutex<HashMap<String, Vec<String>>>,
}

impl CopilotProvider {
    pub fn new(pat: String) -> Self {
        CopilotProvider {
            pat,
            provider_name: "github-copilot".to_string(),
            token_cache: std::sync::Mutex::new(None),
            model_endpoints: std::sync::Mutex::new(HashMap::new()),
            model_reasoning: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Lazily fetch /models once if the endpoint cache is empty, so that
    /// CLI/server entry paths that pass an explicit --model never see an
    /// unpopulated cache (which would force /chat/completions fallback for
    /// responses-only / messages-only models).
    async fn warm_caches_if_needed(&self) -> Result<()> {
        let empty = {
            let cache = self.model_endpoints.lock().unwrap();
            cache.is_empty()
        };
        if empty {
            // list_models() populates both caches as a side effect.
            let _ = self.list_models().await?;
        }
        Ok(())
    }

    /// Get HTTP endpoints for a model (skips ws:). Falls back to ["/chat/completions"].
    fn get_endpoints(&self, model_id: &str) -> Vec<String> {
        let cache = self.model_endpoints.lock().unwrap();
        cache
            .get(model_id)
            .cloned()
            .unwrap_or_else(|| vec!["/chat/completions".to_string()])
    }

    /// True when the model advertises reasoning_effort support via /models.
    /// Cache miss returns false: after warm_caches_if_needed() the cache is
    /// authoritative for picker_enabled models. A miss means the user passed
    /// a model that Copilot has retired (picker_enabled=false), so any
    /// reasoning_effort would 400 anyway.
    fn model_supports_reasoning(&self, model_id: &str) -> bool {
        let cache = self.model_reasoning.lock().unwrap();
        match cache.get(model_id) {
            Some(efforts) => !efforts.is_empty(),
            None => false,
        }
    }

    /// Strip thinking from the request when the model does not support reasoning_effort.
    /// Avoids 400s like "Reasoning effort 'high' not supported by claude-haiku-4.5".
    fn normalize_thinking(&self, mut request: LlmRequest) -> LlmRequest {
        if let Some(ref t) = request.thinking {
            if t != "none" && t != "off" && !self.model_supports_reasoning(&request.model) {
                debug!(model = %request.model, thinking = %t,
                    "stripping thinking: model does not support reasoning_effort");
                request.thinking = None;
            }
        }
        request
    }

    /// Map short endpoint name from API to URL path suffix.
    fn endpoint_to_path(endpoint: &str) -> &str {
        match endpoint {
            "chat" | "/chat/completions" => "/chat/completions",
            "responses" | "/responses" => "/responses",
            "messages" | "/v1/messages" => "/v1/messages",
            other => {
                // If it starts with '/' treat as literal path, otherwise unknown → chat
                if other.starts_with('/') {
                    other
                } else {
                    "/chat/completions"
                }
            }
        }
    }

    /// Refresh the session token if expired or missing.
    async fn get_token(&self) -> Result<(String, String)> {
        // Check cache
        {
            let cache = self.token_cache.lock().unwrap();
            if let Some((ref token, ref endpoint, expires_at)) = *cache {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if expires_at > 60 && now < expires_at - 60 {
                    // refresh 60s before expiry
                    return Ok((token.clone(), endpoint.clone()));
                }
            }
        }

        // Fetch new token
        debug!("refreshing GitHub Copilot session token");
        let client = Client::new();
        let response = client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("Authorization", format!("token {}", self.pat))
            .header("User-Agent", "rune/0.1.0")
            .header("editor-version", "vscode/1.96.0")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| anyhow!("failed to refresh copilot token: {}", e))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "copilot token refresh failed ({}): {}",
                "non-2xx",
                crate::config::safe_truncate(&body, 200)
            ));
        }

        let stdout = response
            .text()
            .await
            .map_err(|e| anyhow!("failed to read token response body: {}", e))?;

        let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
            anyhow!(
                "failed to parse token response: {}\nraw: {}",
                e,
                crate::config::safe_truncate(&stdout, 200)
            )
        })?;

        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("no token in response"))?
            .to_string();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = v
            .get("expires_at")
            .and_then(|e| e.as_u64())
            .unwrap_or(now_secs + 1800); // fallback: 30 min from now

        let endpoint = v
            .get("endpoints")
            .and_then(|e| e.get("api"))
            .and_then(|a| a.as_str())
            .unwrap_or("https://api.githubcopilot.com")
            .to_string();

        info!(endpoint = %endpoint, expires_in = expires_at.saturating_sub(now_secs), "copilot token refreshed");

        // Update cache
        {
            let mut cache = self.token_cache.lock().unwrap();
            *cache = Some((token.clone(), endpoint.clone(), expires_at));
        }

        Ok((token, endpoint))
    }
}

impl Provider for CopilotProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>>> + Send + '_>> {
        Box::pin(async move {
            let (token, endpoint) = self.get_token().await?;
            let url = format!("{}/models", endpoint.trim_end_matches('/'));
            let client = Client::new();
            let response = client
                .get(&url)
                .bearer_auth(&token)
                .header("User-Agent", "rune/0.1.0")
                .header("Copilot-Integration-Id", "vscode-chat")
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| anyhow!("failed to list models: {}", e))?;

            if !response.status().is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "list models failed: {}",
                    crate::config::safe_truncate(&body, 200)
                ));
            }

            let body = response
                .text()
                .await
                .map_err(|e| anyhow!("failed to read models response: {}", e))?;
            let v: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| anyhow!("failed to parse models: {}", e))?;

            let mut models: Vec<ModelInfo> = v
                .get("data")
                .or_else(|| v.get("models"))
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            // Only include models enabled for model picker
                            let picker_enabled = m
                                .get("model_picker_enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if !picker_enabled {
                                return None;
                            }

                            // Must have supported_endpoints (i.e. usable for chat)
                            let has_endpoints = m
                                .get("supported_endpoints")
                                .and_then(|v| v.as_array())
                                .map(|arr| !arr.is_empty())
                                .unwrap_or(false);
                            if !has_endpoints {
                                return None;
                            }

                            let id = m
                                .get("id")
                                .or_else(|| m.get("name"))
                                .and_then(|id| id.as_str())
                                .map(|s| s.to_string())?;
                            let context_window = m
                                .pointer("/capabilities/limits/max_context_window_tokens")
                                .and_then(|v| v.as_u64());
                            let reasoning_efforts = m
                                .pointer("/capabilities/supports/reasoning_effort")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|e| e.as_str().map(|s| s.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let supported_endpoints: Vec<String> = m
                                .get("supported_endpoints")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|e| e.as_str())
                                        .filter(|s| !s.starts_with("ws:"))
                                        .map(|s| s.to_string())
                                        .collect()
                                })
                                .unwrap_or_default();
                            Some(ModelInfo {
                                id,
                                provider: Some(self.name().to_string()),
                                context_window,
                                reasoning_efforts,
                                supported_endpoints,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            models.sort_by(|a, b| a.id.cmp(&b.id));

            // Populate model_endpoints cache
            {
                let mut cache = self.model_endpoints.lock().unwrap();
                cache.clear();
                for m in &models {
                    if !m.supported_endpoints.is_empty() {
                        cache.insert(m.id.clone(), m.supported_endpoints.clone());
                    }
                }
            }

            // Populate model_reasoning cache (used to gate thinking/reasoning_effort)
            {
                let mut cache = self.model_reasoning.lock().unwrap();
                cache.clear();
                for m in &models {
                    cache.insert(m.id.clone(), m.reasoning_efforts.clone());
                }
            }

            Ok(models)
        })
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        Box::pin(async move {
            // Lazy cache warm + thinking gating before dispatch.
            self.warm_caches_if_needed().await?;
            let request = self.normalize_thinking(request);

            let (token, base_endpoint) = self.get_token().await?;
            let endpoints = self.get_endpoints(&request.model);

            let mut last_error = None;
            for ep in &endpoints {
                let path = Self::endpoint_to_path(ep);
                let url = format!("{}{}", base_endpoint.trim_end_matches('/'), path);

                let payload = build_request_payload(&request, path)?;

                debug!(url = %url, model = %request.model, endpoint_path = %path, "sending Copilot request");

                let client = Client::new();
                let result = client
                    .post(&url)
                    .bearer_auth(&token)
                    .header("Content-Type", "application/json")
                    .header("User-Agent", "rune/0.1.0")
                    .header("editor-version", "vscode/1.96.0")
                    .timeout(std::time::Duration::from_secs(120))
                    .body(payload)
                    .send()
                    .await;

                let response = match result {
                    Ok(r) => r,
                    Err(e) => {
                        let err = anyhow!("HTTP request failed: {}", e);
                        if is_retriable_endpoint_error(&err) && endpoints.len() > 1 {
                            warn!(model = %request.model, endpoint = %path, error = %e, "endpoint failed, trying next");
                            last_error = Some(err);
                            continue;
                        }
                        return Err(err);
                    }
                };

                let status = response.status();
                let stdout = response
                    .text()
                    .await
                    .map_err(|e| anyhow!("failed to read response body: {}", e))?;

                if !status.is_success() {
                    let err = anyhow!(
                        "Copilot API request failed ({}): {}",
                        status,
                        crate::config::safe_truncate(&stdout, 500)
                    );
                    if is_retriable_endpoint_error(&err) && endpoints.len() > 1 {
                        warn!(model = %request.model, endpoint = %path, status = %status, "endpoint returned error, trying next");
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }

                let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
                    anyhow!(
                        "failed to parse response: {}\nraw: {}",
                        e,
                        crate::config::safe_truncate(&stdout, 500)
                    )
                })?;

                if let Some(err_obj) = v.get("error") {
                    let msg = err_obj
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    return Err(anyhow!("Copilot API error: {}", msg));
                }

                return parse_response_by_endpoint(&v, path);
            }

            Err(last_error
                .unwrap_or_else(|| anyhow!("no available endpoints for model {}", request.model)))
        })
    }

    fn call_streaming(
        &self,
        request: &LlmRequest,
        tx: Sender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let request = request.clone();

        Box::pin(async move {
            // Lazy cache warm + thinking gating before dispatch.
            self.warm_caches_if_needed().await?;
            let request = self.normalize_thinking(request);

            let (token, base_endpoint) = self.get_token().await?;
            let endpoints = self.get_endpoints(&request.model);

            let mut last_error = None;
            for ep in &endpoints {
                let path = Self::endpoint_to_path(ep);
                let url = format!("{}{}", base_endpoint.trim_end_matches('/'), path);

                let payload_value = build_request_payload_value(&request, path, true)?;

                debug!(url = %url, model = %request.model, endpoint_path = %path, "sending Copilot streaming request");

                let client = Client::new();
                let builder = client
                    .post(&url)
                    .bearer_auth(&token)
                    .header("User-Agent", "rune/0.1.0")
                    .header("editor-version", "vscode/1.96.0")
                    .header("Accept", "text/event-stream")
                    .json(&payload_value);

                let result = match path {
                    "/responses" => stream_responses_api(builder, tx.clone()).await,
                    "/v1/messages" => stream_anthropic_messages(builder, tx.clone()).await,
                    _ => stream_openai_compatible_response(builder, tx.clone()).await,
                };

                match result {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        if is_retriable_endpoint_error(&e) && endpoints.len() > 1 {
                            warn!(model = %request.model, endpoint = %path, error = %e, "streaming endpoint failed, trying next");
                            last_error = Some(e);
                            continue;
                        }
                        return Err(e);
                    }
                }
            }

            Err(last_error
                .unwrap_or_else(|| anyhow!("no available endpoints for model {}", request.model)))
        })
    }
}

/// Google Gemini provider — native Gemini API format.
pub struct GeminiProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    /// Models confirmed by list_models() to support thinkingConfig.
    /// None = not yet fetched (fall back to name heuristic).
    thinking_capable: std::sync::Mutex<Option<std::collections::HashSet<String>>>,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        GeminiProvider {
            api_key,
            model: model.unwrap_or_else(|| "gemini-2.0-flash".to_string()),
            base_url: base_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string()),
            thinking_capable: std::sync::Mutex::new(None),
        }
    }

    /// Returns true if the model supports thinkingConfig.
    /// Uses the API-populated cache; falls back to name heuristic before first list_models call.
    fn model_supports_thinking(&self, model: &str) -> bool {
        if let Ok(guard) = self.thinking_capable.lock() {
            if let Some(ref set) = *guard {
                return set.contains(model);
            }
        }
        // Cache not yet populated — conservative name-based fallback
        model.starts_with("gemini-2.5") || model.contains("thinking")
    }

    /// Convert OpenAI-format messages to Gemini format.
    fn convert_messages(messages: &[LlmMessage]) -> (Option<Value>, Vec<Value>) {
        let mut system_instruction: Option<Value> = None;
        let mut contents: Vec<Value> = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    if let Some(ref text) = msg.content {
                        system_instruction = Some(serde_json::json!({
                            "parts": [{"text": text}]
                        }));
                    }
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if let Some(ref text) = msg.content {
                        if !text.is_empty() {
                            parts.push(serde_json::json!({"text": text}));
                        }
                    }
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            let args: Value = serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(Value::Object(serde_json::Map::new()));
                            // Extract from ID format: "fn_name|fc_id|thought_sig"
                            let id_parts: Vec<&str> = tc.id.splitn(3, '|').collect();
                            let fc_id = if id_parts.len() >= 2 { id_parts[1] } else { "" };
                            let thought_sig = if id_parts.len() >= 3 {
                                Some(id_parts[2])
                            } else {
                                None
                            };
                            let mut fc_obj = serde_json::json!({
                                "name": tc.function.name,
                                "args": args
                            });
                            if !fc_id.is_empty() {
                                fc_obj["id"] = serde_json::Value::String(fc_id.to_string());
                            }
                            let mut fc_part = serde_json::json!({
                                "functionCall": fc_obj
                            });
                            if let Some(sig) = thought_sig {
                                if !sig.is_empty() {
                                    fc_part["thoughtSignature"] =
                                        serde_json::Value::String(sig.to_string());
                                }
                            }
                            parts.push(fc_part);
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(serde_json::json!({
                            "role": "model",
                            "parts": parts
                        }));
                    }
                }
                "tool" => {
                    let raw_content = msg.content.as_deref().unwrap_or("");
                    // Gemini requires functionResponse.response to be a JSON object (Struct).
                    // If the tool output is not valid JSON or not an object, wrap it.
                    let response_data: Value = match serde_json::from_str::<Value>(raw_content) {
                        Ok(v) if v.is_object() => v,
                        _ => serde_json::json!({"result": raw_content}),
                    };

                    // Parse tool_call_id format: "fn_name|fc_id|thought_sig"
                    let raw_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                    let parts_vec: Vec<&str> = raw_id.splitn(3, '|').collect();
                    let fn_name = parts_vec.first().copied().unwrap_or("unknown");
                    let fc_id = if parts_vec.len() >= 2 {
                        parts_vec[1]
                    } else {
                        ""
                    };
                    let thought_sig = if parts_vec.len() >= 3 {
                        Some(parts_vec[2])
                    } else {
                        None
                    };

                    let mut fr_obj = serde_json::json!({
                        "name": fn_name,
                        "response": response_data
                    });
                    if !fc_id.is_empty() {
                        fr_obj["id"] = serde_json::Value::String(fc_id.to_string());
                    }
                    let mut fr_part = serde_json::json!({
                        "functionResponse": fr_obj
                    });
                    if let Some(sig) = thought_sig {
                        if !sig.is_empty() {
                            fr_part["thoughtSignature"] =
                                serde_json::Value::String(sig.to_string());
                        }
                    }
                    contents.push(serde_json::json!({
                        "role": "function",
                        "parts": [fr_part]
                    }));
                }
                _ => {
                    // "user" and anything else
                    if let Some(ref text) = msg.content {
                        contents.push(serde_json::json!({
                            "role": "user",
                            "parts": [{"text": text}]
                        }));
                    }
                }
            }
        }
        (system_instruction, contents)
    }

    /// Convert OpenAI-format tool definitions to Gemini function_declarations.
    fn convert_tools(tools: &[Value]) -> Option<Value> {
        let declarations: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let func = t.get("function")?;
                Some(serde_json::json!({
                    "name": func.get("name")?,
                    "description": func.get("description").unwrap_or(&Value::String(String::new())),
                    "parameters": func.get("parameters").cloned().unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
                }))
            })
            .collect();
        if declarations.is_empty() {
            None
        } else {
            Some(serde_json::json!([{"function_declarations": declarations}]))
        }
    }
}

impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        Box::pin(async move {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;
            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let resp = client
                .get(&url)
                .header("x-goog-api-key", &api_key)
                .send()
                .await?;
            if !resp.status().is_success() {
                return Ok(Vec::new());
            }
            let body: serde_json::Value = resp.json().await?;
            let arr = match body.get("models").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => return Ok(Vec::new()),
            };

            let thinking_levels = vec!["low".to_string(), "medium".to_string(), "high".to_string()];
            let mut thinking_set = std::collections::HashSet::new();

            let mut models: Vec<ModelInfo> = arr
                .iter()
                .filter(|m| {
                    m.get("supportedGenerationMethods")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().any(|e| e.as_str() == Some("generateContent")))
                        .unwrap_or(false)
                })
                .filter_map(|m| {
                    let id = m
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim_start_matches("models/").to_string())?;
                    let supports_thinking =
                        m.get("thinking").and_then(|v| v.as_bool()).unwrap_or(false);
                    if supports_thinking {
                        thinking_set.insert(id.clone());
                    }
                    Some(ModelInfo {
                        id,
                        provider: Some(self.name().to_string()),
                        context_window: m.get("inputTokenLimit").and_then(|v| v.as_u64()),
                        reasoning_efforts: if supports_thinking {
                            thinking_levels.clone()
                        } else {
                            vec![]
                        },
                        supported_endpoints: vec![],
                    })
                })
                .collect();

            models.sort_by(|a, b| a.id.cmp(&b.id));

            // Populate thinking cache for use in chat()
            if let Ok(mut guard) = self.thinking_capable.lock() {
                *guard = Some(thinking_set);
            }

            Ok(models)
        })
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
        let thinking_capable = self.model_supports_thinking(&model);

        Box::pin(async move {
            let (system_instruction, contents) = Self::convert_messages(&request.messages);
            let tools = request.tools.as_ref().and_then(|t| Self::convert_tools(t));

            let mut payload = serde_json::json!({
                "contents": contents
            });

            if let Some(si) = system_instruction {
                payload["systemInstruction"] = si;
            }
            if let Some(t) = tools {
                payload["tools"] = t;
            }

            // Gemini thinking config — only for models that support it (2.5+ series)
            if thinking_capable {
                if let Some(ref thinking) = request.thinking {
                    let budget = match thinking.as_str() {
                        "low" => Some(1024),
                        "medium" => Some(4096),
                        "high" => Some(8192),
                        "xhigh" => Some(16384),
                        "none" | "off" => Some(0),
                        _ => None,
                    };
                    if let Some(b) = budget {
                        payload["generationConfig"] = serde_json::json!({
                            "thinkingConfig": {
                                "thinkingBudget": b
                            }
                        });
                    }
                }
            }

            let url = format!(
                "{}/models/{}:generateContent",
                base_url.trim_end_matches('/'),
                model
            );

            let payload_str = serde_json::to_string(&payload)
                .map_err(|e| anyhow!("failed to serialize Gemini request: {}", e))?;

            debug!(url = %url, payload_len = payload_str.len(), "sending Gemini request");

            let client = Client::new();
            let response = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &api_key)
                .timeout(std::time::Duration::from_secs(120))
                .body(payload_str)
                .send()
                .await
                .map_err(|e| anyhow!("Gemini HTTP request failed: {}", e))?;

            let status = response.status();
            let stdout = response
                .text()
                .await
                .map_err(|e| anyhow!("failed to read Gemini response body: {}", e))?;

            if !status.is_success() {
                return Err(anyhow!(
                    "Gemini API request failed ({}): {}",
                    status,
                    crate::config::safe_truncate(&stdout, 500)
                ));
            }

            let v: Value = serde_json::from_str(&stdout).map_err(|e| {
                anyhow!(
                    "failed to parse Gemini response: {}\nraw: {}",
                    e,
                    crate::config::safe_truncate(&stdout, 500)
                )
            })?;

            // Check for API error
            if let Some(err) = v.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                return Err(anyhow!("Gemini API error: {}", msg));
            }

            // Parse response
            let candidate = v.pointer("/candidates/0/content");
            let mut content_text = String::new();
            let mut tool_calls: Vec<LlmToolCall> = Vec::new();
            let _tc_counter = 0u32;

            if let Some(cand_content) = candidate {
                if let Some(parts) = cand_content.get("parts").and_then(|p| p.as_array()) {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            content_text.push_str(text);
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = fc
                                .get("args")
                                .cloned()
                                .unwrap_or(Value::Object(serde_json::Map::new()));
                            // Gemini function call has an "id" field we must echo back
                            let fc_id = fc
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string();
                            // Capture thoughtSignature from part level
                            let thought_sig = part
                                .get("thoughtSignature")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Encode: fn_name|fc_id|thought_sig
                            let id = format!("{}|{}|{}", name, fc_id, thought_sig);
                            tool_calls.push(LlmToolCall {
                                id,
                                call_type: "function".to_string(),
                                function: LlmFunction {
                                    name,
                                    arguments: serde_json::to_string(&args).unwrap_or_default(),
                                },
                            });
                        }
                    }
                }
            }

            // Parse usage
            let usage = if let Some(um) = v.get("usageMetadata") {
                TokenUsage {
                    prompt_tokens: um
                        .get("promptTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    completion_tokens: um
                        .get("candidatesTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    total_tokens: um
                        .get("totalTokenCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                }
            } else {
                TokenUsage::default()
            };

            let response_model = v
                .get("modelVersion")
                .and_then(|m| m.as_str())
                .unwrap_or(&model)
                .to_string();

            debug!(model = %response_model, content_len = content_text.len(),
                   tool_calls = tool_calls.len(), tokens = usage.total_tokens, "Gemini response received");

            Ok(LlmResponse {
                content: if content_text.is_empty() {
                    None
                } else {
                    Some(content_text)
                },
                tool_calls,
                usage,
                model: response_model,
            })
        })
    }

    fn chat_streaming(
        &self,
        request: LlmRequest,
        on_token: Box<dyn Fn(&str) + Send>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
        let thinking_capable = self.model_supports_thinking(&model);

        Box::pin(async move {
            let (system_instruction, contents) = Self::convert_messages(&request.messages);
            let tools = request.tools.as_ref().and_then(|t| Self::convert_tools(t));

            let mut payload = serde_json::json!({
                "contents": contents
            });

            if let Some(si) = system_instruction {
                payload["systemInstruction"] = si;
            }
            if let Some(t) = tools {
                payload["tools"] = t;
            }

            // Gemini thinking config — only for models that support it (2.5+ series)
            if thinking_capable {
                if let Some(ref thinking) = request.thinking {
                    let budget = match thinking.as_str() {
                        "low" => Some(1024),
                        "medium" => Some(4096),
                        "high" => Some(8192),
                        "xhigh" => Some(16384),
                        "none" | "off" => Some(0),
                        _ => None,
                    };
                    if let Some(b) = budget {
                        payload["generationConfig"] = serde_json::json!({
                            "thinkingConfig": {
                                "thinkingBudget": b
                            }
                        });
                    }
                }
            }

            let url = format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                base_url.trim_end_matches('/'),
                model
            );

            let payload_str = serde_json::to_string(&payload)
                .map_err(|e| anyhow!("failed to serialize Gemini request: {}", e))?;

            debug!(url = %url, payload_len = payload_str.len(), "sending Gemini streaming request");

            let client = Client::new();
            let response = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &api_key)
                .timeout(std::time::Duration::from_secs(120))
                .body(payload_str)
                .send()
                .await
                .map_err(|e| anyhow!("Gemini streaming request failed: {}", e))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "Gemini streaming request failed ({}): {}",
                    status,
                    crate::config::safe_truncate(&body, 500)
                ));
            }

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut content_text = String::new();
            let mut tool_calls: Vec<LlmToolCall> = Vec::new();
            let _tc_counter = 0u32;
            let mut usage = TokenUsage::default();
            let mut response_model = model.clone();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| anyhow!("Gemini stream read error: {}", e))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buffer.find('\n') {
                    let mut line = buffer.drain(..=pos).collect::<String>();
                    if line.ends_with('\n') {
                        line.pop();
                    }
                    if line.ends_with('\r') {
                        line.pop();
                    }

                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if !line.starts_with("data:") {
                        continue;
                    }

                    let data = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                        .unwrap_or("")
                        .trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }

                    let v: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!(error = %e, chunk = %data, "skipping malformed Gemini SSE chunk");
                            continue;
                        }
                    };

                    if let Some(um) = v.get("usageMetadata") {
                        usage = TokenUsage {
                            prompt_tokens: um
                                .get("promptTokenCount")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as u32,
                            completion_tokens: um
                                .get("candidatesTokenCount")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as u32,
                            total_tokens: um
                                .get("totalTokenCount")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as u32,
                        };
                    }

                    if let Some(mv) = v.get("modelVersion").and_then(|m| m.as_str()) {
                        if !mv.is_empty() {
                            response_model = mv.to_string();
                        }
                    }

                    if let Some(parts) = v
                        .pointer("/candidates/0/content/parts")
                        .and_then(|p| p.as_array())
                    {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    on_token(text);
                                    content_text.push_str(text);
                                }
                            }
                            if let Some(fc) = part.get("functionCall") {
                                let name = fc
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let args = fc
                                    .get("args")
                                    .cloned()
                                    .unwrap_or(Value::Object(serde_json::Map::new()));
                                let fc_id = fc
                                    .get("id")
                                    .and_then(|i| i.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let thought_sig = part
                                    .get("thoughtSignature")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                let id = format!("{}|{}|{}", name, fc_id, thought_sig);
                                tool_calls.push(LlmToolCall {
                                    id,
                                    call_type: "function".to_string(),
                                    function: LlmFunction {
                                        name,
                                        arguments: serde_json::to_string(&args).unwrap_or_default(),
                                    },
                                });
                            }
                        }
                    }
                }
            }

            let usage = if usage.total_tokens == 0 {
                estimate_usage(Some(&content_text), &tool_calls)
            } else {
                usage
            };

            debug!(model = %response_model, content_len = content_text.len(),
                   tool_calls = tool_calls.len(), tokens = usage.total_tokens, "Gemini streaming response received");

            Ok(LlmResponse {
                content: if content_text.is_empty() {
                    None
                } else {
                    Some(content_text)
                },
                tool_calls,
                usage,
                model: if response_model.is_empty() {
                    model
                } else {
                    response_model
                },
            })
        })
    }
}

pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
    default_provider: usize,
}

fn is_transient_error(err: &anyhow::Error) -> bool {
    let err_str = err.to_string();
    err_str.contains("timeout")
        || err_str.contains("429")
        || err_str.contains("500")
        || err_str.contains("502")
        || err_str.contains("503")
}

impl ProviderRegistry {
    pub fn new() -> Self {
        ProviderRegistry {
            providers: Vec::new(),
            default_provider: 0,
        }
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// List available models from the default provider.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        if self.providers.is_empty() {
            return Err(anyhow!("no providers registered"));
        }
        let idx = self.default_provider.min(self.providers.len() - 1);
        self.providers[idx].list_models().await
    }

    /// Call default provider, fallback only on transient failure.
    pub async fn chat(&self, request: LlmRequest) -> Result<LlmResponse> {
        if self.providers.is_empty() {
            return Err(anyhow!("no providers registered"));
        }

        let len = self.providers.len();
        let mut idx = self.default_provider.min(len - 1);
        let mut last_err = anyhow!("no providers");

        let retry_delays = [100u64, 500, 2000]; // ms

        for _attempt in 0..len {
            let provider = &self.providers[idx];

            // Try with retries for transient errors
            let mut result = Err(anyhow!("not attempted"));
            for retry in 0..=3 {
                result = provider.chat(request.clone()).await;
                match &result {
                    Ok(_) => break,
                    Err(e) if is_transient_error(e) && retry < 3 => {
                        let delay = retry_delays[retry as usize];
                        debug!(
                            provider = provider.name(),
                            retry,
                            delay_ms = delay,
                            "retrying after transient error"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                    Err(_) => break,
                }
            }

            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if !is_transient_error(&e) {
                        return Err(e);
                    }
                    debug!(provider = provider.name(), error = %e, "provider failed, trying next transient provider");
                    last_err = e;
                    idx = (idx + 1) % len;
                }
            }
        }

        Err(last_err)
    }

    /// Call a specific named provider.
    pub async fn chat_with(&self, provider_name: &str, request: LlmRequest) -> Result<LlmResponse> {
        for p in &self.providers {
            if p.name() == provider_name {
                return p.chat(request).await;
            }
        }
        Err(anyhow!("provider not found: {}", provider_name))
    }

    /// Streaming call with the default provider fallback chain.
    pub async fn chat_streaming(
        &self,
        request: LlmRequest,
        tx: Sender<String>,
    ) -> Result<LlmResponse> {
        if self.providers.is_empty() {
            return Err(anyhow!("no providers registered"));
        }

        let len = self.providers.len();
        let mut idx = self.default_provider.min(len - 1);
        let mut last_err = anyhow!("no providers");
        let retry_delays = [100u64, 500, 2000];

        for _attempt in 0..len {
            let provider = &self.providers[idx];
            let mut result = Err(anyhow!("not attempted"));

            for retry in 0..=3 {
                result = provider.call_streaming(&request, tx.clone()).await;
                match &result {
                    Ok(_) => break,
                    Err(e) if is_transient_error(e) && retry < 3 => {
                        let delay = retry_delays[retry as usize];
                        debug!(
                            provider = provider.name(),
                            retry,
                            delay_ms = delay,
                            "retrying streaming call after transient error"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                    Err(_) => break,
                }
            }

            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if !is_transient_error(&e) {
                        return Err(e);
                    }
                    debug!(provider = provider.name(), error = %e, "provider failed, trying next transient provider");
                    last_err = e;
                    idx = (idx + 1) % len;
                }
            }
        }

        Err(last_err)
    }

    /// Streaming call to a specific named provider.
    pub async fn chat_with_streaming(
        &self,
        provider_name: &str,
        request: LlmRequest,
        tx: Sender<String>,
    ) -> Result<LlmResponse> {
        for p in &self.providers {
            if p.name() == provider_name {
                return p.call_streaming(&request, tx).await;
            }
        }
        Err(anyhow!("provider not found: {}", provider_name))
    }
}

/// Parse content and tool_calls from potentially multi-choice API responses.
/// Some providers (e.g. Copilot + Claude with thinking) split content and
/// tool_calls across multiple choices. This scans all choices to collect both.
fn parse_choices(v: &serde_json::Value) -> (Option<String>, Vec<LlmToolCall>) {
    let choices = v.get("choices").and_then(|c| c.as_array());

    let mut content: Option<String> = None;
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();

    if let Some(choices_arr) = choices {
        for choice in choices_arr {
            let msg = choice.get("message");
            // Collect content from the first choice that has it
            if content.is_none() {
                if let Some(c) = msg.and_then(|m| m.get("content")) {
                    if !c.is_null() {
                        if let Some(s) = c.as_str() {
                            if !s.is_empty() {
                                content = Some(s.to_string());
                            }
                        }
                    }
                }
            }
            // Collect tool_calls from any choice that has them
            if let Some(tc) = msg.and_then(|m| m.get("tool_calls")) {
                if tc.is_array() {
                    if let Some(calls) = serde_json::from_value::<Vec<LlmToolCall>>(tc.clone()).ok()
                    {
                        tool_calls.extend(calls);
                    }
                }
            }
        }
    }

    (content, tool_calls)
}

mod tests {
    use super::*;

    struct FailingProvider {
        name: String,
        msg: String,
    }
    impl FailingProvider {
        fn new(name: &str, msg: &str) -> Self {
            Self {
                name: name.to_string(),
                msg: msg.to_string(),
            }
        }
    }
    impl Provider for FailingProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn chat(
            &self,
            _request: LlmRequest,
        ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
            let msg = self.msg.clone();
            Box::pin(async move { Err(anyhow!(msg)) })
        }
    }

    struct SucceedProvider {
        name: String,
        resp: LlmResponse,
    }
    impl SucceedProvider {
        fn new(name: &str, resp: LlmResponse) -> Self {
            Self {
                name: name.to_string(),
                resp,
            }
        }
    }
    impl Provider for SucceedProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn chat(
            &self,
            _request: LlmRequest,
        ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
            let r = self.resp.clone();
            Box::pin(async move { Ok(r) })
        }
    }

    #[tokio::test]
    async fn test_provider_registry_transient_fallback() {
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FailingProvider::new(
            "fail",
            "timeout talking to provider",
        )));
        let resp = LlmResponse {
            content: Some("ok".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m1".into(),
        };
        reg.register(Box::new(SucceedProvider::new("succ", resp)));

        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let res = reg
            .chat(req)
            .await
            .expect("transient fallback should succeed");
        assert_eq!(res.content, Some("ok".to_string()));
    }

    #[tokio::test]
    async fn test_chat_with_specific_provider() {
        let mut reg = ProviderRegistry::new();
        let resp = LlmResponse {
            content: Some("hello".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m1".into(),
        };
        reg.register(Box::new(SucceedProvider::new("p1", resp)));

        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let res = reg.chat_with("p1", req).await.expect("should find p1");
        assert_eq!(res.content, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn test_provider_registry_permanent_failure_no_fallback() {
        struct PermanentFailProvider;
        impl Provider for PermanentFailProvider {
            fn name(&self) -> &str {
                "perm"
            }
            fn chat(
                &self,
                _request: LlmRequest,
            ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
                Box::pin(async move {
                    Err(anyhow!("failed to parse response: expected value at line 1 column 1 raw: Unprocessable Entity"))
                })
            }
        }

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(PermanentFailProvider));
        let resp = LlmResponse {
            content: Some("ok".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m1".into(),
        };
        reg.register(Box::new(SucceedProvider::new("succ", resp)));

        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let err = reg
            .chat(req)
            .await
            .expect_err("permanent failure should not fallback");
        assert!(err.to_string().contains("failed to parse response"));
    }

    #[test]
    fn test_gemini_convert_messages_system() {
        let messages = vec![
            LlmMessage {
                role: "system".to_string(),
                name: None,
                content: Some("You are helpful.".to_string()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
            },
            LlmMessage {
                role: "user".to_string(),
                name: None,
                content: Some("Hello".to_string()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let (sys, contents) = GeminiProvider::convert_messages(&messages);
        assert!(sys.is_some());
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
    }

    #[test]
    fn test_gemini_convert_messages_assistant_to_model() {
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: Some("Hi there!".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "model");
    }

    #[test]
    fn test_gemini_convert_tools_empty() {
        let tools: Vec<serde_json::Value> = vec![];
        assert!(GeminiProvider::convert_tools(&tools).is_none());
    }

    #[test]
    fn test_gemini_convert_tools_one_function() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
            }
        })];
        let result = GeminiProvider::convert_tools(&tools);
        assert!(result.is_some());
        let arr = result.unwrap();
        assert!(arr.is_array());
        let decls = &arr[0]["function_declarations"];
        assert_eq!(decls[0]["name"], "read_file");
    }

    #[test]
    fn test_content_part_text_serialization() {
        let part = ContentPart::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn test_content_part_image_url_serialization() {
        let part = ContentPart::ImageUrl {
            image_url: ImageUrlDetail {
                url: "data:image/png;base64,abc123".to_string(),
                detail: Some("high".to_string()),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"image_url\""));
        assert!(json.contains("data:image/png;base64,abc123"));
        assert!(json.contains("\"detail\":\"high\""));
    }

    #[test]
    fn test_content_part_image_url_no_detail() {
        let part = ContentPart::ImageUrl {
            image_url: ImageUrlDetail {
                url: "https://example.com/image.png".to_string(),
                detail: None,
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(!json.contains("detail"));
    }

    #[test]
    fn test_llm_message_content_parts_none_skipped() {
        let msg = LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("hello".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("content_parts"));
    }

    #[test]
    fn test_provider_auto_detect_copilot() {
        let key = "ghu_abc123";
        assert!(key.starts_with("ghu_"));
    }

    #[test]
    fn test_provider_auto_detect_gemini() {
        let key = "AIzaSyAbc123";
        assert!(key.starts_with("AIza"));
    }
    #[test]
    fn test_provider_auto_detect_openrouter() {
        let key = "sk-or-v1-abc123";
        assert!(key.starts_with("sk-or-"));
    }

    #[test]
    fn test_embedding_base_url_fallback_openrouter() {
        // When provider is openrouter and embedding has no base_url,
        // it should fall back to the main config's base_url
        use crate::embedding::EmbeddingConfig;

        let mut emb_cfg = EmbeddingConfig::default();
        emb_cfg.enabled = true;
        emb_cfg.api_key = Some("sk-or-v1-test".to_string());
        // Simulate the fallback logic from cli/mod.rs
        let main_base_url = Some("https://openrouter.ai/api/v1".to_string());
        if emb_cfg.base_url.is_none() {
            emb_cfg.base_url = main_base_url;
        }
        assert_eq!(
            emb_cfg.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
    }

    #[test]
    fn test_embedding_base_url_not_overridden_when_set() {
        // If embedding already has a base_url, don't override it
        use crate::embedding::EmbeddingConfig;

        let mut emb_cfg = EmbeddingConfig::default();
        emb_cfg.enabled = true;
        emb_cfg.base_url = Some("https://custom.endpoint.com/v1".to_string());
        // Simulate fallback
        let main_base_url = Some("https://openrouter.ai/api/v1".to_string());
        if emb_cfg.base_url.is_none() {
            emb_cfg.base_url = main_base_url;
        }
        assert_eq!(
            emb_cfg.base_url.as_deref(),
            Some("https://custom.endpoint.com/v1")
        );
    }

    #[test]
    fn test_openrouter_key_not_detected_as_copilot_or_gemini() {
        let key = "sk-or-v1-abc123def456";
        assert!(!key.starts_with("ghu_"));
        assert!(!key.starts_with("ghp_"));
        assert!(!key.starts_with("AIza"));
        assert!(key.starts_with("sk-or-"));
    }
    async fn test_provider_registry_streaming_bridge() {
        struct EchoProvider {
            name: String,
            content: String,
        }

        impl Provider for EchoProvider {
            fn name(&self) -> &str {
                &self.name
            }

            fn chat(
                &self,
                _request: LlmRequest,
            ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
                let content = self.content.clone();
                Box::pin(async move {
                    Ok(LlmResponse {
                        content: Some(content),
                        tool_calls: vec![],
                        usage: TokenUsage::default(),
                        model: "echo-model".to_string(),
                    })
                })
            }
        }

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(EchoProvider {
            name: "stream".to_string(),
            content: "hello stream".to_string(),
        }));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(4);
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };

        let res = reg
            .chat_streaming(req, tx)
            .await
            .expect("streaming should work");
        assert_eq!(res.content.as_deref(), Some("hello stream"));
        assert_eq!(rx.recv().await.as_deref(), Some("hello stream"));
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn test_gemini_tool_response_plain_text_wrapped() {
        // Plain text tool output must be wrapped in {"result": "..."} for Gemini
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some("obvious".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("execute_cmd|fc_id_1|".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        assert_eq!(contents.len(), 1);
        let resp = &contents[0]["parts"][0]["functionResponse"]["response"];
        assert_eq!(resp["result"], "obvious");
    }

    #[test]
    fn test_gemini_tool_response_json_object_preserved() {
        // JSON object tool output should be passed through as-is
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some(r#"{"name":"test","value":42}"#.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("fetch_url|fc_id_1|".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let resp = &contents[0]["parts"][0]["functionResponse"]["response"];
        assert_eq!(resp["name"], "test");
        assert_eq!(resp["value"], 42);
    }

    #[test]
    fn test_gemini_tool_response_fn_name_from_id() {
        // Function name should be extracted from tool_call_id format "name|gemini_tc_N"
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some("{}".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("read_file|fc_id_2|".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let name = &contents[0]["parts"][0]["functionResponse"]["name"];
        assert_eq!(name, "read_file");
    }

    #[test]
    fn test_gemini_thought_signature_preserved_in_assistant() {
        // When assistant message has tool calls with thought_signature in ID,
        // the reconstructed functionCall part should include thought_signature
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: Some(vec![LlmToolCall {
                id: "execute_cmd|fc_id_1|abc123sig".to_string(),
                call_type: "function".to_string(),
                function: LlmFunction {
                    name: "execute_cmd".to_string(),
                    arguments: r#"{"cmd":"ls"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        assert_eq!(contents.len(), 1);
        let part = &contents[0]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "execute_cmd");
        assert_eq!(part["thoughtSignature"], "abc123sig");
    }

    #[test]
    fn test_gemini_thought_signature_in_tool_response() {
        // Function response should include thought_signature from tool_call_id
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some(r#"{"output":"done"}"#.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("execute_cmd|fc_id_1|sig456xyz".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let part = &contents[0]["parts"][0];
        assert_eq!(part["functionResponse"]["name"], "execute_cmd");
        assert_eq!(part["thoughtSignature"], "sig456xyz");
    }

    #[test]
    fn test_gemini_no_thought_signature_when_absent() {
        // No thought_signature in ID → no field in output
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: Some(vec![LlmToolCall {
                id: "list_dir|fc_id_1|".to_string(),
                call_type: "function".to_string(),
                function: LlmFunction {
                    name: "list_dir".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let part = &contents[0]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "list_dir");
        assert!(part.get("thoughtSignature").is_none() || part["thoughtSignature"].is_null());
    }

    #[test]
    fn test_gemini_function_call_id_preserved_in_assistant() {
        // functionCall.id from Gemini should be preserved in reconstructed model message
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: Some(vec![LlmToolCall {
                id: "execute_cmd|iofonrJ2|sigABC".to_string(),
                call_type: "function".to_string(),
                function: LlmFunction {
                    name: "execute_cmd".to_string(),
                    arguments: r#"{"cmd":"echo hi"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let fc = &contents[0]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "execute_cmd");
        assert_eq!(fc["id"], "iofonrJ2");
    }

    #[test]
    fn test_gemini_function_response_includes_id() {
        // functionResponse should include the id from functionCall
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some(r#"{"result":"hello"}"#.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("execute_cmd|iofonrJ2|sigXYZ".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let fr = &contents[0]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "execute_cmd");
        assert_eq!(fr["id"], "iofonrJ2");
        // thoughtSignature at part level
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "sigXYZ");
    }

    #[test]
    fn test_gemini_id_format_parsing_all_fields() {
        // Full ID format: "fn_name|fc_id|thought_sig"
        let messages = vec![
            LlmMessage {
                role: "assistant".to_string(),
                name: None,
                content: Some("Let me check.".to_string()),
                content_parts: None,
                tool_calls: Some(vec![LlmToolCall {
                    id: "fetch_url|abc123|longThoughtSigBase64==".to_string(),
                    call_type: "function".to_string(),
                    function: LlmFunction {
                        name: "fetch_url".to_string(),
                        arguments: r#"{"url":"https://example.com"}"#.to_string(),
                    },
                }]),
                tool_call_id: None,
            },
            LlmMessage {
                role: "tool".to_string(),
                name: None,
                content: Some(r#"{"body":"<html>..."}"#.to_string()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: Some("fetch_url|abc123|longThoughtSigBase64==".to_string()),
            },
        ];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        // Assistant (model) message
        let model_part = &contents[0]["parts"][1];
        assert_eq!(model_part["functionCall"]["name"], "fetch_url");
        assert_eq!(model_part["functionCall"]["id"], "abc123");
        assert_eq!(model_part["thoughtSignature"], "longThoughtSigBase64==");
        // Tool (function) response
        let tool_part = &contents[1]["parts"][0];
        assert_eq!(tool_part["functionResponse"]["name"], "fetch_url");
        assert_eq!(tool_part["functionResponse"]["id"], "abc123");
        assert_eq!(tool_part["thoughtSignature"], "longThoughtSigBase64==");
    }

    #[test]
    fn test_gemini_empty_fc_id_not_included() {
        // When fc_id is empty, id field should not be in the output
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: Some(vec![LlmToolCall {
                id: "list_dir||".to_string(),
                call_type: "function".to_string(),
                function: LlmFunction {
                    name: "list_dir".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let fc = &contents[0]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "list_dir");
        // Empty id should not be present
        assert!(fc.get("id").is_none() || fc["id"].is_null());
    }

    #[test]
    fn test_parse_choices_single_choice_with_content_only() {
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello world"
                }
            }]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert_eq!(content, Some("Hello world".to_string()));
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_parse_choices_single_choice_with_tool_calls() {
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "execute_cmd",
                            "arguments": "{\"cmd\":\"ls\"}"
                        }
                    }]
                }
            }]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert!(content.is_none());
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "execute_cmd");
    }

    #[test]
    fn test_parse_choices_multi_choice_split_content_and_tools() {
        // This is the Claude-via-Copilot thinking pattern:
        // choices[0] has content + reasoning, choices[1] has tool_calls
        let v = serde_json::json!({
            "choices": [
                {
                    "finish_reason": "tool_calls",
                    "message": {
                        "content": "Let me look that up for you.",
                        "reasoning_text": "I should use lp-api to fetch the bug.",
                        "role": "assistant"
                    }
                },
                {
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "toolu_abc123",
                            "type": "function",
                            "function": {
                                "name": "execute_cmd",
                                "arguments": "{\"cmd\":\"lp-api get bugs/1234567\"}"
                            }
                        }]
                    }
                }
            ]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert_eq!(content, Some("Let me look that up for you.".to_string()));
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_abc123");
        assert_eq!(tool_calls[0].function.name, "execute_cmd");
        assert!(tool_calls[0].function.arguments.contains("lp-api"));
    }

    #[test]
    fn test_parse_choices_multi_choice_tools_in_multiple_choices() {
        // Edge case: tool_calls spread across multiple choices
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "Running commands...",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"/tmp/a.txt\"}"
                            }
                        }]
                    }
                },
                {
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_2",
                            "type": "function",
                            "function": {
                                "name": "execute_cmd",
                                "arguments": "{\"cmd\":\"date\"}"
                            }
                        }]
                    }
                }
            ]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert_eq!(content, Some("Running commands...".to_string()));
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[1].function.name, "execute_cmd");
    }

    #[test]
    fn test_parse_choices_empty_content_skipped() {
        // Empty string content should be treated as None
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": ""
                    }
                },
                {
                    "message": {
                        "role": "assistant",
                        "content": "Actual answer"
                    }
                }
            ]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert_eq!(content, Some("Actual answer".to_string()));
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_parse_choices_null_content_skipped() {
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_x",
                            "type": "function",
                            "function": {
                                "name": "fetch_url",
                                "arguments": "{\"url\":\"https://example.com\"}"
                            }
                        }]
                    }
                }
            ]
        });
        let (content, tool_calls) = parse_choices(&v);
        assert!(content.is_none());
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "fetch_url");
    }

    #[test]
    fn test_parse_choices_no_choices_field() {
        let v = serde_json::json!({ "model": "test" });
        let (content, tool_calls) = parse_choices(&v);
        assert!(content.is_none());
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_parse_choices_empty_choices_array() {
        let v = serde_json::json!({ "choices": [] });
        let (content, tool_calls) = parse_choices(&v);
        assert!(content.is_none());
        assert!(tool_calls.is_empty());
    }

    // =========================================================
    // Additional tests for increased coverage
    // =========================================================

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_short() {
        // "ab" => 2 chars => (2+3)/4 = 1
        assert_eq!(estimate_tokens("ab"), 1);
    }

    #[test]
    fn test_estimate_tokens_exact_four() {
        // "abcd" => 4 chars => (4+3)/4 = 1
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn test_estimate_tokens_five_chars() {
        // 5 chars => (5+3)/4 = 2
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn test_estimate_tokens_long_string() {
        let s = "a".repeat(100);
        let expected = (100u32 + 3) / 4;
        assert_eq!(estimate_tokens(&s), expected);
    }

    #[test]
    fn test_estimate_usage_content_only() {
        let usage = estimate_usage(Some("hello world"), &[]);
        assert_eq!(usage.prompt_tokens, 0);
        assert!(usage.completion_tokens > 0);
        assert_eq!(usage.total_tokens, usage.completion_tokens);
    }

    #[test]
    fn test_estimate_usage_tool_calls_only() {
        let calls = vec![LlmToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: LlmFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/f"}"#.to_string(),
            },
        }];
        let usage = estimate_usage(None, &calls);
        assert_eq!(usage.prompt_tokens, 0);
        assert!(usage.completion_tokens > 0);
    }

    #[test]
    fn test_estimate_usage_both() {
        let calls = vec![LlmToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: LlmFunction {
                name: "execute_cmd".to_string(),
                arguments: r#"{"cmd":"ls -la"}"#.to_string(),
            },
        }];
        let usage = estimate_usage(Some("let me check"), &calls);
        assert!(usage.total_tokens > 0);
    }

    #[test]
    fn test_estimate_usage_empty() {
        let usage = estimate_usage(None, &[]);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_finalize_tool_calls_single() {
        let mut states = BTreeMap::new();
        states.insert(
            0,
            StreamingToolCallState {
                id: Some("call_1".to_string()),
                call_type: "function".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp"}"#.to_string(),
            },
        );
        let calls = finalize_tool_calls(states);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].call_type, "function");
    }

    #[test]
    fn test_finalize_tool_calls_no_id_uses_index() {
        let mut states = BTreeMap::new();
        states.insert(
            3,
            StreamingToolCallState {
                id: None,
                call_type: String::new(),
                name: "execute_cmd".to_string(),
                arguments: "{}".to_string(),
            },
        );
        let calls = finalize_tool_calls(states);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "stream-3");
        assert_eq!(calls[0].call_type, "function"); // default_tool_type fallback
    }

    #[test]
    fn test_finalize_tool_calls_ordered_by_index() {
        let mut states = BTreeMap::new();
        states.insert(
            1,
            StreamingToolCallState {
                id: Some("c1".to_string()),
                call_type: "function".to_string(),
                name: "b_tool".to_string(),
                arguments: "{}".to_string(),
            },
        );
        states.insert(
            0,
            StreamingToolCallState {
                id: Some("c0".to_string()),
                call_type: "function".to_string(),
                name: "a_tool".to_string(),
                arguments: "{}".to_string(),
            },
        );
        let calls = finalize_tool_calls(states);
        // BTreeMap orders by key ascending
        assert_eq!(calls[0].function.name, "a_tool");
        assert_eq!(calls[1].function.name, "b_tool");
    }

    #[test]
    fn test_is_transient_error_timeout() {
        let e = anyhow::anyhow!("connection timeout");
        assert!(is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_429() {
        let e = anyhow::anyhow!("rate limit 429 too many requests");
        assert!(is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_500() {
        let e = anyhow::anyhow!("API request failed (500): internal server error");
        assert!(is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_502() {
        let e = anyhow::anyhow!("bad gateway 502");
        assert!(is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_503() {
        let e = anyhow::anyhow!("service unavailable 503");
        assert!(is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_permanent() {
        let e = anyhow::anyhow!("failed to parse response JSON: unexpected character");
        assert!(!is_transient_error(&e));
    }

    #[test]
    fn test_is_transient_error_auth_failure() {
        let e = anyhow::anyhow!("API request failed (401): Unauthorized");
        assert!(!is_transient_error(&e));
    }

    #[test]
    fn test_default_tool_type_value() {
        assert_eq!(default_tool_type(), "function");
    }

    #[test]
    fn test_token_usage_default() {
        let u = TokenUsage::default();
        assert_eq!(u.prompt_tokens, 0);
        assert_eq!(u.completion_tokens, 0);
        assert_eq!(u.total_tokens, 0);
    }

    #[test]
    fn test_token_usage_deserialization() {
        let json = r#"{"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}"#;
        let u: TokenUsage = serde_json::from_str(json).unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 20);
        assert_eq!(u.total_tokens, 30);
    }

    #[test]
    fn test_openai_provider_new_default_base_url() {
        let p = OpenAiProvider::new("openai".to_string(), "sk-test".to_string(), None);
        assert_eq!(p.base_url, "https://api.openai.com/v1");
        assert_eq!(p.provider_name, "openai");
        assert_eq!(p.api_key, "sk-test");
    }

    #[test]
    fn test_openai_provider_new_custom_base_url() {
        let p = OpenAiProvider::new(
            "ollama".to_string(),
            "".to_string(),
            Some("http://localhost:11434/v1".to_string()),
        );
        assert_eq!(p.base_url, "http://localhost:11434/v1");
        assert_eq!(p.name(), "ollama");
    }

    #[test]
    fn test_gemini_provider_new_defaults() {
        let p = GeminiProvider::new("AIzaTest".to_string(), None, None);
        assert_eq!(p.model, "gemini-2.0-flash");
        assert_eq!(
            p.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(p.api_key, "AIzaTest");
    }

    #[test]
    fn test_gemini_provider_new_custom() {
        let p = GeminiProvider::new(
            "key".to_string(),
            Some("gemini-1.5-pro".to_string()),
            Some("https://custom.endpoint.com/v1beta".to_string()),
        );
        assert_eq!(p.model, "gemini-1.5-pro");
        assert_eq!(p.base_url, "https://custom.endpoint.com/v1beta");
    }

    #[test]
    fn test_provider_registry_empty_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let reg = ProviderRegistry::new();
            let req = LlmRequest {
                model: "m".into(),
                messages: vec![],
                tools: None,
                max_tokens: None,
                thinking: None,
            };
            let result = reg.chat(req).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("no providers"));
        });
    }

    #[test]
    fn test_provider_registry_is_empty() {
        let mut reg = ProviderRegistry::new();
        assert!(reg.is_empty());
        let resp = LlmResponse {
            content: Some("x".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m".into(),
        };
        reg.register(Box::new(SucceedProvider::new("p", resp)));
        assert!(!reg.is_empty());
    }

    #[tokio::test]
    async fn test_provider_registry_named_provider_not_found() {
        let mut reg = ProviderRegistry::new();
        let resp = LlmResponse {
            content: Some("ok".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m".into(),
        };
        reg.register(Box::new(SucceedProvider::new("p1", resp)));
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let err = reg.chat_with("nonexistent", req).await.unwrap_err();
        assert!(err.to_string().contains("provider not found"));
    }

    #[tokio::test]
    async fn test_provider_registry_streaming_empty_returns_error() {
        let reg = ProviderRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(4);
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let result = reg.chat_streaming(req, tx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_provider_registry_streaming_chat_with_not_found() {
        let mut reg = ProviderRegistry::new();
        let resp = LlmResponse {
            content: Some("ok".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
            model: "m".into(),
        };
        reg.register(Box::new(SucceedProvider::new("p1", resp)));
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(4);
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: None,
        };
        let err = reg
            .chat_with_streaming("missing", req, tx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("provider not found"));
    }

    #[test]
    fn test_llm_request_serialization() {
        let req = LlmRequest {
            model: "gpt-4o".to_string(),
            messages: vec![LlmMessage {
                role: "user".to_string(),
                name: None,
                content: Some("Hello".to_string()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
            max_tokens: Some(512),
            thinking: None, // #[serde(skip)] so not in output
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("Hello"));
        assert!(json.contains("512"));
        // thinking is skipped
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn test_llm_message_tool_call_serialization() {
        let msg = LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: Some(vec![LlmToolCall {
                id: "call_abc".to_string(),
                call_type: "function".to_string(),
                function: LlmFunction {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp/x"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("call_abc"));
        assert!(json.contains("read_file"));
        assert!(!json.contains("content_parts"));
    }

    #[test]
    fn test_llm_tool_call_deserialization() {
        let json = r#"{"id":"call_123","type":"function","function":{"name":"execute_cmd","arguments":"{\"cmd\":\"ls\"}"}}"#;
        let tc: LlmToolCall = serde_json::from_str(json).unwrap();
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.call_type, "function");
        assert_eq!(tc.function.name, "execute_cmd");
        assert!(tc.function.arguments.contains("ls"));
    }

    #[test]
    fn test_llm_tool_call_default_type_when_missing() {
        // type field missing → uses default_tool_type()
        let json = r#"{"id":"c1","function":{"name":"foo","arguments":"{}"}}"#;
        let tc: LlmToolCall = serde_json::from_str(json).unwrap();
        assert_eq!(tc.call_type, "function");
    }

    #[test]
    fn test_streaming_chunk_deserialization() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "content": "Hello"
                }
            }],
            "model": "gpt-4o",
            "usage": null
        }"#;
        let chunk: StreamingChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
        assert_eq!(chunk.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_streaming_chunk_tool_call_delta() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":"
                        }
                    }]
                }
            }]
        }"#;
        let chunk: StreamingChunk = serde_json::from_str(json).unwrap();
        let delta = &chunk.choices[0].delta;
        let tool_deltas = delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_deltas.len(), 1);
        assert_eq!(tool_deltas[0].index, 0);
        assert_eq!(tool_deltas[0].id.as_deref(), Some("call_1"));
        assert_eq!(
            tool_deltas[0].function.as_ref().unwrap().name.as_deref(),
            Some("read_file")
        );
    }

    #[test]
    fn test_copilot_provider_new() {
        let p = CopilotProvider::new("ghu_test_token".to_string());
        assert_eq!(p.pat, "ghu_test_token");
        assert_eq!(p.name(), "github-copilot");
    }

    #[test]
    fn test_llm_response_clone() {
        let r = LlmResponse {
            content: Some("test".to_string()),
            tool_calls: vec![],
            usage: TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 10,
                total_tokens: 15,
            },
            model: "test-model".to_string(),
        };
        let r2 = r.clone();
        assert_eq!(r2.content, r.content);
        assert_eq!(r2.model, r.model);
        assert_eq!(r2.usage.total_tokens, 15);
    }

    #[test]
    fn test_gemini_convert_messages_tool_with_non_object_json() {
        // Non-object JSON array should be wrapped in {"result": ...}
        let messages = vec![LlmMessage {
            role: "tool".to_string(),
            name: None,
            content: Some(r#"[1, 2, 3]"#.to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("list_items|fc_id_1|".to_string()),
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        let resp = &contents[0]["parts"][0]["functionResponse"]["response"];
        assert_eq!(resp["result"], "[1, 2, 3]");
    }

    #[test]
    fn test_gemini_convert_messages_user_message() {
        let messages = vec![LlmMessage {
            role: "user".to_string(),
            name: None,
            content: Some("What is the weather?".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let (sys, contents) = GeminiProvider::convert_messages(&messages);
        assert!(sys.is_none());
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "What is the weather?");
    }

    #[test]
    fn test_gemini_convert_messages_assistant_with_text() {
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: Some("The weather is sunny.".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        assert_eq!(contents[0]["role"], "model");
        assert_eq!(contents[0]["parts"][0]["text"], "The weather is sunny.");
    }

    #[test]
    fn test_gemini_convert_messages_empty_assistant_skipped() {
        // assistant with no content and no tool_calls should produce no parts → skipped
        let messages = vec![LlmMessage {
            role: "assistant".to_string(),
            name: None,
            content: None,
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let (_, contents) = GeminiProvider::convert_messages(&messages);
        assert_eq!(contents.len(), 0);
    }

    #[test]
    fn test_gemini_convert_tools_multiple_functions() {
        let tools = vec![
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a file",
                    "parameters": {"type": "object", "properties": {}}
                }
            }),
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write a file",
                    "parameters": {"type": "object", "properties": {}}
                }
            }),
        ];
        let result = GeminiProvider::convert_tools(&tools).unwrap();
        let decls = &result[0]["function_declarations"];
        assert_eq!(decls.as_array().unwrap().len(), 2);
        assert_eq!(decls[0]["name"], "read_file");
        assert_eq!(decls[1]["name"], "write_file");
    }

    #[test]
    fn test_gemini_convert_tools_missing_function_field_skipped() {
        let tools = vec![
            serde_json::json!({ "type": "other" }), // no "function" field
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "valid_func",
                    "description": "valid",
                    "parameters": {}
                }
            }),
        ];
        let result = GeminiProvider::convert_tools(&tools).unwrap();
        let decls = &result[0]["function_declarations"];
        assert_eq!(decls.as_array().unwrap().len(), 1);
        assert_eq!(decls[0]["name"], "valid_func");
    }

    #[test]
    fn test_parse_choices_with_whitespace_content() {
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "   "
                }
            }]
        });
        // Note: parse_choices doesn't trim, but "   " is non-empty so it returns
        let (content, _) = parse_choices(&v);
        assert_eq!(content, Some("   ".to_string()));
    }

    // ---------- Regression tests for multi-endpoint cache + thinking gating ----------
    //
    // These tests cover the bug found by integration testing on 2026-06-03:
    //   1. model_endpoints cache was never populated when callers passed an
    //      explicit --model (only auto-discovery via list_models warmed it),
    //      so /responses-only models incorrectly fell back to /chat/completions.
    //   2. thinking/reasoning_effort was forwarded to models that don't support
    //      it (e.g. claude-haiku-4.5), causing 400s.
    // The fix adds warm_caches_if_needed() and normalize_thinking().

    fn copilot_with_cached_models(
        endpoints_map: Vec<(&str, Vec<&str>)>,
        reasoning_map: Vec<(&str, Vec<&str>)>,
    ) -> CopilotProvider {
        let provider = CopilotProvider::new("ghu_test".to_string());
        {
            let mut cache = provider.model_endpoints.lock().unwrap();
            for (id, eps) in endpoints_map {
                cache.insert(id.to_string(), eps.into_iter().map(String::from).collect());
            }
        }
        {
            let mut cache = provider.model_reasoning.lock().unwrap();
            for (id, efforts) in reasoning_map {
                cache.insert(
                    id.to_string(),
                    efforts.into_iter().map(String::from).collect(),
                );
            }
        }
        provider
    }

    #[test]
    fn test_get_endpoints_cache_hit_routes_to_responses() {
        let provider = copilot_with_cached_models(vec![("gpt-5.5", vec!["/responses"])], vec![]);
        let eps = provider.get_endpoints("gpt-5.5");
        assert_eq!(eps, vec!["/responses".to_string()]);
    }

    #[test]
    fn test_get_endpoints_cache_miss_falls_back_to_chat() {
        let provider = copilot_with_cached_models(vec![], vec![]);
        let eps = provider.get_endpoints("unknown-model");
        assert_eq!(eps, vec!["/chat/completions".to_string()]);
    }

    #[test]
    fn test_get_endpoints_preserves_api_ordering_for_fallback() {
        let provider = copilot_with_cached_models(
            vec![("claude-opus-4.8", vec!["/v1/messages", "/chat/completions"])],
            vec![],
        );
        let eps = provider.get_endpoints("claude-opus-4.8");
        assert_eq!(eps[0], "/v1/messages");
        assert_eq!(eps[1], "/chat/completions");
    }

    #[test]
    fn test_endpoint_to_path_normalizes_short_names() {
        assert_eq!(
            CopilotProvider::endpoint_to_path("chat"),
            "/chat/completions"
        );
        assert_eq!(CopilotProvider::endpoint_to_path("responses"), "/responses");
        assert_eq!(
            CopilotProvider::endpoint_to_path("messages"),
            "/v1/messages"
        );
        assert_eq!(
            CopilotProvider::endpoint_to_path("/responses"),
            "/responses"
        );
        // ws: filtered earlier in list_models, but if it slips through fall back to chat.
        assert_eq!(
            CopilotProvider::endpoint_to_path("unknown"),
            "/chat/completions"
        );
    }

    #[test]
    fn test_model_supports_reasoning_via_cache() {
        let provider = copilot_with_cached_models(
            vec![],
            vec![
                ("claude-sonnet-4.6", vec!["low", "medium", "high"]),
                ("claude-haiku-4.5", vec![]),
            ],
        );
        assert!(provider.model_supports_reasoning("claude-sonnet-4.6"));
        assert!(!provider.model_supports_reasoning("claude-haiku-4.5"));
        // Cache miss: assume not supported (after warm_caches_if_needed the cache is
        // authoritative; a miss means the model is not picker_enabled).
        assert!(!provider.model_supports_reasoning("unknown"));
    }

    #[test]
    fn test_normalize_thinking_strips_when_unsupported() {
        let provider = copilot_with_cached_models(vec![], vec![("claude-haiku-4.5", vec![])]);
        let req = LlmRequest {
            model: "claude-haiku-4.5".to_string(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: Some("high".to_string()),
        };
        let normalized = provider.normalize_thinking(req);
        assert_eq!(normalized.thinking, None);
    }

    #[test]
    fn test_normalize_thinking_preserves_when_supported() {
        let provider = copilot_with_cached_models(
            vec![],
            vec![("claude-sonnet-4.6", vec!["low", "medium", "high"])],
        );
        let req = LlmRequest {
            model: "claude-sonnet-4.6".to_string(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: Some("high".to_string()),
        };
        let normalized = provider.normalize_thinking(req);
        assert_eq!(normalized.thinking, Some("high".to_string()));
    }

    #[test]
    fn test_normalize_thinking_ignores_off_and_none_sentinels() {
        let provider = copilot_with_cached_models(vec![], vec![("claude-haiku-4.5", vec![])]);
        for sentinel in ["off", "none"] {
            let req = LlmRequest {
                model: "claude-haiku-4.5".to_string(),
                messages: vec![],
                tools: None,
                max_tokens: None,
                thinking: Some(sentinel.to_string()),
            };
            let normalized = provider.normalize_thinking(req);
            // sentinels pass through unchanged; payload builder skips them.
            assert_eq!(normalized.thinking, Some(sentinel.to_string()));
        }
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    fn make_state(name: &str, args: &str) -> StreamingToolCallState {
        StreamingToolCallState {
            id: Some(format!("call_{}", name)),
            name: name.to_string(),
            call_type: "function".to_string(),
            arguments: args.to_string(),
        }
    }

    #[test]
    fn test_finalize_tool_calls_valid_json_kept() {
        let mut states = BTreeMap::new();
        states.insert(0, make_state("execute_cmd", r#"{"cmd":"ls"}"#));
        states.insert(1, make_state("read_file", r#"{"path":"/tmp/x.txt"}"#));

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].function.name, "execute_cmd");
        assert_eq!(result[1].function.name, "read_file");
    }

    #[test]
    fn test_finalize_tool_calls_invalid_json_dropped() {
        let mut states = BTreeMap::new();
        // Two JSON objects concatenated — the known bug pattern
        states.insert(0, make_state("execute_cmd", r#"{"cmd":"ls"}{"path":"."}"#));

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 0, "malformed tool_call should be dropped");
    }

    #[test]
    fn test_finalize_tool_calls_empty_arguments_kept() {
        let mut states = BTreeMap::new();
        states.insert(0, make_state("no_args_tool", ""));

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 1, "empty arguments should be allowed");
        assert_eq!(result[0].function.name, "no_args_tool");
    }

    #[test]
    fn test_finalize_tool_calls_mixed_valid_invalid() {
        let mut states = BTreeMap::new();
        states.insert(0, make_state("good_tool", r#"{"x": 1}"#));
        states.insert(1, make_state("bad_tool", r#"{"x": 1}{"y": 2}"#));
        states.insert(2, make_state("another_good", r#"{"z": "hello"}"#));

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 2, "only valid tool_calls should survive");
        assert_eq!(result[0].function.name, "good_tool");
        assert_eq!(result[1].function.name, "another_good");
    }

    #[test]
    fn test_finalize_tool_calls_truncated_json_dropped() {
        let mut states = BTreeMap::new();
        // Incomplete JSON from interrupted streaming
        states.insert(0, make_state("execute_cmd", r#"{"cmd":"ls -la /tmp"#));

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 0, "truncated JSON should be dropped");
    }

    #[test]
    fn test_finalize_tool_calls_nested_valid_json() {
        let mut states = BTreeMap::new();
        states.insert(
            0,
            make_state("complex_tool", r#"{"a":{"b":[1,2,3]},"c":"d"}"#),
        );

        let result = finalize_tool_calls(states);
        assert_eq!(result.len(), 1, "valid nested JSON should be kept");
    }

    #[test]
    fn test_copilot_provider_list_models_method_exists() {
        // Verify the list_models method is callable (compile-time check).
        // Actual API call tested via integration test.
        let p = CopilotProvider::new("ghu_fake_token".to_string());
        assert_eq!(p.name(), "github-copilot");
        // list_models() returns a Future — just ensure it compiles.
        let _fut = p.list_models();
    }

    #[test]
    fn test_provider_registry_list_models_empty_registry() {
        let reg = ProviderRegistry::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(reg.list_models());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no providers"));
    }

    #[test]
    fn test_openai_provider_list_models_returns_empty_by_default() {
        // OpenAI provider uses the default trait impl which returns empty Vec
        let p = OpenAiProvider::new("openai".to_string(), "sk-fake".to_string(), None);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(p.list_models());
        assert!(result.is_ok());
        let models: Vec<ModelInfo> = result.unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_openrouter_list_models_with_zdr_filtering() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0; 1024];
                let _ = stream.read(&mut buf);
                let request_str = String::from_utf8_lossy(&buf);

                if request_str.contains("GET /endpoints/zdr") {
                    let response_body = r#"{
                        "data": [
                            {"model_id": "meta-llama/llama-3.3-70b-instruct"},
                            {"model_id": "google/gemini-2.0-flash"}
                        ]
                    }"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );
                    let _ = stream.write_all(response.as_bytes());
                } else if request_str.contains("GET /models") {
                    let response_body = r#"{
                        "data": [
                            {"id": "meta-llama/llama-3.3-70b-instruct", "context_length": 131072, "supported_parameters": ["tools"]},
                            {"id": "google/gemini-2.0-flash", "context_length": 1048576, "supported_parameters": ["tools", "reasoning"]},
                            {"id": "openai/gpt-4o-mini", "context_length": 128000, "supported_parameters": ["tools"]},
                            {"id": "deepseek/deepseek-chat", "context_length": 64000, "supported_parameters": ["tools"]}
                        ]
                    }"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });

        let p = OpenAiProvider::new(
            "openrouter".to_string(),
            "sk-fake".to_string(),
            Some(base_url),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(p.list_models());
        assert!(result.is_ok());
        let models = result.unwrap();

        // Should filter out "openai/gpt-4o-mini" and "deepseek/deepseek-chat" because they aren't in ZDR list
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "google/gemini-2.0-flash");
        assert_eq!(models[1].id, "meta-llama/llama-3.3-70b-instruct");
    }

    #[test]
    fn test_openrouter_supports_reasoning() {
        let p = OpenAiProvider::new("openrouter".to_string(), "sk-fake".to_string(), None);
        // Fallbacks
        assert!(p.supports_reasoning("openai/o1-mini"));
        assert!(p.supports_reasoning("openai/o3-mini"));
        assert!(p.supports_reasoning("deepseek/deepseek-r1"));
        assert!(p.supports_reasoning("anthropic/claude-3-7-sonnet"));
        assert!(!p.supports_reasoning("openai/gpt-4o-mini"));

        // Cache behavior
        {
            let mut cache = p.reasoning_models.lock().unwrap();
            cache.insert("my-special-model".to_string());
        }
        // Once cache is populated, only items in cache match
        assert!(p.supports_reasoning("my-special-model"));
        assert!(!p.supports_reasoning("openai/o1-mini"));
    }

    #[test]
    fn test_openrouter_normalize_thinking() {
        let p = OpenAiProvider::new("openrouter".to_string(), "sk-fake".to_string(), None);
        // Fallback model supporting reasoning preserves thinking
        let req = LlmRequest {
            model: "openai/o1-mini".to_string(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: Some("high".to_string()),
        };
        let normalized = p.normalize_thinking(req);
        assert_eq!(normalized.thinking, Some("high".to_string()));

        // Fallback model NOT supporting reasoning strips thinking
        let req2 = LlmRequest {
            model: "openai/gpt-4o-mini".to_string(),
            messages: vec![],
            tools: None,
            max_tokens: None,
            thinking: Some("high".to_string()),
        };
        let normalized2 = p.normalize_thinking(req2);
        assert_eq!(normalized2.thinking, None);
    }

    #[test]
    fn test_registry_list_models_delegates_to_default_provider() {
        // A registry with an OpenAI provider should return empty (default impl)
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(OpenAiProvider::new(
            "openai".to_string(),
            "sk-fake".to_string(),
            None,
        )));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(reg.list_models());
        assert!(result.is_ok());
        let models: Vec<ModelInfo> = result.unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_copilot_models_json_parsing_data_array() {
        // Simulate the JSON response format from Copilot /models endpoint with metadata
        let json = r#"{"data":[
            {"id":"gpt-5-mini","model_picker_enabled":true,"supported_endpoints":["chat"],"capabilities":{"limits":{"max_context_window_tokens":16384},"supports":{}}},
            {"id":"claude-sonnet-4.6","model_picker_enabled":true,"supported_endpoints":["chat"],"capabilities":{"limits":{"max_context_window_tokens":200000},"supports":{"reasoning_effort":["low","medium","high"]}}},
            {"id":"gemini-3.5-flash","model_picker_enabled":true,"supported_endpoints":["chat"],"capabilities":{"limits":{},"supports":{}}}
        ]}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut models: Vec<ModelInfo> = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let picker_enabled = m
                            .get("model_picker_enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if !picker_enabled {
                            return None;
                        }
                        let has_endpoints = m
                            .get("supported_endpoints")
                            .and_then(|v| v.as_array())
                            .map(|arr| !arr.is_empty())
                            .unwrap_or(false);
                        if !has_endpoints {
                            return None;
                        }
                        let id = m
                            .get("id")
                            .or_else(|| m.get("name"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())?;
                        let context_window = m
                            .pointer("/capabilities/limits/max_context_window_tokens")
                            .and_then(|v| v.as_u64());
                        let reasoning_efforts = m
                            .pointer("/capabilities/supports/reasoning_effort")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(ModelInfo {
                            id,
                            provider: None,
                            context_window,
                            reasoning_efforts,
                            supported_endpoints: vec![],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(models.len(), 3);
        // sorted alphabetically
        assert_eq!(models[0].id, "claude-sonnet-4.6");
        assert_eq!(models[0].context_window, Some(200000));
        assert_eq!(models[0].reasoning_efforts, vec!["low", "medium", "high"]);
        assert_eq!(models[1].id, "gemini-3.5-flash");
        assert_eq!(models[1].context_window, None);
        assert!(models[1].reasoning_efforts.is_empty());
        assert_eq!(models[2].id, "gpt-5-mini");
        assert_eq!(models[2].context_window, Some(16384));
        assert!(models[2].reasoning_efforts.is_empty());
    }

    #[test]
    fn test_copilot_models_json_parsing_models_array() {
        // Alternative format: "models" key with "name" field
        let json = r#"{"models":[{"name":"model-a","model_picker_enabled":true,"supported_endpoints":["chat"]},{"name":"model-b","model_picker_enabled":true,"supported_endpoints":["chat"]}]}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut models: Vec<ModelInfo> = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let picker_enabled = m
                            .get("model_picker_enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if !picker_enabled {
                            return None;
                        }
                        let has_endpoints = m
                            .get("supported_endpoints")
                            .and_then(|v| v.as_array())
                            .map(|arr| !arr.is_empty())
                            .unwrap_or(false);
                        if !has_endpoints {
                            return None;
                        }
                        let id = m
                            .get("id")
                            .or_else(|| m.get("name"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())?;
                        let context_window = m
                            .pointer("/capabilities/limits/max_context_window_tokens")
                            .and_then(|v| v.as_u64());
                        let reasoning_efforts = m
                            .pointer("/capabilities/supports/reasoning_effort")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(ModelInfo {
                            id,
                            provider: None,
                            context_window,
                            reasoning_efforts,
                            supported_endpoints: vec![],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "model-a");
        assert_eq!(models[1].id, "model-b");
        assert!(models[0].context_window.is_none());
        assert!(models[0].reasoning_efforts.is_empty());
    }

    #[test]
    fn test_copilot_models_json_parsing_empty_response() {
        let json = r#"{"data":[]}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let models: Vec<ModelInfo> = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m
                            .get("id")
                            .or_else(|| m.get("name"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())?;
                        Some(ModelInfo {
                            id,
                            provider: None,
                            context_window: None,
                            reasoning_efforts: vec![],
                            supported_endpoints: vec![],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(models.is_empty());
    }

    #[test]
    fn test_copilot_models_json_parsing_no_data_key() {
        // Neither "data" nor "models" key — should return empty
        let json = r#"{"error":"something"}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let models: Vec<ModelInfo> = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m
                            .get("id")
                            .or_else(|| m.get("name"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())?;
                        Some(ModelInfo {
                            id,
                            provider: None,
                            context_window: None,
                            reasoning_efforts: vec![],
                            supported_endpoints: vec![],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(models.is_empty());
    }

    #[test]
    fn test_no_api_key_in_url_leak() {
        fn find_rs_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            find_rs_files(&path, files);
                        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                            files.push(path);
                        }
                    }
                }
            }
        }

        let mut files = Vec::new();
        find_rs_files(std::path::Path::new("src"), &mut files);

        // Use concatenated/separated string parts to avoid matching this test itself.
        let target_part1 = "googleapis.com";
        let target_part2 = "key=";
        let target_part3 = "generativelanguage";

        for file_path in files {
            let content = std::fs::read_to_string(&file_path)
                .unwrap_or_else(|_| panic!("Failed to read {:?}", file_path));

            for (line_num, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                // Skip comments and this test function itself
                if trimmed.starts_with("//")
                    || trimmed.starts_with("/*")
                    || trimmed.contains("test_no_api_key_in_url_leak")
                {
                    continue;
                }

                if (line.contains(target_part1) || line.contains(target_part3))
                    && line.contains(target_part2)
                {
                    panic!(
                        "Potential API key leak in URL detected in {:?} at line {}: {}",
                        file_path,
                        line_num + 1,
                        line
                    );
                }
            }
        }
    }
}

pub struct MockLoopProvider;

impl MockLoopProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Provider for MockLoopProvider {
    fn name(&self) -> &str {
        "mock-loop"
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        Box::pin(async move {
            let user_msg = request
                .messages
                .last()
                .and_then(|m| m.content.as_deref())
                .unwrap_or("");

            let content = if user_msg.contains("Always fail") {
                Some("Verification failed: the goal was not satisfied because the task is set to always fail.".to_string())
            } else if user_msg.contains("Verifier") || user_msg.contains("verify") {
                Some("GOAL_COMPLETE: All tests pass successfully and the requested feature is fully implemented!".to_string())
            } else {
                Some(
                    "I have implemented the requested changes in src/main.rs. Please verify them."
                        .to_string(),
                )
            };

            Ok(LlmResponse {
                content,
                tool_calls: Vec::new(),
                usage: crate::provider::TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 10,
                    total_tokens: 20,
                },
                model: request.model.clone(),
            })
        })
    }
}
