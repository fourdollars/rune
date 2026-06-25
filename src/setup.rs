use colored::Colorize;
use std::io::{self, Write};
use std::path::PathBuf;

/// Existing config values loaded for defaults.
struct ExistingConfig {
    provider: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    skills_dir: Option<String>,
    thinking: Option<String>,
    embedding_enabled: Option<bool>,
    /// Raw [policy] section to preserve (includes header)
    policy_section: Option<String>,
    /// Raw [embedding] section to preserve (includes header)
    embedding_section: Option<String>,
    /// Raw [notes] section to preserve (includes header)
    notes_section: Option<String>,
    notes_port: Option<u16>,
    notes_bind: Option<String>,
    notes_github_client_id: Option<String>,
    notes_github_client_secret: Option<String>,
    notes_github_admins: Option<String>,
    notes_github_users: Option<String>,
    notes_github_guests: Option<String>,
    notes_local_admins: Option<String>,
    notes_local_users: Option<String>,
    notes_local_guests: Option<String>,
    notes_model: Option<String>,
    openrouter_zdr: Option<bool>,
}

/// Extract a raw TOML section (from [header] line to next [header] or EOF).
pub fn extract_toml_section(content: &str, header: &str) -> Option<String> {
    let mut result = String::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if trimmed == header
                || trimmed.starts_with(&format!("{}.", &header[..header.len() - 1]))
            {
                in_section = true;
                result.push_str(line);
                result.push('\n');
                continue;
            } else if in_section {
                break;
            }
        }
        if in_section {
            result.push_str(line);
            result.push('\n');
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn load_existing_config(path: &std::path::Path) -> ExistingConfig {
    if !path.exists() {
        return ExistingConfig {
            provider: None,
            api_key: None,
            model: None,
            base_url: None,
            skills_dir: None,
            thinking: None,
            embedding_enabled: None,
            policy_section: None,
            embedding_section: None,
            notes_section: None,
            notes_port: None,
            notes_bind: None,
            notes_github_client_id: None,
            notes_github_client_secret: None,
            notes_github_admins: None,
            notes_github_users: None,
            notes_github_guests: None,
            notes_local_admins: None,
            notes_local_users: None,
            notes_local_guests: None,
            notes_model: None,
            openrouter_zdr: None,
        };
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let table: toml::Table = content.parse().unwrap_or_default();

    let policy_section = extract_toml_section(&content, "[policy]");
    let embedding_section = extract_toml_section(&content, "[embedding]");
    let notes_section = extract_toml_section(&content, "[notes]");
    let notes_table = table.get("notes").and_then(|v| v.as_table());

    ExistingConfig {
        provider: table
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        api_key: table
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model: table
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        base_url: table
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        skills_dir: table
            .get("skills_dir")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        thinking: table
            .get("thinking")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        embedding_enabled: table
            .get("embedding")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("enabled"))
            .and_then(|v| v.as_bool()),
        policy_section,
        embedding_section,
        notes_section,
        notes_port: notes_table
            .and_then(|t| t.get("port"))
            .and_then(|v| v.as_integer())
            .map(|i| i as u16),
        notes_bind: notes_table
            .and_then(|t| t.get("bind"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        notes_github_client_id: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("client_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        notes_github_client_secret: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("client_secret"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        notes_github_admins: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("admins"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_github_users: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("users"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_github_guests: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("github"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("guests"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_local_admins: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("local"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("admins"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_local_users: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("local"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("users"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_local_guests: table
            .get("notes")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("local"))
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("guests"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        notes_model: notes_table
            .and_then(|t| t.get("model"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        openrouter_zdr: table.get("openrouter_zdr").and_then(|v| v.as_bool()),
    }
}

const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// GitHub Device Flow OAuth — interactive token acquisition.
/// Returns the access token on success, or None on failure/cancellation.
async fn github_device_flow() -> Option<String> {
    println!();
    println!("  {} Starting GitHub Device Flow...", "⚙".cyan());
    println!();

    // Step 1: Request device code
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("client_id={}&scope=read:user", GITHUB_CLIENT_ID))
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  {} Failed to contact GitHub: {}", "✗".red(), e);
            return None;
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  {} Invalid response from GitHub: {}", "✗".red(), e);
            return None;
        }
    };

    let device_code = body["device_code"].as_str()?;
    let user_code = body["user_code"].as_str()?;
    let verification_uri = body["verification_uri"]
        .as_str()
        .unwrap_or("https://github.com/login/device");
    let expires_in = body["expires_in"].as_u64().unwrap_or(900);
    let interval = body["interval"].as_u64().unwrap_or(5);

    // Step 2: Show the code to the user
    println!("  ┌─────────────────────────────────────────┐");
    println!(
        "  │  Your code: {:<28}│",
        format!("{}", user_code).cyan().bold()
    );
    println!("  └─────────────────────────────────────────┘");
    println!();
    println!(
        "  {} Open {} in your browser",
        "→".yellow(),
        verification_uri.cyan()
    );
    println!("  {} Enter the code shown above", "→".yellow());
    println!();
    println!(
        "  {} Waiting for authorization (expires in {}s)...",
        "⏳".dimmed(),
        expires_in
    );
    println!("  {} Press Ctrl+C to cancel", "ℹ".dimmed());

    // Step 3: Poll for the access token
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
    let poll_interval = std::time::Duration::from_secs(interval);

    loop {
        tokio::time::sleep(poll_interval).await;

        if std::time::Instant::now() > deadline {
            eprintln!("  {} Device code expired. Please try again.", "✗".red());
            return None;
        }

        let poll_resp = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "client_id={}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
                GITHUB_CLIENT_ID, device_code
            ))
            .send()
            .await;

        let poll_resp = match poll_resp {
            Ok(r) => r,
            Err(_) => continue,
        };

        let poll_body: serde_json::Value = match poll_resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(token) = poll_body["access_token"].as_str() {
            println!();
            println!("  {} Authorization successful!", "✓".green().bold());
            return Some(token.to_string());
        }

        match poll_body["error"].as_str() {
            Some("authorization_pending") => {
                // Still waiting — continue polling
                eprint!(".");
                let _ = io::stderr().flush();
            }
            Some("slow_down") => {
                // Back off
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            Some("expired_token") => {
                eprintln!();
                eprintln!("  {} Device code expired. Please try again.", "✗".red());
                return None;
            }
            Some("access_denied") => {
                eprintln!();
                eprintln!("  {} Authorization was denied.", "✗".red());
                return None;
            }
            Some(other) => {
                eprintln!();
                eprintln!("  {} Unexpected error: {}", "✗".red(), other);
                return None;
            }
            None => continue,
        }
    }
}

#[derive(serde::Deserialize, Clone)]
struct OpenRouterPricing {
    prompt: Option<String>,
    completion: Option<String>,
}

#[derive(serde::Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(serde::Deserialize)]
struct OpenRouterModel {
    id: String,
    pricing: Option<OpenRouterPricing>,
}

async fn fetch_openrouter_models(openrouter_zdr: bool) -> Option<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let zdr_set = if openrouter_zdr {
        // Fetch ZDR endpoints to filter models by ZDR support
        let zdr_resp = client
            .get("https://openrouter.ai/api/v1/endpoints/zdr")
            .send()
            .await
            .ok()?;
        if !zdr_resp.status().is_success() {
            return None;
        }

        #[derive(serde::Deserialize)]
        struct OpenRouterZdrEndpoint {
            model_id: String,
        }

        #[derive(serde::Deserialize)]
        struct OpenRouterZdrEndpointsResponse {
            data: Vec<OpenRouterZdrEndpoint>,
        }

        let zdr_body: OpenRouterZdrEndpointsResponse = zdr_resp.json().await.ok()?;
        let set: std::collections::HashSet<String> =
            zdr_body.data.into_iter().map(|e| e.model_id).collect();
        Some(set)
    } else {
        None
    };

    let resp = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: OpenRouterModelsResponse = resp.json().await.ok()?;

    let model_ids: Vec<String> = body.data.into_iter().map(|m| m.id).collect();
    let model_set: std::collections::HashSet<String> = model_ids.iter().cloned().collect();

    let mut filtered = Vec::new();
    for m in &model_ids {
        let is_zdr_ok = !openrouter_zdr || zdr_set.as_ref().map(|s| s.contains(m)).unwrap_or(false);
        if is_zdr_ok && !m.ends_with(":free") {
            let free = format!("{}:free", m);
            if model_set.contains(&free) {
                filtered.push(m.clone());
            }
        }
    }
    filtered.sort();
    Some(filtered)
}

/// Auto-detect available models for a provider using the same mechanism `rune notes` uses
/// (ProviderRegistry::list_models). Returns model IDs sorted by the provider, or None on
/// network/auth failure or if the provider's `list_models` is unimplemented.
async fn fetch_provider_models(
    provider_id: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Option<Vec<crate::provider::ModelInfo>> {
    if api_key.is_empty() {
        return None;
    }
    let mut cfg = crate::config::RuneConfig::default();
    cfg.api_key = Some(api_key.to_string());
    cfg.provider = Some(provider_id.to_string());
    cfg.base_url = base_url.map(|s| s.to_string());

    // Try via full provider machinery (PAT → session token exchange for Copilot).
    if let Ok(registry) = crate::serve::api::build_provider_pub(&cfg) {
        if let Ok(models) = registry.list_models().await {
            if !models.is_empty() {
                return Some(models);
            }
        }
    }

    // For Copilot: the key may already be a session token, so the PAT exchange
    // above can fail.  Try Bearer auth directly against the known endpoints.
    if provider_id == "github-copilot" {
        return fetch_copilot_models_bearer(api_key).await;
    }

    None
}

/// Call the Copilot models endpoint using `api_key` as a Bearer session token.
/// Mirrors the exact filter used by `CopilotProvider::list_models`.
async fn fetch_copilot_models_bearer(
    session_token: &str,
) -> Option<Vec<crate::provider::ModelInfo>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    for endpoint in &[
        "https://api.githubcopilot.com/models",
        "https://api.business.githubcopilot.com/models",
    ] {
        let resp = client
            .get(*endpoint)
            .bearer_auth(session_token)
            .header("User-Agent", "rune/0.1.0")
            .header("Copilot-Integration-Id", "vscode-chat")
            .send()
            .await;
        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let arr = match body
            .get("data")
            .or_else(|| body.get("models"))
            .and_then(|v| v.as_array())
        {
            Some(a) => a,
            None => continue,
        };
        let mut models: Vec<crate::provider::ModelInfo> = arr
            .iter()
            .filter(|m| {
                m.get("model_picker_enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .filter(|m| {
                m.get("supported_endpoints")
                    .and_then(|v| v.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            })
            .filter_map(|m| {
                let id = m
                    .get("id")
                    .or_else(|| m.get("name"))
                    .and_then(|v| v.as_str())
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
                Some(crate::provider::ModelInfo {
                    id,
                    provider: Some("github-copilot".to_string()),
                    context_window,
                    reasoning_efforts,
                    supported_endpoints,
                })
            })
            .collect();
        if !models.is_empty() {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            return Some(models);
        }
    }
    None
}

async fn fetch_gemini_models(api_key: &str, base_url: &str) -> Option<Vec<String>> {
    if api_key.is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let url = format!("{}/models", base_url);
    let resp = client
        .get(&url)
        .header("x-goog-api-key", api_key)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let arr = body.get("models")?.as_array()?;
    let mut models: Vec<String> = arr
        .iter()
        .filter(|m| {
            m.get("supportedGenerationMethods")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|e| e.as_str() == Some("generateContent")))
                .unwrap_or(false)
        })
        .filter_map(|m| {
            m.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_start_matches("models/").to_string())
        })
        .collect();
    models.sort();
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

async fn fetch_openrouter_embedding_models(openrouter_zdr: bool) -> Option<(Vec<String>, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let zdr_set = if openrouter_zdr {
        // Fetch ZDR endpoints to filter models by ZDR support
        let zdr_resp = client
            .get("https://openrouter.ai/api/v1/endpoints/zdr")
            .send()
            .await
            .ok()?;
        if !zdr_resp.status().is_success() {
            return None;
        }

        #[derive(serde::Deserialize)]
        struct OpenRouterZdrEndpoint {
            model_id: String,
        }

        #[derive(serde::Deserialize)]
        struct OpenRouterZdrEndpointsResponse {
            data: Vec<OpenRouterZdrEndpoint>,
        }

        let zdr_body: OpenRouterZdrEndpointsResponse = zdr_resp.json().await.ok()?;
        let set: std::collections::HashSet<String> =
            zdr_body.data.into_iter().map(|e| e.model_id).collect();
        Some(set)
    } else {
        None
    };

    let resp = client
        .get("https://openrouter.ai/api/v1/embeddings/models")
        .send()
        .await
        .ok()?;
    let body: OpenRouterModelsResponse = resp.json().await.ok()?;

    if body.data.is_empty() {
        return None;
    }

    let mut sorted_data = body.data;
    if let Some(ref zdr) = zdr_set {
        sorted_data.retain(|m| zdr.contains(&m.id));
    }
    sorted_data.sort_by(|a, b| a.id.cmp(&b.id));

    if sorted_data.is_empty() {
        return None;
    }

    let mut min_sum: Option<f64> = None;
    let mut default_model: Option<String> = None;

    for m in &sorted_data {
        let (prompt_price, completion_price) = if let Some(ref pricing) = m.pricing {
            let p = pricing
                .prompt
                .as_deref()
                .unwrap_or("0.0")
                .parse::<f64>()
                .unwrap_or(0.0);
            let c = pricing
                .completion
                .as_deref()
                .unwrap_or("0.0")
                .parse::<f64>()
                .unwrap_or(0.0);
            (p, c)
        } else {
            (0.0, 0.0)
        };
        let sum = prompt_price + completion_price;
        if min_sum.is_none() || Some(sum) < min_sum {
            min_sum = Some(sum);
            default_model = Some(m.id.clone());
        }
    }

    let model_ids: Vec<String> = sorted_data.into_iter().map(|m| m.id).collect();
    let def_model =
        default_model.unwrap_or_else(|| "nvidia/llama-nemotron-embed-vl-1b-v2:free".to_string());

    Some((model_ids, def_model))
}

/// Interactive setup wizard for Rune configuration.
pub async fn run_setup(config_path_override: Option<String>) {
    // Determine target config file path
    let target_config_path = match config_path_override {
        Some(ref p) => PathBuf::from(p),
        None => dirs_home().join(".rune").join("rune.toml"),
    };
    let target_display = target_config_path.display().to_string();

    let existing = load_existing_config(&target_config_path);
    let has_existing = existing.api_key.is_some() || existing.model.is_some();

    println!();
    println!("{}", "  ᚱ  Rune Setup Wizard".cyan().bold());
    println!("{}", "  ─────────────────────".dimmed());
    println!();
    if has_existing {
        println!(
            "  {} Existing config found. Press Enter to keep current values.",
            "ℹ".dimmed()
        );
        println!();
    }
    println!(
        "  This will create a configuration file at {}",
        target_display.green()
    );
    println!();

    // 1. Provider selection
    println!("{}", "1. Choose your LLM provider:".bold());
    println!(
        "   {} GitHub Copilot  (recommended — auto token refresh)",
        "[1]".cyan()
    );
    println!(
        "   {} Google Gemini   (generativelanguage.googleapis.com)",
        "[2]".cyan()
    );
    println!("   {} OpenAI          (api.openai.com)", "[3]".cyan());
    println!("   {} OpenRouter      (openrouter.ai)", "[4]".cyan());
    println!(
        "   {} Anthropic       (api.anthropic.com — via proxy)",
        "[5]".cyan()
    );
    println!("   {} Local/Custom    (specify URL)", "[6]".cyan());
    if let Some(ref p) = existing.provider {
        println!("   {}", format!("(current: {})", p).dimmed());
    }
    println!();

    let provider_default = match existing.provider.as_deref() {
        Some("github-copilot") => "1",
        Some("gemini") => "2",
        Some("openai") => "3",
        Some("openrouter") => "4",
        Some("anthropic") => "5",
        _ => "",
    };
    let provider_prompt = if provider_default.is_empty() {
        "  Select [1-6]: ".to_string()
    } else {
        format!("  Select [1-6] (Enter={}): ", provider_default)
    };
    let provider_input = prompt(&provider_prompt).unwrap_or_default();
    let provider_choice = if provider_input.trim().is_empty() && !provider_default.is_empty() {
        provider_default.to_string()
    } else {
        provider_input
    };
    let (base_url, provider_name, provider_id, key_hint) = match provider_choice.trim() {
        "1" => (
            None,
            "GitHub Copilot",
            "github-copilot",
            "GitHub PAT (starts with ghu_ or ghp_)",
        ),
        "2" => (
            Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            "Google Gemini",
            "gemini",
            "Gemini API key (starts with AIza)",
        ),
        "3" => (
            Some("https://api.openai.com/v1".to_string()),
            "OpenAI",
            "openai",
            "OpenAI API key (starts with sk-)",
        ),
        "4" => (
            None,
            "OpenRouter",
            "openrouter",
            "OpenRouter key (starts with sk-or-)",
        ),
        "5" => (
            Some("https://api.anthropic.com/v1".to_string()),
            "Anthropic",
            "anthropic",
            "Anthropic key",
        ),
        "6" => {
            let url = prompt("  Enter base URL: ").unwrap_or_default();
            (Some(url.trim().to_string()), "Custom", "openai", "API key")
        }
        _ => {
            println!("  {} Defaulting to GitHub Copilot", "⚠".yellow());
            (
                None,
                "GitHub Copilot",
                "github-copilot",
                "GitHub PAT (starts with ghu_ or ghp_)",
            )
        }
    };

    let default_url_display = if provider_id == "github-copilot" {
        "(auto — Copilot endpoint)"
    } else if provider_id == "openrouter" {
        "(auto — OpenRouter endpoint)"
    } else {
        "(auto)"
    };
    let base_url_display = base_url.as_deref().unwrap_or(default_url_display);
    println!(
        "  {} Selected: {} ({})",
        "✓".green(),
        provider_name,
        base_url_display.dimmed()
    );
    println!();

    // 2. API Key
    let api_key = if provider_id == "github-copilot" {
        // GitHub Copilot: offer two auth methods
        println!("{}", "2. GitHub Copilot authentication:".bold());
        if let Some(ref k) = existing.api_key {
            let chars: Vec<char> = k.chars().collect();
            let prefix_len = chars.len().min(4);
            let suffix_start = chars.len().saturating_sub(4).max(prefix_len);
            let prefix: String = chars[..prefix_len].iter().collect();
            let suffix: String = chars[suffix_start..].iter().collect();
            let masked = format!("{}...{}", prefix, suffix);
            println!("   {}", format!("(current: {})", masked).dimmed());
        }
        println!(
            "   {} Paste a GitHub token directly (ghu_ or ghp_)",
            "[1]".cyan()
        );
        println!(
            "   {} Login via GitHub Device Flow (opens browser)",
            "[2]".cyan()
        );
        if existing.api_key.is_some() {
            println!("   {} Keep current token", "[Enter]".cyan());
        }
        println!();

        let auth_prompt = if existing.api_key.is_some() {
            "  Select [1-2] (Enter=keep current): "
        } else {
            "  Select [1-2]: "
        };
        let auth_choice = prompt(auth_prompt).unwrap_or_default();
        match auth_choice.trim() {
            "1" => {
                // Direct token input
                let key_prompt = "  GitHub token: ";
                let key_input = prompt(key_prompt).unwrap_or_default().trim().to_string();
                if key_input.is_empty() {
                    existing.api_key.clone().unwrap_or_default()
                } else {
                    key_input
                }
            }
            "2" => {
                // GitHub Device Flow
                match github_device_flow().await {
                    Some(token) => token,
                    None => {
                        println!("  {} Falling back to manual token entry.", "ℹ".dimmed());
                        let key_input = prompt("  GitHub token: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if key_input.is_empty() {
                            existing.api_key.clone().unwrap_or_default()
                        } else {
                            key_input
                        }
                    }
                }
            }
            "" if existing.api_key.is_some() => {
                // Keep current
                existing.api_key.clone().unwrap_or_default()
            }
            _ => {
                // Treat as direct token input (user pasted a token instead of choosing)
                let input = auth_choice.trim().to_string();
                if input.starts_with("ghu_") || input.starts_with("ghp_") {
                    input
                } else {
                    println!(
                        "  {} Unrecognized choice, defaulting to manual entry.",
                        "⚠".yellow()
                    );
                    let key_input = prompt("  GitHub token: ")
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if key_input.is_empty() {
                        existing.api_key.clone().unwrap_or_default()
                    } else {
                        key_input
                    }
                }
            }
        }
    } else {
        // Non-Copilot providers: direct key input
        println!("{}", "2. Enter your API key:".bold());
        println!("   {}", format!("Hint: {}", key_hint).dimmed());
        if let Some(ref k) = existing.api_key {
            let chars: Vec<char> = k.chars().collect();
            let prefix_len = chars.len().min(4);
            let suffix_start = chars.len().saturating_sub(4).max(prefix_len);
            let prefix: String = chars[..prefix_len].iter().collect();
            let suffix: String = chars[suffix_start..].iter().collect();
            let masked = format!("{}...{}", prefix, suffix);
            println!("   {}", format!("(current: {})", masked).dimmed());
        }
        let key_prompt = if existing.api_key.is_some() {
            "  API key (Enter=keep current): "
        } else {
            "  API key: "
        };
        let key_input = prompt(key_prompt).unwrap_or_default().trim().to_string();
        if key_input.is_empty() {
            existing.api_key.clone().unwrap_or_default()
        } else {
            key_input
        }
    };
    if api_key.is_empty() {
        println!(
            "  {} No API key provided. You can set it later via RUNE_API_KEY.",
            "⚠".yellow()
        );
    } else {
        println!(
            "  {} API key set ({}...)",
            "✓".green(),
            crate::config::safe_truncate(&api_key, 8)
        );
    }
    println!();

    // 3. Model selection
    println!("{}", "3. Choose a model:".bold());
    if let Some(ref m) = existing.model {
        println!("   {}", format!("(current: {})", m).dimmed());
    }

    let mut openrouter_zdr = existing.openrouter_zdr.unwrap_or(false);
    if provider_choice.trim() == "4" {
        let default_zdr_str = if openrouter_zdr { "Y/n" } else { "y/N" };
        let zdr_prompt = format!(
            "  Enforce Zero Data Retention (ZDR) models only? ({}): ",
            default_zdr_str
        );
        let zdr_input = prompt(&zdr_prompt).unwrap_or_default();
        let zdr_trimmed = zdr_input.trim();
        if !zdr_trimmed.is_empty() {
            openrouter_zdr =
                zdr_trimmed.eq_ignore_ascii_case("y") || zdr_trimmed.eq_ignore_ascii_case("yes");
        }
    }

    let mut openrouter_models = None;
    let mut copilot_models = None;
    let mut gemini_models = None;
    if provider_choice.trim() == "4" {
        print!("  Fetching models from OpenRouter...");
        let _ = io::stdout().flush();
        if let Some(models) = fetch_openrouter_models(openrouter_zdr).await {
            println!(" Done.");
            openrouter_models = Some(models);
        } else {
            println!(" Failed. Using defaults.");
        }
    } else if provider_choice.trim() == "1" && !api_key.is_empty() {
        print!("  Detecting models from GitHub Copilot...");
        let _ = io::stdout().flush();
        if let Some(models) = fetch_provider_models("github-copilot", &api_key, None).await {
            println!(" Done.");
            copilot_models = Some(models);
        } else {
            println!(" Failed.");
        }
    } else if provider_choice.trim() == "2" && !api_key.is_empty() {
        print!("  Fetching models from Gemini...");
        let _ = io::stdout().flush();
        let gemini_base = base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta");
        if let Some(models) = fetch_gemini_models(&api_key, gemini_base).await {
            println!(" Done.");
            gemini_models = Some(models);
        } else {
            println!(" Failed. Using defaults.");
        }
    }

    match provider_choice.trim() {
        "1" => {
            if let Some(ref models) = copilot_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model.id);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!("   {} Custom", "[1]".cyan());
            }
        }
        "2" => {
            if let Some(ref models) = gemini_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!("   {} Custom", "[1]".cyan());
            }
        }
        "3" => {
            println!("   {} gpt-4o-mini     (fast, cheap)", "[1]".cyan());
            println!("   {} gpt-4o          (powerful)", "[2]".cyan());
            println!("   {} gpt-4-turbo     (balanced)", "[3]".cyan());
            println!("   {} Custom", "[4]".cyan());
        }
        "4" => {
            if let Some(ref models) = openrouter_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!("   {} openai/gpt-4o-mini", "[1]".cyan());
                println!("   {} anthropic/claude-3.5-sonnet", "[2]".cyan());
                println!("   {} google/gemini-pro", "[3]".cyan());
                println!("   {} Custom", "[4]".cyan());
            }
        }
        _ => {
            println!("   {} Enter model name", "[1]".cyan());
        }
    }
    println!();

    let model_prompt = if let Some(ref m) = existing.model {
        format!("  Select or type model name (Enter={}): ", m)
    } else if copilot_models.is_some() || gemini_models.is_some() {
        "  Select or type model name (Enter=1): ".to_string()
    } else {
        "  Select or type model name: ".to_string()
    };
    let model_choice = prompt(&model_prompt).unwrap_or_default();
    let model = if model_choice.trim().is_empty() && existing.model.is_some() {
        existing.model.clone().unwrap()
    } else {
        match (provider_choice.trim(), model_choice.trim()) {
            ("1", choice) => {
                if let Some(ref models) = copilot_models {
                    // Empty choice defaults to [1] (first model).
                    let effective = if choice.is_empty() { "1" } else { choice };
                    if let Ok(idx) = effective.parse::<usize>() {
                        if idx > 0 && idx <= models.len() {
                            models[idx - 1].id.clone()
                        } else if idx == models.len() + 1 {
                            prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string()
                        } else {
                            effective.to_string()
                        }
                    } else {
                        effective.to_string()
                    }
                } else {
                    // Auto-detect failed: only Custom is offered.
                    let custom = if choice.is_empty() || choice == "1" {
                        prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    } else {
                        choice.to_string()
                    };
                    custom
                }
            }
            ("2", choice) => {
                if let Some(ref models) = gemini_models {
                    let effective = if choice.is_empty() { "1" } else { choice };
                    if let Ok(idx) = effective.parse::<usize>() {
                        if idx > 0 && idx <= models.len() {
                            models[idx - 1].clone()
                        } else if idx == models.len() + 1 {
                            prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string()
                        } else {
                            effective.to_string()
                        }
                    } else {
                        effective.to_string()
                    }
                } else {
                    // fetch failed — only Custom was shown
                    if choice.is_empty() || choice == "1" {
                        prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    } else {
                        choice.to_string()
                    }
                }
            }
            ("3", "1") => "gpt-4o-mini".to_string(),
            ("3", "2") => "gpt-4o".to_string(),
            ("3", "3") => "gpt-4-turbo".to_string(),
            ("4", choice) => {
                if let Some(ref models) = openrouter_models {
                    if choice.is_empty() {
                        let custom = prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if custom.is_empty() {
                            "".to_string()
                        } else {
                            custom
                        }
                    } else if let Ok(idx) = choice.parse::<usize>() {
                        if idx > 0 && idx <= models.len() {
                            models[idx - 1].clone()
                        } else if idx == models.len() + 1 {
                            let custom = prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string();
                            if custom.is_empty() {
                                "".to_string()
                            } else {
                                custom
                            }
                        } else {
                            choice.to_string()
                        }
                    } else {
                        choice.to_string()
                    }
                } else {
                    match choice {
                        "1" => "openai/gpt-4o-mini".to_string(),
                        "2" => "anthropic/claude-3.5-sonnet".to_string(),
                        "3" => "google/gemini-pro".to_string(),
                        _ => {
                            let custom = prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string();
                            if custom.is_empty() {
                                "".to_string()
                            } else {
                                custom
                            }
                        }
                    }
                }
            }
            (_, choice) if !choice.is_empty() && !["3", "4"].contains(&choice) => {
                choice.to_string()
            }
            _ => {
                let custom = prompt("  Model name: ")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if custom.is_empty() {
                    "gpt-4o".to_string()
                } else {
                    custom
                }
            }
        }
    };
    if model.is_empty() {
        println!("  {} Model: not set", "✓".green());
    } else {
        println!("  {} Model: {}", "✓".green(), model.green());
    }
    println!();

    // Look up dynamic ModelInfo for the selected model (Copilot only).
    let selected_model_info = copilot_models
        .as_ref()
        .and_then(|models| models.iter().find(|m| m.id == model));

    // 4. Thinking (reasoning effort)
    //   - Copilot model with no reasoning_efforts → skip, force "off" (overrides existing config)
    //   - Copilot model with reasoning_efforts     → show step with those levels
    //   - No dynamic info (non-Copilot / custom)   → show standard levels
    let thinking = if let Some(info) = selected_model_info {
        if info.reasoning_efforts.is_empty() {
            println!(
                "  {} Thinking: off (model does not support reasoning)",
                "✓".green()
            );
            println!();
            "off".to_string()
        } else {
            println!("{}", "4. Thinking (reasoning effort):".bold());
            for (i, level) in info.reasoning_efforts.iter().enumerate() {
                println!("   {} {}", format!("[{}]", i + 1).cyan(), level);
            }
            if let Some(ref t) = existing.thinking {
                println!("   {}", format!("(current: {})", t).dimmed());
            }
            println!();
            let thinking_default = existing.thinking.as_deref().unwrap_or("off");
            let thinking_prompt = format!("  Select or type level (Enter={}): ", thinking_default);
            let thinking_input = prompt(&thinking_prompt).unwrap_or_default();
            let chosen = if thinking_input.trim().is_empty() {
                thinking_default.to_string()
            } else if let Ok(idx) = thinking_input.trim().parse::<usize>() {
                if idx > 0 && idx <= info.reasoning_efforts.len() {
                    info.reasoning_efforts[idx - 1].clone()
                } else {
                    thinking_input.trim().to_string()
                }
            } else {
                thinking_input.trim().to_string()
            };
            println!("  {} Thinking: {}", "✓".green(), chosen.cyan());
            println!();
            chosen
        }
    } else {
        println!("{}", "4. Thinking (reasoning effort):".bold());
        println!("   {} off     — no extended reasoning", "[1]".cyan());
        println!("   {} low     — minimal reasoning", "[2]".cyan());
        println!("   {} medium  — balanced", "[3]".cyan());
        println!("   {} high    — deep reasoning", "[4]".cyan());
        println!("   {} xhigh   — maximum reasoning effort", "[5]".cyan());
        if let Some(ref t) = existing.thinking {
            println!("   {}", format!("(current: {})", t).dimmed());
        }
        println!();
        let thinking_default = existing.thinking.as_deref().unwrap_or("off");
        let thinking_prompt = format!(
            "  Select [1-5] or type level (Enter={}): ",
            thinking_default
        );
        let thinking_input = prompt(&thinking_prompt).unwrap_or_default();
        let chosen = if thinking_input.trim().is_empty() {
            thinking_default.to_string()
        } else {
            match thinking_input.trim() {
                "1" | "off" | "none" => "off".to_string(),
                "2" | "low" => "low".to_string(),
                "3" | "medium" => "medium".to_string(),
                "4" | "high" => "high".to_string(),
                "5" | "xhigh" => "xhigh".to_string(),
                other => other.to_string(),
            }
        };
        println!("  {} Thinking: {}", "✓".green(), chosen.cyan());
        println!();
        chosen
    };

    // 5. Skills directory
    println!("{}", "5. Skills directory:".bold());
    let skills_default = existing.skills_dir.as_deref().unwrap_or("./skills");
    let skills_input = prompt(&format!("  Path [{}]: ", skills_default)).unwrap_or_default();
    let skills_dir = if skills_input.trim().is_empty() {
        skills_default.to_string()
    } else {
        skills_input.trim().to_string()
    };
    println!("  {} Skills dir: {}", "✓".green(), skills_dir);
    println!();

    // 6. Enable semantic features (embedding)?
    println!("{}", "6. Enable semantic features (embedding)?".bold());
    println!("   Embedding enables:");
    println!("   • Automatic skill matching (no @name needed)");
    println!("   • Smart context compaction (keeps relevant history)");
    println!();
    let emb_default = existing.embedding_enabled.unwrap_or(true);
    let emb_prompt = if emb_default {
        "  Enable embedding? [Y/n]: "
    } else {
        "  Enable embedding? [y/N]: "
    };
    let emb_choice = prompt(emb_prompt).unwrap_or_default();
    let embedding_enabled = if emb_choice.trim().is_empty() {
        emb_default
    } else {
        !emb_choice.trim().eq_ignore_ascii_case("n")
    };
    if embedding_enabled {
        println!("  {} Embedding enabled", "✓".green());
    } else {
        println!(
            "  {} Embedding disabled (can enable later in rune.toml)",
            "ℹ".dimmed()
        );
    }

    // 6b. Embedding model (only if enabled)
    let embedding_model = if embedding_enabled {
        let mut openrouter_emb_models = None;
        if provider_choice.trim() == "4" {
            print!("  Fetching embedding models from OpenRouter...");
            let _ = io::stdout().flush();
            if let Some(res) = fetch_openrouter_embedding_models(openrouter_zdr).await {
                println!(" Done.");
                openrouter_emb_models = Some(res);
            } else {
                println!(" Failed. Using defaults.");
            }
        }

        let default_emb_model = match provider_choice.trim() {
            "1" => "text-embedding-3-small".to_string(),
            "2" => "gemini-embedding-2".to_string(),
            "4" => {
                if let Some((_, ref def)) = openrouter_emb_models {
                    def.clone()
                } else {
                    "nvidia/llama-nemotron-embed-vl-1b-v2:free".to_string()
                }
            }
            _ => "text-embedding-3-small".to_string(),
        };
        // Check existing config for model
        let current_emb_model = existing.embedding_section.as_ref().and_then(|s| {
            s.lines()
                .find(|l| l.trim().starts_with("model"))
                .and_then(|l| l.split('"').nth(1))
                .map(|s| s.to_string())
        });
        let emb_model_default = current_emb_model.as_deref().unwrap_or(&default_emb_model);
        println!();
        println!("   {}", "Embedding model:".bold());

        if provider_choice.trim() == "4" {
            if let Some((ref models, _)) = openrouter_emb_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!(
                    "   {} nvidia/llama-nemotron-embed-vl-1b-v2:free",
                    "[1]".cyan()
                );
                println!("   {} Custom", "[2]".cyan());
            }
        }

        let emb_model_prompt = format!(
            "  Select or type model name (Enter={}): ",
            emb_model_default
        );
        let emb_model_input = prompt(&emb_model_prompt).unwrap_or_default();
        let model = if emb_model_input.trim().is_empty() {
            emb_model_default.to_string()
        } else {
            let choice = emb_model_input.trim();
            if provider_choice.trim() == "4" {
                if let Some((ref models, _)) = openrouter_emb_models {
                    if let Ok(idx) = choice.parse::<usize>() {
                        if idx > 0 && idx <= models.len() {
                            models[idx - 1].clone()
                        } else if idx == models.len() + 1 {
                            let custom = prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string();
                            if custom.is_empty() {
                                "".to_string()
                            } else {
                                custom
                            }
                        } else {
                            choice.to_string()
                        }
                    } else {
                        choice.to_string()
                    }
                } else {
                    match choice {
                        "1" => "nvidia/llama-nemotron-embed-vl-1b-v2:free".to_string(),
                        _ => {
                            let custom = prompt("  Model name: ")
                                .unwrap_or_default()
                                .trim()
                                .to_string();
                            if custom.is_empty() {
                                "".to_string()
                            } else {
                                custom
                            }
                        }
                    }
                }
            } else {
                choice.to_string()
            }
        };

        if model.is_empty() {
            println!(
                "  {} Embedding model: not set (embedding disabled)",
                "✓".green()
            );
            None
        } else {
            println!("  {} Embedding model: {}", "✓".green(), model.cyan());
            Some(model)
        }
    } else {
        None
    };
    println!();

    // 6c. Enable notes?
    println!("{}", "6c. Enable Notes (serve mode)?".bold());
    let notes_default = existing.notes_section.is_some();
    let notes_prompt = if notes_default {
        "  Enable notes? [Y/n]: "
    } else {
        "  Enable notes? [y/N]: "
    };
    let notes_choice = prompt(notes_prompt).unwrap_or_default();
    let enable_notes = if notes_choice.trim().is_empty() {
        notes_default
    } else {
        !notes_choice.trim().eq_ignore_ascii_case("n")
    };
    println!();

    let mut notes_port = 9527;
    let mut notes_bind = "127.0.0.1".to_string();
    let mut notes_github_client_id = "".to_string();
    let mut notes_github_client_secret = "".to_string();
    let mut notes_github_admins = "".to_string();
    let mut notes_github_users = "".to_string();
    let mut notes_github_guests = "".to_string();
    let mut notes_local_admins = "".to_string();
    let mut notes_local_users = "".to_string();
    let mut notes_local_guests = "".to_string();
    let mut notes_model = "".to_string();
    let mut enable_github = false;
    let mut enable_local = false;

    if enable_notes {
        println!("  Enter config values for [notes] section:");
        println!();

        let port_default = existing.notes_port.unwrap_or(9527);
        let port_prompt = format!("  Port [{}]: ", port_default);
        let port_input = prompt(&port_prompt).unwrap_or_default();
        notes_port = if port_input.trim().is_empty() {
            port_default
        } else {
            port_input.trim().parse().unwrap_or(9527)
        };

        let bind_default = existing.notes_bind.as_deref().unwrap_or("127.0.0.1");
        let bind_prompt = format!("  Bind address [{}]: ", bind_default);
        let bind_input = prompt(&bind_prompt).unwrap_or_default();
        notes_bind = if bind_input.trim().is_empty() {
            bind_default.to_string()
        } else {
            bind_input.trim().to_string()
        };

        println!("  Select authentication methods for Notes:");
        println!("   {} GitHub OAuth 2.0", "[1]".cyan());
        println!(
            "   {} Local Static Credentials (username:password)",
            "[2]".cyan()
        );
        println!("   {} Both", "[3]".cyan());
        println!();
        let auth_default =
            if existing.notes_github_client_id.is_some() && existing.notes_local_admins.is_some() {
                "3"
            } else if existing.notes_local_admins.is_some() {
                "2"
            } else {
                "1"
            };
        let auth_prompt = format!("  Authentication method [{}]: ", auth_default);
        let auth_choice = prompt(&auth_prompt).unwrap_or_default();
        let auth_choice = if auth_choice.trim().is_empty() {
            auth_default
        } else {
            auth_choice.trim()
        };
        enable_github = auth_choice == "1" || auth_choice == "3";
        enable_local = auth_choice == "2" || auth_choice == "3";

        if enable_github {
            println!();
            println!("  Configure GitHub OAuth 2.0 (create an OAuth App at https://github.com/settings/developers):");
            let client_id_default = existing.notes_github_client_id.as_deref().unwrap_or("");
            let client_id_prompt = if client_id_default.is_empty() {
                "   GitHub OAuth Client ID: ".to_string()
            } else {
                format!("   GitHub OAuth Client ID [{}]: ", client_id_default)
            };
            let client_id_input = prompt(&client_id_prompt).unwrap_or_default();
            notes_github_client_id = if client_id_input.trim().is_empty() {
                client_id_default.to_string()
            } else {
                client_id_input.trim().to_string()
            };

            let client_secret_default =
                existing.notes_github_client_secret.as_deref().unwrap_or("");
            let secret_prompt = if client_secret_default.is_empty() {
                "   GitHub OAuth Client Secret: ".to_string()
            } else {
                format!(
                    "   GitHub OAuth Client Secret [{}]: ",
                    client_secret_default
                )
            };
            let secret_input = prompt(&secret_prompt).unwrap_or_default();
            notes_github_client_secret = if secret_input.trim().is_empty() {
                client_secret_default.to_string()
            } else {
                secret_input.trim().to_string()
            };

            println!(
                "   {} Enter GitHub logins or \"org:org/team\" for each role (comma-separated):",
                "ℹ".dimmed()
            );
            let admins_default = existing.notes_github_admins.as_deref().unwrap_or("");
            let admins_prompt = format!("   Admins [{}]: ", admins_default);
            let admins_input = prompt(&admins_prompt).unwrap_or_default();
            notes_github_admins = if admins_input.trim().is_empty() {
                admins_default.to_string()
            } else {
                admins_input.trim().to_string()
            };

            let users_default = existing.notes_github_users.as_deref().unwrap_or("");
            let users_prompt = format!("   Users [{}]: ", users_default);
            let users_input = prompt(&users_prompt).unwrap_or_default();
            notes_github_users = if users_input.trim().is_empty() {
                users_default.to_string()
            } else {
                users_input.trim().to_string()
            };

            let guests_default = existing.notes_github_guests.as_deref().unwrap_or("");
            let guests_prompt = format!("   Guests [{}]: ", guests_default);
            let guests_input = prompt(&guests_prompt).unwrap_or_default();
            notes_github_guests = if guests_input.trim().is_empty() {
                guests_default.to_string()
            } else {
                guests_input.trim().to_string()
            };
        }

        if enable_local {
            println!();
            println!("  Configure Local Static Credentials:");
            println!(
                "   {} Enter local credentials (format: \"username:password\", comma-separated):",
                "ℹ".dimmed()
            );
            let local_admins_default = existing.notes_local_admins.as_deref().unwrap_or("");
            let local_admins_prompt = format!("   Admins [{}]: ", local_admins_default);
            let local_admins_input = prompt(&local_admins_prompt).unwrap_or_default();
            notes_local_admins = if local_admins_input.trim().is_empty() {
                local_admins_default.to_string()
            } else {
                local_admins_input.trim().to_string()
            };

            let local_users_default = existing.notes_local_users.as_deref().unwrap_or("");
            let local_users_prompt = format!("   Users [{}]: ", local_users_default);
            let local_users_input = prompt(&local_users_prompt).unwrap_or_default();
            notes_local_users = if local_users_input.trim().is_empty() {
                local_users_default.to_string()
            } else {
                local_users_input.trim().to_string()
            };

            let local_guests_default = existing.notes_local_guests.as_deref().unwrap_or("");
            let local_guests_prompt = format!("   Guests [{}]: ", local_guests_default);
            let local_guests_input = prompt(&local_guests_prompt).unwrap_or_default();
            notes_local_guests = if local_guests_input.trim().is_empty() {
                local_guests_default.to_string()
            } else {
                local_guests_input.trim().to_string()
            };
        }

        let model_default = existing.notes_model.as_deref().unwrap_or("");
        if provider_choice.trim() == "4" {
            println!("  Select a model for notes:");
            if let Some(ref models) = openrouter_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!("   {} openai/gpt-4o-mini", "[1]".cyan());
                println!("   {} anthropic/claude-3.5-sonnet", "[2]".cyan());
                println!("   {} google/gemini-pro", "[3]".cyan());
                println!("   {} Custom", "[4]".cyan());
            }
            println!();
        } else if provider_choice.trim() == "2" {
            println!("  Select a model for notes:");
            if let Some(ref models) = gemini_models {
                for (i, model) in models.iter().enumerate() {
                    println!("   {} {}", format!("[{}]", i + 1).cyan(), model);
                }
                println!("   {} Custom", format!("[{}]", models.len() + 1).cyan());
            } else {
                println!("   {} Custom", "[1]".cyan());
            }
            println!();
        }

        let model_prompt = if model_default.is_empty() {
            "  Model for notes (Enter for auto-detect): ".to_string()
        } else {
            format!("  Model for notes (Enter={}): ", model_default)
        };
        let model_input = prompt(&model_prompt).unwrap_or_default();
        let notes_model_choice = model_input.trim();

        notes_model = if notes_model_choice.is_empty() && !model_default.is_empty() {
            model_default.to_string()
        } else if provider_choice.trim() == "4" {
            if let Some(ref models) = openrouter_models {
                if notes_model_choice.is_empty() {
                    "".to_string()
                } else if let Ok(idx) = notes_model_choice.parse::<usize>() {
                    if idx > 0 && idx <= models.len() {
                        models[idx - 1].clone()
                    } else if idx == models.len() + 1 {
                        let custom = prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        custom
                    } else {
                        notes_model_choice.to_string()
                    }
                } else {
                    notes_model_choice.to_string()
                }
            } else {
                match notes_model_choice {
                    "1" => "openai/gpt-4o-mini".to_string(),
                    "2" => "anthropic/claude-3.5-sonnet".to_string(),
                    "3" => "google/gemini-pro".to_string(),
                    "" => "".to_string(),
                    _ => {
                        let custom = prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        custom
                    }
                }
            }
        } else if provider_choice.trim() == "2" {
            if let Some(ref models) = gemini_models {
                if notes_model_choice.is_empty() {
                    "".to_string()
                } else if let Ok(idx) = notes_model_choice.parse::<usize>() {
                    if idx > 0 && idx <= models.len() {
                        models[idx - 1].clone()
                    } else if idx == models.len() + 1 {
                        let custom = prompt("  Model name: ")
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        custom
                    } else {
                        notes_model_choice.to_string()
                    }
                } else {
                    notes_model_choice.to_string()
                }
            } else {
                // fetch failed — only Custom was shown
                if notes_model_choice.is_empty() || notes_model_choice == "1" {
                    let custom = prompt("  Model name: ")
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    custom
                } else {
                    notes_model_choice.to_string()
                }
            }
        } else {
            if notes_model_choice.is_empty() {
                "".to_string()
            } else {
                notes_model_choice.to_string()
            }
        };
        println!();
    }

    // 7. Build config — preserve existing [policy] and [embedding] sections
    let config_path = target_config_path.clone();
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut toml_content = String::new();
    if !model.is_empty() {
        toml_content.push_str(&format!("model = \"{}\"\n", model));
    }
    toml_content.push_str(&format!("provider = \"{}\"\n", provider_id));
    if provider_id == "openrouter" {
        toml_content.push_str(&format!("openrouter_zdr = {}\n", openrouter_zdr));
    }
    if !api_key.is_empty() {
        toml_content.push_str(&format!("api_key = \"{}\"\n", api_key));
    }
    if let Some(ref url) = base_url {
        toml_content.push_str(&format!("base_url = \"{}\"\n", url));
    }
    toml_content.push_str(&format!("skills_dir = \"{}\"\n", skills_dir));
    toml_content.push_str("log_level = \"warn\"\n");
    if thinking != "off" && thinking != "none" {
        toml_content.push_str(&format!("thinking = \"{}\"\n", thinking));
    }
    toml_content.push('\n');

    // Preserve existing [policy] section (with all its accumulated allowlists)
    if let Some(ref policy) = existing.policy_section {
        toml_content.push_str(policy);
    } else {
        // Fresh install defaults
        toml_content.push_str("[policy]\n");
        toml_content.push_str("mode = \"confirm\"\n");
        toml_content.push_str("allowed_commands = [\"ls\", \"cat\", \"head\", \"ps\", \"echo\", \"uname\", \"free\", \"df\", \"date\", \"hostname\"]\n");
        toml_content.push_str("allowed_domains = []\n");
    }

    // Embedding section
    if embedding_enabled && embedding_model.is_some() {
        let emb_model_str = embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");
        if let Some(ref emb) = existing.embedding_section {
            // Update model in existing section to what user chose
            if !toml_content.ends_with('\n') {
                toml_content.push('\n');
            }
            let updated = update_toml_field(emb, "model", emb_model_str);
            toml_content.push_str(&updated);
        } else {
            toml_content.push_str("\n[embedding]\n");
            toml_content.push_str("enabled = true\n");
            toml_content.push_str(&format!("model = \"{}\"\n", emb_model_str));
            toml_content.push_str("threshold = 0.3\n");
        }
    }

    // Notes section
    if enable_notes {
        if !toml_content.ends_with("\n\n") {
            if toml_content.ends_with('\n') {
                toml_content.push('\n');
            } else {
                toml_content.push_str("\n\n");
            }
        }
        toml_content.push_str("[notes]\n");
        toml_content.push_str(&format!("port = {}\n", notes_port));
        toml_content.push_str(&format!("bind = \"{}\"\n", notes_bind));
        if !notes_model.is_empty() {
            toml_content.push_str(&format!("model = \"{}\"\n", notes_model));
        }
        // Convert comma-separated logins to TOML array
        fn to_toml_array(s: &str) -> String {
            if s.trim().is_empty() {
                return "[]".to_string();
            }
            let items: Vec<String> = s
                .split(',')
                .map(|x| format!("\"{}\"", x.trim()))
                .filter(|x| x != "\"\"")
                .collect();
            format!("[{}]", items.join(", "))
        }

        if enable_github {
            toml_content.push_str("\n[notes.github]\n");
            toml_content.push_str(&format!("client_id = \"{}\"\n", notes_github_client_id));
            toml_content.push_str(&format!(
                "client_secret = \"{}\"\n",
                notes_github_client_secret
            ));
            toml_content.push_str(&format!(
                "admins = {}\n",
                to_toml_array(&notes_github_admins)
            ));
            toml_content.push_str(&format!("users = {}\n", to_toml_array(&notes_github_users)));
            toml_content.push_str(&format!(
                "guests = {}\n",
                to_toml_array(&notes_github_guests)
            ));
        }

        if enable_local {
            toml_content.push_str("\n[notes.local]\n");
            toml_content.push_str(&format!(
                "admins = {}\n",
                to_toml_array(&notes_local_admins)
            ));
            toml_content.push_str(&format!("users = {}\n", to_toml_array(&notes_local_users)));
            toml_content.push_str(&format!(
                "guests = {}\n",
                to_toml_array(&notes_local_guests)
            ));
        }
    }

    // Show preview
    println!("{}", "─".repeat(50).dimmed());
    println!("{}", "  Configuration preview:".bold());
    println!("{}", "─".repeat(50).dimmed());
    for line in toml_content.lines() {
        println!("  {}", line.dimmed());
    }
    println!("{}", "─".repeat(50).dimmed());
    println!();

    let confirm = prompt(&format!("  Write to {}? [Y/n]: ", target_display)).unwrap_or_default();
    if confirm.trim().to_lowercase() == "n" {
        println!("  {} Setup cancelled.", "✗".red());
        return;
    }

    // Create directory and write
    if let Err(e) = std::fs::create_dir_all(&config_dir) {
        eprintln!(
            "  {} Failed to create {}: {}",
            "✗".red(),
            config_dir.display(),
            e
        );
        return;
    }
    if let Err(e) = std::fs::write(&config_path, &toml_content) {
        eprintln!(
            "  {} Failed to write {}: {}",
            "✗".red(),
            config_path.display(),
            e
        );
        return;
    }

    println!();
    println!(
        "  {} Configuration saved to {}",
        "✓".green().bold(),
        config_path.display().to_string().green()
    );
    println!();
    println!(
        "  {} Run {} to start using Rune!",
        "🎉",
        "rune".cyan().bold()
    );
    println!();
}

