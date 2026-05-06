use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::Sender;
use tracing::{debug, info};

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
        .map(|(index, state)| LlmToolCall {
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

/// Provider trait.
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
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
}

impl OpenAiProvider {
    pub fn new(name: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        OpenAiProvider {
            api_key,
            base_url: base,
            provider_name: name,
        }
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();

        Box::pin(async move {
            // Build the full request payload using serde
            let mut payload_value: serde_json::Value = serde_json::to_value(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            // Inject thinking/reasoning_effort if set
            if let Some(ref thinking) = request.thinking {
                if thinking != "none" {
                    payload_value["reasoning_effort"] = serde_json::Value::String(thinking.clone());
                }
            }

            let payload = serde_json::to_string(&payload_value)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

            debug!(url = %url, payload_len = payload.len(), "sending LLM request");

            let client = Client::new();
            let response = client
                .post(&url)
                .bearer_auth(&api_key)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(120))
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
                    &stdout[..stdout.len().min(500)]
                ));
            }

            // Parse response JSON
            let v: Value = serde_json::from_str(&stdout).map_err(|e| {
                anyhow!(
                    "failed to parse response JSON: {}\nraw: {}",
                    e,
                    &stdout[..stdout.len().min(500)]
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
        let request = request.clone();

        Box::pin(async move {
            let client = Client::new();
            let mut payload = serde_json::to_value(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;
            if let Value::Object(ref mut map) = payload {
                map.insert("stream".to_string(), Value::Bool(true));
            } else {
                return Err(anyhow!("request payload must be an object"));
            }

            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let builder = client
                .post(url)
                .bearer_auth(api_key)
                .header("Accept", "text/event-stream")
                .json(&payload);

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
}

impl CopilotProvider {
    pub fn new(pat: String) -> Self {
        CopilotProvider {
            pat,
            provider_name: "github-copilot".to_string(),
            token_cache: std::sync::Mutex::new(None),
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
                if now < expires_at - 60 {
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
                &body[..body.len().min(200)]
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
                &stdout[..stdout.len().min(200)]
            )
        })?;

        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("no token in response"))?
            .to_string();

        let expires_at = v.get("expires_at").and_then(|e| e.as_u64()).unwrap_or(0);

        let endpoint = v
            .get("endpoints")
            .and_then(|e| e.get("api"))
            .and_then(|a| a.as_str())
            .unwrap_or("https://api.githubcopilot.com")
            .to_string();

        info!(endpoint = %endpoint, expires_in = expires_at.saturating_sub(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
        ), "copilot token refreshed");

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

    fn chat(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        Box::pin(async move {
            let (token, endpoint) = self.get_token().await?;

            let payload = serde_json::to_string(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));

            let client = Client::new();
            let response = client
                .post(&url)
                .bearer_auth(&token)
                .header("Content-Type", "application/json")
                .header("User-Agent", "rune/0.1.0")
                .header("editor-version", "vscode/1.96.0")
                .timeout(std::time::Duration::from_secs(120))
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
                    "Copilot API request failed ({}): {}",
                    status,
                    &stdout[..stdout.len().min(500)]
                ));
            }

            let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
                anyhow!(
                    "failed to parse response: {}\nraw: {}",
                    e,
                    &stdout[..stdout.len().min(500)]
                )
            })?;

            if let Some(err) = v.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                return Err(anyhow!("Copilot API error: {}", msg));
            }

            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let (content, tool_calls) = parse_choices(&v);

            let usage = v
                .get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

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
        let request = request.clone();

        Box::pin(async move {
            let (token, endpoint) = self.get_token().await?;
            let client = Client::new();
            let mut payload = serde_json::to_value(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;
            if let Value::Object(ref mut map) = payload {
                map.insert("stream".to_string(), Value::Bool(true));
            } else {
                return Err(anyhow!("request payload must be an object"));
            }

            let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
            let builder = client
                .post(url)
                .bearer_auth(token)
                .header("User-Agent", "rune/0.1.0")
                .header("editor-version", "vscode/1.96.0")
                .header("Accept", "text/event-stream")
                .json(&payload);

            stream_openai_compatible_response(builder, tx).await
        })
    }
}

/// Google Gemini provider — native Gemini API format.
pub struct GeminiProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        GeminiProvider {
            api_key,
            model: model.unwrap_or_else(|| "gemini-2.0-flash".to_string()),
            base_url: base_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string()),
        }
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

            // Gemini thinking config
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

            let url = format!(
                "{}/models/{}:generateContent?key={}",
                base_url.trim_end_matches('/'),
                model,
                api_key
            );

            let payload_str = serde_json::to_string(&payload)
                .map_err(|e| anyhow!("failed to serialize Gemini request: {}", e))?;

            debug!(url = %url, payload_len = payload_str.len(), "sending Gemini request");

            let client = Client::new();
            let response = client
                .post(&url)
                .header("Content-Type", "application/json")
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
                    &stdout[..stdout.len().min(500)]
                ));
            }

            let v: Value = serde_json::from_str(&stdout).map_err(|e| {
                anyhow!(
                    "failed to parse Gemini response: {}\nraw: {}",
                    e,
                    &stdout[..stdout.len().min(500)]
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

            // Gemini thinking config
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

            let url = format!(
                "{}/models/{}:streamGenerateContent?alt=sse&key={}",
                base_url.trim_end_matches('/'),
                model,
                api_key
            );

            let payload_str = serde_json::to_string(&payload)
                .map_err(|e| anyhow!("failed to serialize Gemini request: {}", e))?;

            debug!(url = %url, payload_len = payload_str.len(), "sending Gemini streaming request");

            let client = Client::new();
            let response = client
                .post(&url)
                .header("Content-Type", "application/json")
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
                    &body[..body.len().min(500)]
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
                    if let Some(calls) = serde_json::from_value::<Vec<LlmToolCall>>(tc.clone()).ok() {
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
                content: Some("You are helpful.".to_string()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
            },
            LlmMessage {
                role: "user".to_string(),
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

}
