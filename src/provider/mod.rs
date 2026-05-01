use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use std::future::Future;
use tokio::process::Command;
use std::process::Stdio;

/// LLM 請求
#[derive(Debug, Clone, Serialize)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    pub tools: Option<Vec<serde_json::Value>>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,     // "system", "user", "assistant", "tool"
    pub content: Option<String>,
    pub tool_calls: Option<Vec<LlmToolCall>>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub function: LlmFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFunction {
    pub name: String,
    pub arguments: String,  // JSON string
}

/// LLM 回應
#[derive(Debug, Clone, Deserialize)]
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

/// Provider trait — 不用 async_trait，用手動 Pin<Box<Future>>
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn chat(&self, request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>>;
}

pub struct OpenAiProvider {
    pub api_key: String,
    pub base_url: String, // default: https://api.openai.com/v1
    pub name: String,
}

impl OpenAiProvider {
    pub fn new(name: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        OpenAiProvider { api_key, base_url: base, name }
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn chat(&self, request: LlmRequest) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + '_>> {
        // Clone what's needed so the future doesn't borrow &self
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();

        // Build a payload compatible with OpenAI chat/completions
        let mut payload = serde_json::map::Map::new();
        payload.insert("model".to_string(), serde_json::Value::String(request.model.clone()));

        // messages: map LlmMessage -> { role, content }
        let msgs: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut mm = serde_json::map::Map::new();
                mm.insert("role".to_string(), Value::String(m.role.clone()));
                mm.insert("content".to_string(), Value::String(m.content.clone().unwrap_or_default()));
                Value::Object(mm)
            })
            .collect();
        payload.insert("messages".to_string(), Value::Array(msgs));

        if let Some(max) = request.max_tokens {
            payload.insert("max_tokens".to_string(), Value::Number(serde_json::Number::from(max)));
        }

        // Tools/support for function calling not implemented here; skip

        let payload_json = serde_json::Value::Object(payload).to_string();
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        Box::pin(async move {
            // Use a heredoc to safely pass JSON payload to curl
            let cmd = format!(
                "curl -s -X POST -H 'Content-Type: application/json' -H \"Authorization: Bearer $OPENAI_API_KEY\" '{url}' -d @- <<'JSON'\n{payload}\nJSON",
                url = url,
                payload = payload_json
            );

            let output = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .env("OPENAI_API_KEY", api_key)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| anyhow!("failed to spawn curl: {}", e))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !output.status.success() {
                return Err(anyhow!("curl failed: exit {:?}\nstdout: {}\nstderr: {}", output.status.code(), stdout, stderr));
            }

            // Parse response JSON from OpenAI-like API
            let v: serde_json::Value = serde_json::from_str(&stdout)
                .map_err(|e| anyhow!("failed to parse json response: {}\nraw: {}", e, stdout))?;

            // Extract model
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();

            // Extract content from choices[0].message.content OR choices[0].text
            let content = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|first| {
                    first
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .or_else(|| first.get("text"))
                })
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            // Extract usage
            let usage = v
                .get("usage")
                .and_then(|u| serde_json::from_value::<TokenUsage>(u.clone()).ok())
                .unwrap_or_default();

            // Tool calls - not parsing OpenAI function_call here; leave empty
            let tool_calls: Vec<LlmToolCall> = Vec::new();

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

    /// 呼叫預設 provider，失敗則依序 fallback
    pub async fn chat(&self, request: LlmRequest) -> Result<LlmResponse> {
        if self.providers.is_empty() {
            return Err(anyhow!("no providers registered"));
        }

        // try default first, then fallback sequentially
        let len = self.providers.len();
        let mut idx = self.default_provider.min(len - 1);

        for _ in 0..len {
            let provider = &self.providers[idx];
            match provider.chat(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    // try next
                    idx = (idx + 1) % len;
                    // continue
                    let _ = e; // ignore for now
                }
            }
        }

        Err(anyhow!("all providers failed"))
    }

    /// 直接呼叫指定 provider
    pub async fn chat_with(&self, provider_name: &str, request: LlmRequest) -> Result<LlmResponse> {
        for p in &self.providers {
            if p.name() == provider_name {
                return p.chat(request).await;
            }
        }
        Err(anyhow!("provider not found: {}", provider_name))
    }
}
