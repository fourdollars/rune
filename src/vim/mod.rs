//! Vim integration module for Rune (`--features vim`).

pub mod completion;
pub mod fim;
pub mod plugin;
pub mod policy;
pub mod postprocess;
pub mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::config::RuneConfig;
use crate::provider::ProviderRegistry;
use completion::{execute_completion, CompletionCache};
use policy::{SharedRateLimiter, VimPolicy};
use protocol::*;

pub struct VimServerState {
    pub config: RuneConfig,
    pub providers: Arc<ProviderRegistry>,
    pub policy: VimPolicy,
    pub rate_limiter: SharedRateLimiter,
    pub cache: CompletionCache,
    pub active_model: Arc<RwLock<String>>,
    pub thinking: Arc<RwLock<Option<String>>>,
    pub in_flight: Arc<RwLock<u32>>,
    pub last_latency_ms: Arc<RwLock<u64>>,
    pub last_error: Arc<RwLock<Option<String>>>,
}

pub async fn run_stdio_server(config_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let config = crate::config::load_without_clap_path(config_path).unwrap_or_default();
    let providers =
        ProviderRegistry::build_from_config(&config).unwrap_or_else(|_| ProviderRegistry::new());

    let first_user_model = config
        .model
        .split(',')
        .next()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("gpt-4o-mini");

    let available_models = providers.list_models().await.ok().unwrap_or_default();

    let active_model = if available_models.iter().any(|m| m.id == first_user_model) {
        first_user_model.to_string()
    } else if let Some(first_available) = available_models.first() {
        first_available.id.clone()
    } else {
        first_user_model.to_string()
    };

    let thinking = config.thinking.clone();

    let state = Arc::new(VimServerState {
        config,
        providers: Arc::new(providers),
        policy: VimPolicy::new(),
        rate_limiter: SharedRateLimiter::default(),
        cache: CompletionCache::new(),
        active_model: Arc::new(RwLock::new(active_model)),
        thinking: Arc::new(RwLock::new(thinking)),
        in_flight: Arc::new(RwLock::new(0)),
        last_latency_ms: Arc::new(RwLock::new(0)),
        last_error: Arc::new(RwLock::new(None)),
    });

    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let err = JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e), None);
                let _ = writeln!(writer, "{}", serde_json::to_string(&err).unwrap());
                let _ = writer.flush();
                continue;
            }
        };

        let response = handle_rpc_request(&state, req).await;
        if let Some(res) = response {
            let json = serde_json::to_string(&res).unwrap();
            let _ = writeln!(writer, "{}", json);
            let _ = writer.flush();
        }
    }

    Ok(())
}

