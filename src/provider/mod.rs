use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use tokio::process::Command;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
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
            let payload = serde_json::to_string(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

            debug!(url = %url, payload_len = payload.len(), "sending LLM request");

            // Use a temp file to pass the payload to curl (avoids shell escaping issues)
            let tmp_path = format!("/tmp/rune_llm_req_{}.json", std::process::id());
            tokio::fs::write(&tmp_path, &payload)
                .await
                .map_err(|e| anyhow!("failed to write temp payload: {}", e))?;

            let output = Command::new("curl")
                .args([
                    "-s",
                    "-S",
                    "-X",
                    "POST",
                    "-H",
                    "Content-Type: application/json",
                    "-H",
                    &format!("Authorization: Bearer {}", api_key),
                    "-d",
                    &format!("@{}", tmp_path),
                    "--max-time",
                    "120",
                    &url,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| anyhow!("failed to spawn curl: {}", e))?;

            // Clean up temp file (best effort)
            let _ = tokio::fs::remove_file(&tmp_path).await;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !output.status.success() {
                return Err(anyhow!(
                    "curl failed (exit {:?}): {}",
                    output.status.code(),
                    stderr
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

            // Extract from choices[0].message
            let message = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|first| first.get("message"));

            let content = message.and_then(|m| m.get("content")).and_then(|c| {
                if c.is_null() {
                    None
                } else {
                    c.as_str().map(|s| s.to_string())
                }
            });

            // Parse tool_calls from response
            let tool_calls: Vec<LlmToolCall> = message
                .and_then(|m| m.get("tool_calls"))
                .and_then(|tc| {
                    if tc.is_array() {
                        serde_json::from_value::<Vec<LlmToolCall>>(tc.clone()).ok()
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            // Parse usage
            let usage = v
                .get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

            debug!(model = %model, content_len = content.as_ref().map(|c| c.len()).unwrap_or(0),
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
        let output = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "10",
                "-H",
                &format!("Authorization: token {}", self.pat),
                "-H",
                "editor-version: vscode/1.96.0",
                "https://api.github.com/copilot_internal/v2/token",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| anyhow!("failed to refresh copilot token: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            return Err(anyhow!("copilot token refresh failed: {}", stdout));
        }

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
            let tmp_path = format!("/tmp/rune_copilot_req_{}.json", std::process::id());
            tokio::fs::write(&tmp_path, &payload)
                .await
                .map_err(|e| anyhow!("failed to write temp payload: {}", e))?;

            let output = Command::new("curl")
                .args([
                    "-sS",
                    "-X",
                    "POST",
                    "-H",
                    "Content-Type: application/json",
                    "-H",
                    &format!("Authorization: Bearer {}", token),
                    "-H",
                    "editor-version: vscode/1.96.0",
                    "-d",
                    &format!("@{}", tmp_path),
                    "--max-time",
                    "120",
                    &url,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| anyhow!("failed to spawn curl: {}", e))?;

            let _ = tokio::fs::remove_file(&tmp_path).await;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(anyhow!(
                    "curl failed (exit {:?}): {}",
                    output.status.code(),
                    stderr
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
            let message = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|first| first.get("message"));

            let content = message.and_then(|m| m.get("content")).and_then(|c| {
                if c.is_null() {
                    None
                } else {
                    c.as_str().map(|s| s.to_string())
                }
            });

            let tool_calls: Vec<LlmToolCall> = message
                .and_then(|m| m.get("tool_calls"))
                .and_then(|tc| serde_json::from_value::<Vec<LlmToolCall>>(tc.clone()).ok())
                .unwrap_or_default();

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
                            parts.push(serde_json::json!({
                                "functionCall": {
                                    "name": tc.function.name,
                                    "args": args
                                }
                            }));
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
                    let response_data: Value = serde_json::from_str(
                        msg.content.as_deref().unwrap_or("{}"),
                    )
                    .unwrap_or(serde_json::json!({"result": msg.content.as_deref().unwrap_or("")}));

                    // Try to find the tool name from tool_call_id
                    let name = msg.tool_call_id.as_deref().unwrap_or("unknown");
                    contents.push(serde_json::json!({
                        "role": "function",
                        "parts": [{
                            "functionResponse": {
                                "name": name,
                                "response": response_data
                            }
                        }]
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

            let url = format!(
                "{}/models/{}:generateContent?key={}",
                base_url.trim_end_matches('/'),
                model,
                api_key
            );

            let tmp_path = format!("/tmp/rune_gemini_req_{}.json", std::process::id());
            let payload_str = serde_json::to_string(&payload)
                .map_err(|e| anyhow!("failed to serialize Gemini request: {}", e))?;
            tokio::fs::write(&tmp_path, &payload_str)
                .await
                .map_err(|e| anyhow!("failed to write temp payload: {}", e))?;

            debug!(url = %url, payload_len = payload_str.len(), "sending Gemini request");

            let output = Command::new("curl")
                .args([
                    "-s",
                    "-S",
                    "-X",
                    "POST",
                    "-H",
                    "Content-Type: application/json",
                    "-d",
                    &format!("@{}", tmp_path),
                    "--max-time",
                    "120",
                    &url,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| anyhow!("failed to spawn curl: {}", e))?;

            let _ = tokio::fs::remove_file(&tmp_path).await;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(anyhow!("curl failed: {}", stderr));
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
            let mut tc_counter = 0u32;

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
                            tc_counter += 1;
                            tool_calls.push(LlmToolCall {
                                id: format!("gemini_tc_{}", tc_counter),
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

#[cfg(test)]
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
        };
        let err = reg
            .chat(req)
            .await
            .expect_err("permanent failure should not fallback");
        assert!(err.to_string().contains("failed to parse response"));
    }
}
