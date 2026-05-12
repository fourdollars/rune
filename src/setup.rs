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
        };
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let table: toml::Table = content.parse().unwrap_or_default();

    let policy_section = extract_toml_section(&content, "[policy]");
    let embedding_section = extract_toml_section(&content, "[embedding]");

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
            Some("https://openrouter.ai/api/v1".to_string()),
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

    let base_url_display = base_url.as_deref().unwrap_or("(auto — Copilot endpoint)");
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
            let masked = format!(
                "{}...{}",
                &k[..k.len().min(4)],
                &k[k.len().saturating_sub(4)..]
            );
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
            let masked = format!(
                "{}...{}",
                &k[..k.len().min(4)],
                &k[k.len().saturating_sub(4)..]
            );
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
            &api_key[..api_key.len().min(8)]
        );
    }
    println!();

    // 3. Model selection
    println!("{}", "3. Choose a model:".bold());
    if let Some(ref m) = existing.model {
        println!("   {}", format!("(current: {})", m).dimmed());
    }
    match provider_choice.trim() {
        "1" => {
            println!(
                "   {} gpt-4o          (powerful, recommended)",
                "[1]".cyan()
            );
            println!("   {} gpt-4o-mini     (fast, cheap)", "[2]".cyan());
            println!("   {} claude-3.5-sonnet", "[3]".cyan());
            println!("   {} Custom", "[4]".cyan());
        }
        "2" => {
            println!("   {} gemini-2.0-flash (fast)", "[1]".cyan());
            println!("   {} gemini-1.5-pro   (powerful)", "[2]".cyan());
            println!("   {} Custom", "[3]".cyan());
        }
        "3" => {
            println!("   {} gpt-4o-mini     (fast, cheap)", "[1]".cyan());
            println!("   {} gpt-4o          (powerful)", "[2]".cyan());
            println!("   {} gpt-4-turbo     (balanced)", "[3]".cyan());
            println!("   {} Custom", "[4]".cyan());
        }
        "4" => {
            println!("   {} openai/gpt-4o-mini", "[1]".cyan());
            println!("   {} anthropic/claude-3.5-sonnet", "[2]".cyan());
            println!("   {} google/gemini-pro", "[3]".cyan());
            println!("   {} Custom", "[4]".cyan());
        }
        _ => {
            println!("   {} Enter model name", "[1]".cyan());
        }
    }
    println!();

    let model_prompt = if let Some(ref m) = existing.model {
        format!("  Select or type model name (Enter={}): ", m)
    } else {
        "  Select or type model name: ".to_string()
    };
    let model_choice = prompt(&model_prompt).unwrap_or_default();
    let model = if model_choice.trim().is_empty() && existing.model.is_some() {
        existing.model.clone().unwrap()
    } else {
        match (provider_choice.trim(), model_choice.trim()) {
            ("1", "1") => "gpt-4o".to_string(),
            ("1", "2") => "gpt-4o-mini".to_string(),
            ("1", "3") => "claude-3.5-sonnet".to_string(),
            ("2", "1") => "gemini-2.0-flash".to_string(),
            ("2", "2") => "gemini-1.5-pro".to_string(),
            ("3", "1") => "gpt-4o-mini".to_string(),
            ("3", "2") => "gpt-4o".to_string(),
            ("3", "3") => "gpt-4-turbo".to_string(),
            ("4", "1") => "openai/gpt-4o-mini".to_string(),
            ("4", "2") => "anthropic/claude-3.5-sonnet".to_string(),
            ("4", "3") => "google/gemini-pro".to_string(),
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
    println!("  {} Model: {}", "✓".green(), model.green());
    println!();

    // 4. Skills directory
    println!("{}", "4. Skills directory:".bold());
    let skills_default = existing.skills_dir.as_deref().unwrap_or("./skills");
    let skills_input = prompt(&format!("  Path [{}]: ", skills_default)).unwrap_or_default();
    let skills_dir = if skills_input.trim().is_empty() {
        skills_default.to_string()
    } else {
        skills_input.trim().to_string()
    };
    println!("  {} Skills dir: {}", "✓".green(), skills_dir);
    println!();

    // 5. Embedding
    println!("{}", "5. Enable semantic features (embedding)?".bold());
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

    // 5b. Embedding model (only if enabled)
    let embedding_model = if embedding_enabled {
        let default_emb_model = match provider_choice.trim() {
            "1" => "text-embedding-3-small",
            "2" => "gemini-embedding-2",
            "4" => "nvidia/llama-nemotron-embed-vl-1b-v2:free",
            _ => "text-embedding-3-small",
        };
        // Check existing config for model
        let current_emb_model = existing.embedding_section.as_ref().and_then(|s| {
            s.lines()
                .find(|l| l.trim().starts_with("model"))
                .and_then(|l| l.split('"').nth(1))
                .map(|s| s.to_string())
        });
        let emb_model_default = current_emb_model.as_deref().unwrap_or(default_emb_model);
        println!();
        println!("   {}", "Embedding model:".bold());
        let emb_model_prompt = format!("  Model [{}]: ", emb_model_default);
        let emb_model_input = prompt(&emb_model_prompt).unwrap_or_default();
        let model = if emb_model_input.trim().is_empty() {
            emb_model_default.to_string()
        } else {
            emb_model_input.trim().to_string()
        };
        println!("  {} Embedding model: {}", "✓".green(), model.cyan());
        Some(model)
    } else {
        None
    };
    println!();

    // 6. Thinking level
    println!("{}", "6. Thinking (reasoning effort):".bold());
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
    let thinking = if thinking_input.trim().is_empty() {
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
    println!("  {} Thinking: {}", "✓".green(), thinking.cyan());
    println!();

    // 7. Build config — preserve existing [policy] and [embedding] sections
    let config_path = target_config_path.clone();
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut toml_content = String::new();
    toml_content.push_str(&format!("model = \"{}\"\n", model));
    toml_content.push_str(&format!("provider = \"{}\"\n", provider_id));
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
    if embedding_enabled {
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
    fn test_extract_toml_section_missing() {
        let content = "model = \"gpt-4o\"\napi_key = \"test\"\n";
        assert!(extract_toml_section(content, "[policy]").is_none());
        assert!(extract_toml_section(content, "[embedding]").is_none());
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
}