async fn handle_rpc_request(
    state: &Arc<VimServerState>,
    req: JsonRpcRequest,
) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        "rune/initialize" => {
            let params: InitializeParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return Some(JsonRpcResponse::error(
                            req.id,
                            -32602,
                            "Invalid initialize params",
                            None,
                        ))
                    }
                };

            if params.protocol_version != PROTOCOL_VERSION {
                return Some(JsonRpcResponse::error(
                    req.id,
                    ERR_PROTOCOL_VERSION_UNSUPPORTED,
                    "Unsupported protocol version",
                    None,
                ));
            }

            let (remaining, capacity) = state.rate_limiter.budget();
            let available_models = state
                .providers
                .list_models()
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>();
            let available_levels = vec![
                "off".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ];

            let result = InitializeResult {
                protocol_version: PROTOCOL_VERSION,
                server_version: format!("rune {}", env!("CARGO_PKG_VERSION")),
                provider: state
                    .config
                    .provider
                    .clone()
                    .unwrap_or_else(|| "copilot".to_string()),
                model: state.active_model.read().await.clone(),
                available_models,
                thinking: state.thinking.read().await.clone(),
                available_levels,
                fim: "native".to_string(),
                max_candidates: 1,
                rate_limit: RateLimitInfo {
                    capacity,
                    refill_per_min: 30,
                },
            };
            Some(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(result).unwrap(),
            ))
        }
        "rune/completion" => {
            let params: CompletionParams =
                match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return Some(JsonRpcResponse::error(
                            req.id,
                            -32602,
                            "Invalid completion params",
                            None,
                        ))
                    }
                };

            // Policy check (§9)
            if state.policy.is_excluded(&params.filepath, &params.language) {
                return Some(JsonRpcResponse::error(
                    req.id,
                    ERR_POLICY_EXCLUDED,
                    "File excluded by policy",
                    None,
                ));
            }

            // Rate limit check (§9.1)
            if !state.rate_limiter.try_consume() {
                let (rem, cap) = state.rate_limiter.budget();
                return Some(JsonRpcResponse::error(
                    req.id,
                    ERR_RATE_LIMITED,
                    "Rate limit exceeded",
                    Some(serde_json::json!({
                        "source": "local",
                        "remaining": rem,
                        "capacity": cap,
                        "refill_in_ms": state.rate_limiter.refill_in_ms()
                    })),
                ));
            }

            let start = std::time::Instant::now();
            *state.in_flight.write().await += 1;

            let current_model = state.active_model.read().await.clone();
            let result =
                execute_completion(&params, &state.cache, &state.providers, &current_model).await;

            *state.in_flight.write().await -= 1;
            let elapsed = start.elapsed().as_millis() as u64;
            *state.last_latency_ms.write().await = elapsed;

            match result {
                Ok(res) => Some(JsonRpcResponse::success(
                    req.id,
                    serde_json::to_value(res).unwrap(),
                )),
                Err(e) => {
                    *state.last_error.write().await = Some(e.to_string());
                    Some(JsonRpcResponse::error(
                        req.id,
                        -32002,
                        format!("Completion failed: {}", e),
                        None,
                    ))
                }
            }
        }
        "rune/cancel" => {
            state.rate_limiter.refund();
            None
        }
        "rune/chat" => {
            let params: ChatParams = match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                Some(p) => p,
                None => {
                    return Some(JsonRpcResponse::error(
                        req.id,
                        -32602,
                        "Invalid chat params",
                        None,
                    ))
                }
            };

            let prompt = if let Some(sel) = &params.selection {
                format!(
                    "{}\n\nContext:\n```{}\n{}\n```",
                    params.prompt, params.language, sel.text
                )
            } else {
                params.prompt.clone()
            };

            let request = crate::provider::LlmRequest {
                model: state.active_model.read().await.clone(),
                messages: vec![crate::provider::LlmMessage {
                    role: "user".to_string(),
                    name: None,
                    content: Some(prompt),
                    content_parts: None,
                    tool_calls: None,
                    tool_call_id: None,
                }],
                tools: None,
                max_tokens: Some(1024),
                thinking: None,
            };

            match state.providers.chat(request).await {
                Ok(resp) => {
                    let text = resp.content.unwrap_or_default();
                    Some(JsonRpcResponse::success(
                        req.id,
                        serde_json::json!({ "text": text, "finish_reason": "stop" }),
                    ))
                }
                Err(e) => Some(JsonRpcResponse::error(req.id, -32002, e.to_string(), None)),
            }
        }
        "rune/edit" => {
            let params: EditParams = match req.params.and_then(|p| serde_json::from_value(p).ok()) {
                Some(p) => p,
                None => {
                    return Some(JsonRpcResponse::error(
                        req.id,
                        -32602,
                        "Invalid edit params",
                        None,
                    ))
                }
            };

            let (start_line, end_line, sel_text) = if let Some(sel) = &params.selection {
                (sel.start_line, sel.end_line, sel.text.clone())
            } else {
                (1, 1, "".to_string())
            };

            let prompt = format!(
                "You are an automated code refactoring engine. Fix/edit the following code according to the instruction: {}\n\nCode:\n```{}\n{}\n```\nReturn ONLY the replacement code inside a markdown block.",
                params.prompt, params.language, sel_text
            );

            let request = crate::provider::LlmRequest {
                model: state.active_model.read().await.clone(),
                messages: vec![crate::provider::LlmMessage {
                    role: "user".to_string(),
                    name: None,
                    content: Some(prompt),
                    content_parts: None,
                    tool_calls: None,
                    tool_call_id: None,
                }],
                tools: None,
                max_tokens: Some(1024),
                thinking: None,
            };

            match state.providers.chat(request).await {
                Ok(resp) => {
                    let raw = resp.content.unwrap_or_default();
                    let clean =
                        postprocess::postprocess_completion(&raw, "", "", None).unwrap_or(raw);
                    let result = EditResult {
                        edits: vec![EditItem {
                            start_line,
                            end_line,
                            new_text: clean,
                        }],
                        summary: "Applied AI edit".to_string(),
                    };
                    Some(JsonRpcResponse::success(
                        req.id,
                        serde_json::to_value(result).unwrap(),
                    ))
                }
                Err(e) => Some(JsonRpcResponse::error(req.id, -32002, e.to_string(), None)),
            }
        }
        "rune/model" => {
            let params: Option<ModelParams> =
                req.params.and_then(|p| serde_json::from_value(p).ok());
            if let Some(p) = params {
                if let Some(new_model) = p.model {
                    if !new_model.trim().is_empty() {
                        *state.active_model.write().await = new_model.trim().to_string();
                    }
                }
            }

            let available = state
                .providers
                .list_models()
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>();

            let current_model = state.active_model.read().await.clone();
            let res = ModelResult {
                active_model: current_model,
                available_models: available,
            };

            Some(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(res).unwrap(),
            ))
        }
        "rune/thinking" => {
            let params: Option<ThinkingParams> =
                req.params.and_then(|p| serde_json::from_value(p).ok());
            if let Some(p) = params {
                if let Some(new_thinking) = p.thinking {
                    let level = new_thinking.trim().to_lowercase();
                    if level == "off" || level == "none" || level.is_empty() {
                        *state.thinking.write().await = None;
                    } else {
                        *state.thinking.write().await = Some(level);
                    }
                }
            }

            let current_thinking = state
                .thinking
                .read()
                .await
                .clone()
                .unwrap_or_else(|| "off".to_string());
            let res = ThinkingResult {
                active_thinking: current_thinking,
                available_levels: vec![
                    "off".to_string(),
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                    "xhigh".to_string(),
                ],
            };

            Some(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(res).unwrap(),
            ))
        }
        "rune/status" => {
            let (rem, cap) = state.rate_limiter.budget();
            let available_models = state
                .providers
                .list_models()
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>();
            let available_levels = vec![
                "off".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ];

            let res = StatusResult {
                state: "idle".to_string(),
                provider: state
                    .config
                    .provider
                    .clone()
                    .unwrap_or_else(|| "copilot".to_string()),
                model: state.active_model.read().await.clone(),
                available_models,
                thinking: state.thinking.read().await.clone(),
                available_levels,
                enabled: true,
                in_flight: *state.in_flight.read().await,
                rate_budget: RateBudgetInfo {
                    remaining: rem,
                    capacity: cap,
                },
                last_error: state.last_error.read().await.clone(),
                last_latency_ms: *state.last_latency_ms.read().await,
            };
            Some(JsonRpcResponse::success(
                req.id,
                serde_json::to_value(res).unwrap(),
            ))
        }
        "rune/shutdown" => {
            info!("Vim shutdown requested");
            Some(JsonRpcResponse::success(
                req.id,
                serde_json::json!({"shutdown": true}),
            ))
        }
        _ => Some(JsonRpcResponse::error(
            req.id,
            -32601,
            "Method not found",
            None,
        )),
    }
}

