use colored::Colorize;
use std::io::{self, Write};
use std::path::PathBuf;

/// Interactive setup wizard for Rune configuration.
pub async fn run_setup() {
    println!();
    println!("{}", "  ᚱ  Rune Setup Wizard".cyan().bold());
    println!("{}", "  ─────────────────────".dimmed());
    println!();
    println!(
        "  This will create a configuration file at {}",
        "~/.rune/rune.toml".green()
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
    println!();

    let provider_choice = prompt("  Select [1-6]: ").unwrap_or_default();
    let (base_url, provider_name, provider_id, key_hint) = match provider_choice.trim() {
        "1" => (
            None,
            "GitHub Copilot",
            "github-copilot",
            "GitHub PAT (starts with ghu_ or ghp_)",
        ),
        "2" => (
            Some("https://generativelanguage.googleapis.com/v1beta/openai".to_string()),
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
    println!("{}", "2. Enter your API key:".bold());
    println!("   {}", format!("Hint: {}", key_hint).dimmed());
    let api_key = prompt("  API key: ").unwrap_or_default().trim().to_string();
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

    let model_choice = prompt("  Select or type model name: ").unwrap_or_default();
    let model = match (provider_choice.trim(), model_choice.trim()) {
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
        (_, choice) if !choice.is_empty() && !["3", "4"].contains(&choice) => choice.to_string(),
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
    };
    println!("  {} Model: {}", "✓".green(), model.green());
    println!();

    // 4. Skills directory
    println!("{}", "4. Skills directory:".bold());
    let skills_default = "./skills";
    let skills_input = prompt(&format!("  Path [{}]: ", skills_default)).unwrap_or_default();
    let skills_dir = if skills_input.trim().is_empty() {
        skills_default.to_string()
    } else {
        skills_input.trim().to_string()
    };
    println!("  {} Skills dir: {}", "✓".green(), skills_dir);
    println!();

    // 5. Write config
    let config_dir = dirs_home().join(".rune");
    let config_path = config_dir.join("rune.toml");

    let mut toml_content = String::new();
    toml_content.push_str(&format!("model = \"{}\"\n", model));
    if !api_key.is_empty() {
        toml_content.push_str(&format!("api_key = \"{}\"\n", api_key));
    }
    if let Some(ref url) = base_url {
        toml_content.push_str(&format!("base_url = \"{}\"\n", url));
    }
    toml_content.push_str(&format!("skills_dir = \"{}\"\n", skills_dir));
    toml_content.push_str("log_level = \"warn\"\n");
    toml_content.push('\n');
    toml_content.push_str("[policy]\n");
    toml_content.push_str("mode = \"confirm\"\n");
    toml_content.push_str("allowed_commands = [\"ls\", \"cat\", \"head\", \"ps\", \"echo\", \"uname\", \"free\", \"df\", \"date\", \"hostname\"]\n");
    toml_content.push_str("allowed_domains = []\n");

    // Show preview
    println!("{}", "─".repeat(50).dimmed());
    println!("{}", "  Configuration preview:".bold());
    println!("{}", "─".repeat(50).dimmed());
    for line in toml_content.lines() {
        println!("  {}", line.dimmed());
    }
    println!("{}", "─".repeat(50).dimmed());
    println!();

    let confirm = prompt("  Write to ~/.rune/rune.toml? [Y/n]: ").unwrap_or_default();
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
