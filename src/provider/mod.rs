use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use std::future::Future;
use tokio::process::Command;
use std::process::Stdio;
use tracing::{debug, info, warn};

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

/// Provider trait.
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn chat(&self, request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>>;
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
        OpenAiProvider { api_key, base_url: base, provider_name: name }
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn chat(&self, request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
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
            tokio::fs::write(&tmp_path, &payload).await
                .map_err(|e| anyhow!("failed to write temp payload: {}", e))?;

            let output = Command::new("curl")
                .args([
                    "-s", "-S",
                    "-X", "POST",
                    "-H", "Content-Type: application/json",
                    "-H", &format!("Authorization: Bearer {}", api_key),
                    "-d", &format!("@{}", tmp_path),
                    "--max-time", "120",
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
                return Err(anyhow!("curl failed (exit {:?}): {}", output.status.code(), stderr));
            }

            // Parse response JSON
            let v: Value = serde_json::from_str(&stdout)
                .map_err(|e| anyhow!("failed to parse response JSON: {}\nraw: {}", e, &stdout[..stdout.len().min(500)]))?;

            // Check for API error
            if let Some(err) = v.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                return Err(anyhow!("API error: {}", msg));
            }

            // Extract model
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();

            // Extract from choices[0].message
            let message = v.get("choices")
                .and_then(|c| c.get(0))
                .and_then(|first| first.get("message"));

            let content = message
                .and_then(|m| m.get("content"))
                .and_then(|c| {
                    if c.is_null() { None } else { c.as_str().map(|s| s.to_string()) }
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
            let usage = v.get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

            debug!(model = %model, content_len = content.as_ref().map(|c| c.len()).unwrap_or(0),
                   tool_calls = tool_calls.len(), tokens = usage.total_tokens, "LLM response received");

            Ok(LlmResponse { content, tool_calls, usage, model })
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
                if now < expires_at - 60 { // refresh 60s before expiry
                    return Ok((token.clone(), endpoint.clone()));
                }
            }
        }

        // Fetch new token
        debug!("refreshing GitHub Copilot session token");
        let output = Command::new("curl")
            .args([
                "-sS",
                "--max-time", "10",
                "-H", &format!("Authorization: token {}", self.pat),
                "-H", "editor-version: vscode/1.96.0",
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

        let v: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| anyhow!("failed to parse token response: {}\nraw: {}", e, &stdout[..stdout.len().min(200)]))?;

        let token = v.get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("no token in response"))?
            .to_string();

        let expires_at = v.get("expires_at")
            .and_then(|e| e.as_u64())
            .unwrap_or(0);

        let endpoint = v.get("endpoints")
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

    fn chat(&self, request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        Box::pin(async move {
            let (token, endpoint) = self.get_token().await?;

            let payload = serde_json::to_string(&request)
                .map_err(|e| anyhow!("failed to serialize request: {}", e))?;

            let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
            let tmp_path = format!("/tmp/rune_copilot_req_{}.json", std::process::id());
            tokio::fs::write(&tmp_path, &payload).await
                .map_err(|e| anyhow!("failed to write temp payload: {}", e))?;

            let output = Command::new("curl")
                .args([
                    "-sS",
                    "-X", "POST",
                    "-H", "Content-Type: application/json",
                    "-H", &format!("Authorization: Bearer {}", token),
                    "-H", "editor-version: vscode/1.96.0",
                    "-d", &format!("@{}", tmp_path),
                    "--max-time", "120",
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
                return Err(anyhow!("curl failed (exit {:?}): {}", output.status.code(), stderr));
            }

            let v: serde_json::Value = serde_json::from_str(&stdout)
                .map_err(|e| anyhow!("failed to parse response: {}\nraw: {}", e, &stdout[..stdout.len().min(500)]))?;

            if let Some(err) = v.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                return Err(anyhow!("Copilot API error: {}", msg));
            }

            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let message = v.get("choices")
                .and_then(|c| c.get(0))
                .and_then(|first| first.get("message"));

            let content = message
                .and_then(|m| m.get("content"))
                .and_then(|c| if c.is_null() { None } else { c.as_str().map(|s| s.to_string()) });

            let tool_calls: Vec<LlmToolCall> = message
                .and_then(|m| m.get("tool_calls"))
                .and_then(|tc| serde_json::from_value::<Vec<LlmToolCall>>(tc.clone()).ok())
                .unwrap_or_default();

            let usage = v.get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

            Ok(LlmResponse { content, tool_calls, usage, model })
        })
    }
}
pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
    default_provider: usize,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        ProviderRegistry { providers: Vec::new(), default_provider: 0 }
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Call default provider, fallback on failure.
    pub async fn chat(&self, request: LlmRequest) -> Result<LlmResponse> {
        if self.providers.is_empty() {
            return Err(anyhow!("no providers registered"));
        }

        let len = self.providers.len();
        let mut idx = self.default_provider.min(len - 1);
        let mut last_err = anyhow!("no providers");

        for attempt in 0..len {
            let provider = &self.providers[idx];
            match provider.chat(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    warn!(provider = provider.name(), attempt, error = %e, "provider failed, trying next");
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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingProvider { name: String }
    impl FailingProvider { fn new(name: &str) -> Self { Self { name: name.to_string() } } }
    impl Provider for FailingProvider {
        fn name(&self) -> &str { &self.name }
        fn chat(&self, _request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
            Box::pin(async move { Err(anyhow!("simulated failure")) })
        }
    }

    struct SucceedProvider { name: String, resp: LlmResponse }
    impl SucceedProvider { fn new(name: &str, resp: LlmResponse) -> Self { Self { name: name.to_string(), resp } } }
    impl Provider for SucceedProvider {
        fn name(&self) -> &str { &self.name }
        fn chat(&self, _request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
            let r = self.resp.clone();
            Box::pin(async move { Ok(r) })
        }
    }

    #[tokio::test]
    async fn test_provider_registry_fallback() {
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FailingProvider::new("fail")));
        let resp = LlmResponse { content: Some("ok".into()), tool_calls: vec![], usage: TokenUsage::default(), model: "m1".into() };
        reg.register(Box::new(SucceedProvider::new("succ", resp)));

        let req = LlmRequest { model: "m".into(), messages: vec![], tools: None, max_tokens: None };
        let res = reg.chat(req).await.expect("fallback should succeed");
        assert_eq!(res.content, Some("ok".to_string()));
    }

    #[tokio::test]
    async fn test_chat_with_specific_provider() {
        let mut reg = ProviderRegistry::new();
        let resp = LlmResponse { content: Some("hello".into()), tool_calls: vec![], usage: TokenUsage::default(), model: "m1".into() };
        reg.register(Box::new(SucceedProvider::new("p1", resp)));

        let req = LlmRequest { model: "m".into(), messages: vec![], tools: None, max_tokens: None };
        let res = reg.chat_with("p1", req).await.expect("should find p1");
        assert_eq!(res.content, Some("hello".to_string()));
    }
}