pub fn handle_vim_cli(args: &[String]) -> anyhow::Result<()> {
    if args.len() > 2 && args[2] == "install" {
        plugin::install_plugin()?;
    } else if args.len() > 2 && args[2] == "stdio" {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_stdio_server(None))?;
    } else {
        println!("Usage:");
        println!("  rune vim stdio     — Start Vim JSON-RPC stdio daemon");
        println!("  rune vim install   — Install rune.vim plugin into ~/.vim and ~/.config/nvim");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_vim_rpc_initialize_and_model_thinking_status() {
        let state = Arc::new(VimServerState {
            config: RuneConfig::default(),
            providers: Arc::new(ProviderRegistry::new()),
            policy: VimPolicy::new(),
            rate_limiter: SharedRateLimiter::default(),
            cache: CompletionCache::new(),
            active_model: Arc::new(RwLock::new("gemini-3.6-flash".to_string())),
            thinking: Arc::new(RwLock::new(Some("high".to_string()))),
            in_flight: Arc::new(RwLock::new(0)),
            last_latency_ms: Arc::new(RwLock::new(0)),
            last_error: Arc::new(RwLock::new(None)),
        });

        // 1. Test rune/initialize
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "rune/initialize".to_string(),
            params: Some(json!({"protocol_version": 1})),
        };
        let init_resp = handle_rpc_request(&state, init_req).await.unwrap();
        assert!(init_resp.error.is_none());
        let init_res: InitializeResult = serde_json::from_value(init_resp.result.unwrap()).unwrap();
        assert_eq!(init_res.model, "gemini-3.6-flash");
        assert_eq!(init_res.thinking, Some("high".to_string()));
        assert_eq!(
            init_res.available_levels,
            vec!["off", "low", "medium", "high", "xhigh"]
        );

        // 2. Test rune/model switch
        let model_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "rune/model".to_string(),
            params: Some(json!({"model": "claude-3-5-sonnet"})),
        };
        let model_resp = handle_rpc_request(&state, model_req).await.unwrap();
        assert!(model_resp.error.is_none());
        let model_res: ModelResult = serde_json::from_value(model_resp.result.unwrap()).unwrap();
        assert_eq!(model_res.active_model, "claude-3-5-sonnet");

        // 3. Test rune/thinking switch
        let think_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "rune/thinking".to_string(),
            params: Some(json!({"thinking": "xhigh"})),
        };
        let think_resp = handle_rpc_request(&state, think_req).await.unwrap();
        assert!(think_resp.error.is_none());
        let think_res: ThinkingResult = serde_json::from_value(think_resp.result.unwrap()).unwrap();
        assert_eq!(think_res.active_thinking, "xhigh");

        // 4. Test rune/status
        let status_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "rune/status".to_string(),
            params: None,
        };
        let status_resp = handle_rpc_request(&state, status_req).await.unwrap();
        assert!(status_resp.error.is_none());
        let status_res: StatusResult = serde_json::from_value(status_resp.result.unwrap()).unwrap();
        assert_eq!(status_res.model, "claude-3-5-sonnet");
        assert_eq!(status_res.thinking, Some("xhigh".to_string()));
        assert_eq!(
            status_res.available_levels,
            vec!["off", "low", "medium", "high", "xhigh"]
        );
    }
}