/// Update or insert a key=value field in a raw TOML section string.
fn update_toml_field(section: &str, key: &str, value: &str) -> String {
    let mut result = String::new();
    let mut found = false;
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("{}\x20", key)) || trimmed.starts_with(&format!("{}=", key))
        {
            result.push_str(&format!("{} = \"{}\"\n", key, value));
            found = true;
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    if !found {
        // Insert after section header
        result.push_str(&format!("{} = \"{}\"\n", key, value));
    }
    result
}

/// Read a line from stdin with a prompt.
fn prompt(msg: &str) -> Option<String> {
    print!("{}", msg);
    io::stdout().flush().ok()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok()?;
    Some(input)
}

/// Get home directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_toml_section_policy() {
        let content = r#"model = "gpt-4o"
api_key = "test"

[policy]
mode = "confirm"
allowed_commands = ["ls", "cat", "curl", "git"]
allowed_domains = ["wttr.in", "api.github.com"]
allowed_syscalls = ["clock_gettime"]

[embedding]
enabled = true
"#;
        let section = extract_toml_section(content, "[policy]").unwrap();
        assert!(section.contains("[policy]"));
        assert!(section.contains("allowed_commands"));
        assert!(section.contains("curl"));
        assert!(section.contains("git"));
        assert!(section.contains("wttr.in"));
        assert!(section.contains("api.github.com"));
        assert!(section.contains("allowed_syscalls"));
        // Should NOT contain embedding section
        assert!(!section.contains("[embedding]"));
    }

    #[test]
    fn test_extract_toml_section_embedding() {
        let content = r#"model = "gpt-4o"

[policy]
mode = "confirm"

[embedding]
enabled = true
model = "text-embedding-3-small"
threshold = 0.7
api_key = "sk-custom-embed-key"
"#;
        let section = extract_toml_section(content, "[embedding]").unwrap();
        assert!(section.contains("[embedding]"));
        assert!(section.contains("enabled = true"));
        assert!(section.contains("threshold = 0.7"));
        assert!(section.contains("sk-custom-embed-key"));
        assert!(!section.contains("[policy]"));
    }

    #[test]
    fn test_extract_toml_section_notes() {
        let content = r#"model = "gpt-4o"

[notes]
port = 9527
bind = "127.0.0.1"
admin_token = "adminCHANGE_ME"
user_token = "userCHANGE_ME"
guest_token = "guestCHANGE_ME"
model = "google/gemini-2.5-pro"
"#;
        let section = extract_toml_section(content, "[notes]").unwrap();
        assert!(section.contains("[notes]"));
        assert!(section.contains("port = 9527"));
        assert!(section.contains("bind = \"127.0.0.1\""));
        assert!(section.contains("model = \"google/gemini-2.5-pro\""));
    }

    #[test]
    fn test_extract_toml_section_missing() {
        let content = "model = \"gpt-4o\"\napi_key = \"test\"\n";
        assert!(extract_toml_section(content, "[policy]").is_none());
        assert!(extract_toml_section(content, "[embedding]").is_none());
        assert!(extract_toml_section(content, "[notes]").is_none());
    }

    #[test]
    fn test_extract_toml_section_preserves_accumulated_values() {
        // Simulate a config that has been modified by /add-rw-dir and domain allows
        let content = r#"model = "gpt-4o"
api_key = "ghu_test123"
skills_dir = "./skills"

[policy]
mode = "confirm"
allowed_commands = ["ls", "cat", "head", "ps", "echo", "uname", "free", "df", "date", "hostname", "git", "cargo", "rustc", "make"]
allowed_domains = ["wttr.in", "api.github.com", "crates.io", "registry.npmjs.org"]
allowed_paths_rw = ["/home/u/project", "/tmp"]
allowed_paths_ro = ["/home/u/project", "/usr/share/doc"]
denied_paths = ["/root", "/etc/shadow"]
"#;
        let section = extract_toml_section(content, "[policy]").unwrap();
        // All accumulated values must be preserved
        assert!(section.contains("git"));
        assert!(section.contains("cargo"));
        assert!(section.contains("rustc"));
        assert!(section.contains("crates.io"));
        assert!(section.contains("registry.npmjs.org"));
        assert!(section.contains("allowed_paths_rw"));
        assert!(section.contains("/home/u/project"));
        assert!(section.contains("denied_paths"));
    }

    #[test]
    fn test_extract_toml_section_at_end_of_file() {
        // Section is the last thing in the file (no trailing section header)
        let content = r#"model = "gpt-4o"

[policy]
mode = "allowlist"
allowed_commands = ["curl"]
allowed_domains = ["example.com"]"#;
        let section = extract_toml_section(content, "[policy]").unwrap();
        assert!(section.contains("mode = \"allowlist\""));
        assert!(section.contains("curl"));
        assert!(section.contains("example.com"));
    }

    #[test]
    fn test_update_toml_field_existing() {
        let section =
            "[embedding]\nenabled = true\nmodel = \"text-embedding-3-small\"\nthreshold = 0.3\n";
        let updated = update_toml_field(section, "model", "gemini-embedding-2");
        assert!(updated.contains("model = \"gemini-embedding-2\""));
        assert!(!updated.contains("text-embedding-3-small"));
        assert!(updated.contains("enabled = true"));
        assert!(updated.contains("threshold = 0.3"));
    }

    #[test]
    fn test_update_toml_field_missing() {
        let section = "[embedding]\nenabled = true\nthreshold = 0.3\n";
        let updated = update_toml_field(section, "model", "gemini-embedding-2");
        assert!(updated.contains("model = \"gemini-embedding-2\""));
        assert!(updated.contains("enabled = true"));
    }

    #[test]
    fn test_rune_init_openrouter_embedding_default() {
        // When provider is openrouter (choice "4"), default embedding model should be nvidia
        let provider_choice = "4";
        let default_emb_model = match provider_choice.trim() {
            "1" => "text-embedding-3-small",
            "2" => "gemini-embedding-2",
            "4" => "nvidia/llama-nemotron-embed-vl-1b-v2:free",
            _ => "text-embedding-3-small",
        };
        assert_eq!(
            default_emb_model,
            "nvidia/llama-nemotron-embed-vl-1b-v2:free"
        );
    }

    #[test]
    fn test_rune_init_copilot_embedding_default() {
        let provider_choice = "1";
        let default_emb_model = match provider_choice.trim() {
            "1" => "text-embedding-3-small",
            "2" => "gemini-embedding-2",
            "4" => "nvidia/llama-nemotron-embed-vl-1b-v2:free",
            _ => "text-embedding-3-small",
        };
        assert_eq!(default_emb_model, "text-embedding-3-small");
    }

    #[test]
    fn test_rune_init_gemini_embedding_default() {
        let provider_choice = "2";
        let default_emb_model = match provider_choice.trim() {
            "1" => "text-embedding-3-small",
            "2" => "gemini-embedding-2",
            "4" => "nvidia/llama-nemotron-embed-vl-1b-v2:free",
            _ => "text-embedding-3-small",
        };
        assert_eq!(default_emb_model, "gemini-embedding-2");
    }

    // ── update_toml_field edge cases ─────────────────────────────────────

    #[test]
    fn test_update_toml_field_no_space_around_eq() {
        let section = "[embedding]\nenabled = true\nmodel=\"old-model\"\n";
        let updated = update_toml_field(section, "model", "new-model");
        assert!(updated.contains("model = \"new-model\""));
        assert!(!updated.contains("old-model"));
    }

    #[test]
    fn test_update_toml_field_preserves_other_keys() {
        let section = "[embedding]\nenabled = true\nmodel = \"old\"\nthreshold = 0.5\n";
        let updated = update_toml_field(section, "model", "new");
        assert!(updated.contains("enabled = true"));
        assert!(updated.contains("threshold = 0.5"));
        assert!(updated.contains("model = \"new\""));
        assert!(!updated.contains("model = \"old\""));
    }

    #[test]
    fn test_update_toml_field_insert_when_missing_key() {
        let section = "[embedding]\nenabled = true\n";
        let updated = update_toml_field(section, "threshold", "0.9");
        assert!(updated.contains("threshold = \"0.9\""));
        assert!(updated.contains("enabled = true"));
    }

    // ── extract_toml_section edge cases ─────────────────────────────────

    #[test]
    fn test_extract_toml_section_empty_string() {
        let result = extract_toml_section("", "[policy]");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_toml_section_multiple_sections() {
        let content = "[a]\nkey_a = 1\n[b]\nkey_b = 2\n[c]\nkey_c = 3\n";
        let section_b = extract_toml_section(content, "[b]").unwrap();
        assert!(section_b.contains("key_b"));
        assert!(!section_b.contains("key_a"));
        assert!(!section_b.contains("key_c"));
    }

    // ── Model selection ───────────────────────────────────────────────────

    #[test]
    fn test_model_copilot_gpt4o_mini() {
        let m = match ("1", "2") {
            ("1", "1") => "gpt-4o",
            ("1", "2") => "gpt-4o-mini",
            ("1", "3") => "claude-3.5-sonnet",
            _ => "unknown",
        };
        assert_eq!(m, "gpt-4o-mini");
    }

    #[test]
    fn test_model_gemini_flash() {
        let m = match ("2", "1") {
            ("2", "1") => "gemini-2.0-flash",
            ("2", "2") => "gemini-1.5-pro",
            _ => "unknown",
        };
        assert_eq!(m, "gemini-2.0-flash");
    }

    #[test]
    fn test_model_openrouter_claude() {
        let m = match ("4", "2") {
            ("4", "1") => "openai/gpt-4o-mini",
            ("4", "2") => "anthropic/claude-3.5-sonnet",
            ("4", "3") => "google/gemini-pro",
            _ => "unknown",
        };
        assert_eq!(m, "anthropic/claude-3.5-sonnet");
    }

    #[test]
    fn test_model_openai_provider_choices() {
        assert_eq!(
            match ("3", "1") {
                ("3", "1") => "gpt-4o-mini",
                _ => "x",
            },
            "gpt-4o-mini"
        );
        assert_eq!(
            match ("3", "2") {
                ("3", "2") => "gpt-4o",
                _ => "x",
            },
            "gpt-4o"
        );
        assert_eq!(
            match ("3", "3") {
                ("3", "3") => "gpt-4-turbo",
                _ => "x",
            },
            "gpt-4-turbo"
        );
    }

    // ── Thinking level mapping ────────────────────────────────────────────

    #[test]
    fn test_thinking_off_variants() {
        for input in &["1", "off", "none"] {
            let result = match *input {
                "1" | "off" | "none" => "off",
                "2" | "low" => "low",
                "3" | "medium" => "medium",
                "4" | "high" => "high",
                "5" | "xhigh" => "xhigh",
                other => other,
            };
            assert_eq!(result, "off", "input: {}", input);
        }
    }

    #[test]
    fn test_thinking_high_and_xhigh() {
        let high = match "4" {
            "1" | "off" | "none" => "off",
            "4" | "high" => "high",
            other => other,
        };
        let xhigh = match "5" {
            "5" | "xhigh" => "xhigh",
            other => other,
        };
        assert_eq!(high, "high");
        assert_eq!(xhigh, "xhigh");
    }

    // ── TOML content logic ────────────────────────────────────────────────

    #[test]
    fn test_thinking_not_written_when_off() {
        let thinking = "off";
        let mut c = String::new();
        if thinking != "off" && thinking != "none" {
            c.push_str("thinking\n");
        }
        assert!(!c.contains("thinking"));
    }

    #[test]
    fn test_thinking_written_when_high() {
        let thinking = "high";
        let mut c = String::new();
        if thinking != "off" && thinking != "none" {
            c.push_str(&format!("thinking = \"{}\"\n", thinking));
        }
        assert!(c.contains("thinking = \"high\""));
    }

    #[test]
    fn test_model_not_written_when_empty() {
        let model = "";
        let mut c = String::new();
        if !model.is_empty() {
            c.push_str(&format!("model = \"{}\"\n", model));
        }
        assert!(!c.contains("model"));
    }

    #[test]
    fn test_model_written_when_set() {
        let model = "openai/gpt-4o-mini";
        let mut c = String::new();
        if !model.is_empty() {
            c.push_str(&format!("model = \"{}\"\n", model));
        }
        assert!(c.contains("model = \"openai/gpt-4o-mini\""));
    }

    #[test]
    fn test_embedding_section_not_written_when_model_is_none() {
        let embedding_enabled = true;
        let embedding_model: Option<String> = None;
        let mut toml_content = String::new();
        if embedding_enabled && embedding_model.is_some() {
            let emb_model_str = embedding_model.as_deref().unwrap();
            toml_content.push_str("\n[embedding]\n");
            toml_content.push_str("enabled = true\n");
            toml_content.push_str(&format!("model = \"{}\"\n", emb_model_str));
        }
        assert!(!toml_content.contains("[embedding]"));
    }

    #[test]
    fn test_embedding_section_written_when_model_is_some() {
        let embedding_enabled = true;
        let embedding_model: Option<String> = Some("text-embedding-3-small".to_string());
        let mut toml_content = String::new();
        if embedding_enabled && embedding_model.is_some() {
            let emb_model_str = embedding_model.as_deref().unwrap();
            toml_content.push_str("\n[embedding]\n");
            toml_content.push_str("enabled = true\n");
            toml_content.push_str(&format!("model = \"{}\"\n", emb_model_str));
        }
        assert!(toml_content.contains("[embedding]"));
        assert!(toml_content.contains("model = \"text-embedding-3-small\""));
    }

    #[test]
    fn test_base_url_not_written_when_none() {
        let base_url: Option<String> = None;
        let mut c = String::new();
        if let Some(ref url) = base_url {
            c.push_str(&format!("base_url = \"{}\"\n", url));
        }
        assert!(!c.contains("base_url"));
    }

    #[test]
    fn test_base_url_written_when_some() {
        let base_url = Some("https://api.openai.com/v1".to_string());
        let mut c = String::new();
        if let Some(ref url) = base_url {
            c.push_str(&format!("base_url = \"{}\"\n", url));
        }
        assert!(c.contains("https://api.openai.com/v1"));
    }

    #[test]
    fn test_api_key_not_written_when_empty() {
        let api_key = "";
        let mut c = String::new();
        if !api_key.is_empty() {
            c.push_str(&format!("api_key = \"{}\"\n", api_key));
        }
        assert!(!c.contains("api_key"));
    }

    #[test]
    fn test_api_key_written_when_set() {
        let api_key = "ghu_test123";
        let mut c = String::new();
        if !api_key.is_empty() {
            c.push_str(&format!("api_key = \"{}\"\n", api_key));
        }
        assert!(c.contains("ghu_test123"));
    }

    #[test]
    fn test_openrouter_models_filtering() {
        let raw_response = r#"{
            "data": [
                {"id": "meta-llama/llama-3.3-70b-instruct"},
                {"id": "meta-llama/llama-3.3-70b-instruct:free"},
                {"id": "google/gemini-2.0-flash"},
                {"id": "google/gemini-2.0-flash:free"},
                {"id": "openai/gpt-4o-mini"},
                {"id": "deepseek/deepseek-chat"}
            ]
        }"#;

        let body: OpenRouterModelsResponse = serde_json::from_str(raw_response).unwrap();
        let model_ids: Vec<String> = body.data.into_iter().map(|m| m.id).collect();
        let model_set: std::collections::HashSet<String> = model_ids.iter().cloned().collect();

        // Simulate ZDR list where gemini and llama support ZDR, but others do not
        let zdr_set: std::collections::HashSet<String> = vec![
            "meta-llama/llama-3.3-70b-instruct".to_string(),
            "google/gemini-2.0-flash".to_string(),
        ]
        .into_iter()
        .collect();

        let mut filtered = Vec::new();
        for m in &model_ids {
            if zdr_set.contains(m) && !m.ends_with(":free") {
                let free = format!("{}:free", m);
                if model_set.contains(&free) {
                    filtered.push(m.clone());
                }
            }
        }
        filtered.sort();

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0], "google/gemini-2.0-flash");
        assert_eq!(filtered[1], "meta-llama/llama-3.3-70b-instruct");
    }

    #[test]
    fn test_openrouter_embedding_models_sorting_and_default() {
        let raw_response = r#"{
            "data": [
                {
                    "id": "openai/text-embedding-3-small",
                    "pricing": {"prompt": "0.00000002", "completion": "0.0"}
                },
                {
                    "id": "google/gemini-embedding-2",
                    "pricing": {"prompt": "0.0000002", "completion": "0.0"}
                },
                {
                    "id": "nvidia/llama-nemotron-embed-vl-1b-v2:free",
                    "pricing": {"prompt": "0.0", "completion": "0.0"}
                },
                {
                    "id": "another/free-model",
                    "pricing": {"prompt": "0.0", "completion": "0.0"}
                }
            ]
        }"#;

        let body: OpenRouterModelsResponse = serde_json::from_str(raw_response).unwrap();

        let mut sorted_data = body.data;
        sorted_data.sort_by(|a, b| a.id.cmp(&b.id));

        let mut min_sum: Option<f64> = None;
        let mut default_model: Option<String> = None;

        for m in &sorted_data {
            let (prompt_price, completion_price) = if let Some(ref pricing) = m.pricing {
                let p = pricing
                    .prompt
                    .as_deref()
                    .unwrap_or("0.0")
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let c = pricing
                    .completion
                    .as_deref()
                    .unwrap_or("0.0")
                    .parse::<f64>()
                    .unwrap_or(0.0);
                (p, c)
            } else {
                (0.0, 0.0)
            };
            let sum = prompt_price + completion_price;
            if min_sum.is_none() || Some(sum) < min_sum {
                min_sum = Some(sum);
                default_model = Some(m.id.clone());
            }
        }

        let model_ids: Vec<String> = sorted_data.into_iter().map(|m| m.id).collect();
        assert_eq!(model_ids.len(), 4);
        assert_eq!(model_ids[0], "another/free-model");
        assert_eq!(model_ids[1], "google/gemini-embedding-2");
        assert_eq!(model_ids[2], "nvidia/llama-nemotron-embed-vl-1b-v2:free");
        assert_eq!(model_ids[3], "openai/text-embedding-3-small");

        // The default model should be the cheapest one (sum = 0.0).
        // Since both "another/free-model" and "nvidia/llama-nemotron-embed-vl-1b-v2:free" have sum = 0.0,
        // "another/free-model" should be chosen because it comes first alphabetically.
        assert_eq!(default_model.unwrap(), "another/free-model");
    }
}
