use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::IsTerminal;
use std::time::Duration;

use crate::agent::{Agent, StopReason};
use crate::config;
use crate::provider::{CopilotProvider, GeminiProvider, OpenAiProvider, ProviderRegistry};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Print the startup banner.
fn print_banner() {
    // Runic ASCII art banner with mystical colors
    let rune_border = format!(
        "        {} {} {} {} {} {} {} {} {} {} {}",
        "ᛟ".magenta(),
        "ᚺ".bright_cyan(),
        "ᛊ".blue(),
        "ᛏ".bright_magenta(),
        "ᛒ".cyan(),
        "ᛖ".bright_blue(),
        "ᚹ".magenta(),
        "ᛗ".bright_cyan(),
        "ᛚ".blue(),
        "ᛝ".bright_magenta(),
        "ᛟ".cyan(),
    );
    let line2 = r#"    ┌───────────────────────────────────┐"#;
    let line3 = r#"    │                                   │"#;
    let line4 = r#"    │    ᚱ  ᚢ  ᚾ  ᛖ                     │"#;
    let line5 = r#"    │                                   │"#;
    let line6 = r#"    │    Zero-Trust AI Agent            │"#;
    let line7 = r#"    │    ══════════════════             │"#;
    let line8 = format!("    │    v{:<6}⚡ sandboxed            │", VERSION);
    let line9 = r#"    │                                   │"#;
    let line10 = r#"    └───────────────────────────────────┘"#;
    let rune_border2 = format!(
        "        {} {} {} {} {} {} {} {} {} {} {}",
        "ᛟ".cyan(),
        "ᚺ".bright_magenta(),
        "ᛊ".blue(),
        "ᛏ".bright_cyan(),
        "ᛒ".magenta(),
        "ᛖ".bright_blue(),
        "ᚹ".cyan(),
        "ᛗ".bright_magenta(),
        "ᛚ".magenta(),
        "ᛝ".bright_cyan(),
        "ᛟ".blue(),
    );

    println!();
    println!("{}", rune_border);
    println!("{}", line2.magenta());
    println!("{}", line3.magenta());
    println!("{}", line4.bright_cyan().bold());
    println!("{}", line5.magenta());
    println!("{}", line6.white());
    println!("{}", line7.dimmed());
    println!("{}", line8.green());
    println!("{}", line9.magenta());
    println!("{}", line10.magenta());
    println!("{}", rune_border2);
    println!();
}

/// Print available commands.
fn print_help() {
    println!("{}", "Commands:".bold());
    println!();

    println!("  {}", "Prompts".bold());
    println!(
        "    {:<24} {}",
        "<text>".green(),
        "Send a prompt to the agent"
    );
    println!(
        "    {:<24} {}",
        "/multi".green(),
        "Enter multi-line mode (end with ';;')"
    );
    println!(
        "    {:<24} {}",
        "/image <path>".green(),
        "Attach an image to the next message"
    );
    println!();

    println!("  {}", "Session".bold());
    println!(
        "    {:<24} {}",
        "/info".green(),
        "Show session status (model, provider, context)"
    );
    println!(
        "    {:<24} {}",
        "/info context".green(),
        "Show detailed context usage (tokens, %)"
    );
    println!(
        "    {:<24} {}",
        "/compact".green(),
        "Compact (summarize) older conversation context"
    );
    println!(
        "    {:<24} {}",
        "/reset".green(),
        "Reset conversation history"
    );
    println!("    {:<24} {}", "/clear".green(), "Clear the screen");
    println!();

    println!("  {}", "Configuration".bold());
    println!(
        "    {:<24} {}",
        "/config".green(),
        "Show current configuration"
    );
    println!("    {:<24} {}", "/policy".green(), "Show policy summary");
    println!(
        "    {:<24} {}",
        "/policy full".green(),
        "Show full sandbox + policy status"
    );
    println!(
        "    {:<24} {}",
        "/tools".green(),
        "List available built-in tools"
    );
    println!("    {:<24} {}", "/skills".green(), "List loaded skills");
    println!(
        "    {:<24} {}",
        "/trace".green(),
        "Show trace output directory"
    );
    println!();

    println!("  {}", "Runtime".bold());
    println!(
        "    {:<24} {}",
        "/add-dir <path>".green(),
        "Add directory to read-only paths (saved)"
    );
    println!(
        "    {:<24} {}",
        "/add-rw-dir <path>".green(),
        "Add directory to read-write paths (saved)"
    );
    println!();

    println!("  {}", "Other".bold());
    println!(
        "    {:<24} {}",
        "/thinking [level]".green(),
        "Show/set thinking: off|low|medium|high|xhigh"
    );
    println!("    {:<24} {}", "/version".green(), "Show version info");
    println!("    {:<24} {}", "/help".green(), "Show this help");
    println!("    {:<24} {}", "/exit, /quit".green(), "Exit the CLI");
    println!();

    println!("{}", "Tips:".bold());
    println!(
        "    {} Type your prompt directly — no prefix needed",
        "•".dimmed()
    );
    println!(
        "    {} Use {} in prompts to load skill context",
        "•".dimmed(),
        "@skill_name".cyan()
    );
    println!(
        "    {} Use {}/{} to browse command history",
        "•".dimmed(),
        "↑".cyan(),
        "↓".cyan()
    );
    println!(
        "    {} {} interrupts the current agent run",
        "•".dimmed(),
        "Ctrl+C".cyan()
    );
    println!(
        "    {} Use {} or {} to attach images for vision models",
        "•".dimmed(),
        "/image".cyan(),
        "/img".cyan()
    );
}

/// Create a spinner for LLM thinking.
fn create_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.cyan} {msg}")
            .expect("invalid spinner template"),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Format and display the agent's stop reason.
fn display_result(reason: &StopReason, streamed: bool) {
    match reason {
        StopReason::FinalAnswer(ans) => {
            println!();
            println!("{}", "─".repeat(60).dimmed());
            if !streamed {
                println!("{}", ans);
            }
            println!("{}", "─".repeat(60).dimmed());
        }
        StopReason::MaxSteps => {
            println!("\n{}", "⚠ Stopped: maximum steps reached".yellow());
        }
        StopReason::TokenBudgetExhausted => {
            println!("\n{}", "⚠ Stopped: token budget exhausted".yellow());
        }
        StopReason::Error(e) => {
            println!("\n{} {}", "✗ Error:".red().bold(), e);
        }
        StopReason::UserInterrupt => {
            println!("\n{}", "⚡ Interrupted by user".yellow());
        }
    }
}

/// Display configuration summary.
fn show_config(cfg: &config::RuneConfig) {
    println!("{}", "Current Configuration:".bold());
    println!("  {}  {}", "model:".dimmed(), cfg.model.green());
    println!(
        "  {}  {}",
        "api_key:".dimmed(),
        if cfg.api_key.is_some() {
            "*** (set)".to_string()
        } else {
            "(not set)".red().to_string()
        }
    );
    println!("  {}  {}", "skills_dir:".dimmed(), cfg.skills_dir);
    println!("  {}  {}", "log_level:".dimmed(), cfg.log_level);
    println!(
        "  {}  {}",
        "max_steps:".dimmed(),
        cfg.max_steps
            .map_or("unlimited".to_string(), |s| s.to_string())
    );
    println!(
        "  {}  {}",
        "token_budget:".dimmed(),
        cfg.token_budget
            .map_or("unlimited".to_string(), |b| b.to_string())
    );
    println!(
        "  {}  {}",
        "timeout_secs:".dimmed(),
        cfg.timeout_secs
            .map_or("unlimited".to_string(), |t| t.to_string())
    );
}

/// Display available tools.
fn show_tools() {
    let registry = ToolRegistry::new(vec![]);
    let defs = registry.tool_definitions();
    println!("{} ({} available)", "Built-in Tools:".bold(), defs.len());
    for def in &defs {
        if let Some(func) = def.get("function") {
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = func
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            println!("  {} — {}", name.green(), desc.dimmed());
        }
    }
}

/// Display loaded skills.
fn show_skills(cfg: &config::RuneConfig) {
    let search_paths = vec![std::path::PathBuf::from(&cfg.skills_dir)];
    let loader = SkillLoader::new(search_paths);
    println!("{}", "Skill Loader:".bold());
    println!("  {} {}", "search_dir:".dimmed(), cfg.skills_dir);
    println!(
        "  {} Use @skill_name in prompts to load skills",
        "usage:".dimmed()
    );
    let skill_dir = std::path::Path::new(&cfg.skills_dir);
    if skill_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(skill_dir) {
            let mut skills: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            skills.sort();
            if !skills.is_empty() {
                println!("  {} {}", "found:".dimmed(), skills.join(", ").green());
            } else {
                println!("  {} (no skills found)", "found:".dimmed());
            }
        }
    } else {
        println!("  {} (directory does not exist)", "status:".dimmed());
    }
    let _ = loader;
}

/// Display current session state: model, context usage, skills, MCP.
fn show_info(cfg: &config::RuneConfig, agent: &crate::agent::Agent) {
    println!("{}", "Session Info:".bold());
    println!();

    // LLM Provider
    println!("  {}", "LLM Provider:".bold());
    let provider_display = if let Some(ref p) = cfg.provider {
        match p.as_str() {
            "github-copilot" | "copilot" => "GitHub Copilot".to_string(),
            "gemini" | "google" => "Google Gemini".to_string(),
            "openrouter" => "OpenRouter".to_string(),
            "anthropic" => "Anthropic".to_string(),
            "ollama" => "Ollama".to_string(),
            "openai" => "OpenAI".to_string(),
            other => other.to_string(),
        }
    } else if let Some(ref key) = cfg.api_key {
        if key.starts_with("ghu_") || key.starts_with("ghp_") {
            "GitHub Copilot".to_string()
        } else if key.starts_with("AIza") {
            "Google Gemini".to_string()
        } else if key.starts_with("sk-or-") {
            "OpenRouter".to_string()
        } else if key.starts_with("sk-") {
            "OpenAI".to_string()
        } else {
            "Custom".to_string()
        }
    } else {
        "(not configured)".to_string()
    };

    if provider_display == "(not configured)" {
        println!(
            "    {} provider: {}",
            "•".dimmed(),
            "(not configured)".red()
        );
    } else {
        println!(
            "    {} provider: {}",
            "•".dimmed(),
            provider_display.green()
        );
    }
    println!("    {} model: {}", "•".dimmed(), cfg.model.green());
    println!(
        "    {} thinking: {}",
        "•".dimmed(),
        cfg.thinking.as_deref().unwrap_or("none").cyan()
    );
    if let Some(ref url) = cfg.base_url {
        println!("    {} endpoint: {}", "•".dimmed(), url.dimmed());
    }
    println!();

    // Context / Token usage
    println!("  {}", "Context:".bold());
    println!(
        "    {} tokens used: {} / {} (budget)",
        "•".dimmed(),
        agent.tokens_used(),
        cfg.token_budget
            .map_or("unlimited".to_string(), |b| b.to_string())
    );
    println!(
        "    {} steps: {} / {} (max)",
        "•".dimmed(),
        agent.step_count(),
        cfg.max_steps
            .map_or("unlimited".to_string(), |s| s.to_string())
    );
    println!(
        "    {} timeout: {}",
        "•".dimmed(),
        cfg.timeout_secs
            .map_or("unlimited".to_string(), |t| format!("{}s", t))
    );
    println!();

    // Skills
    println!("  {}", "Skills:".bold());
    let skill_dir = std::path::Path::new(&cfg.skills_dir);
    if skill_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(skill_dir) {
            let skills: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            if skills.is_empty() {
                println!("    {} (none found in {})", "•".dimmed(), cfg.skills_dir);
            } else {
                for s in &skills {
                    println!("    {} @{}", "•".dimmed(), s.green());
                }
            }
        }
    } else {
        println!(
            "    {} (dir {} does not exist)",
            "•".dimmed(),
            cfg.skills_dir
        );
    }
    println!();

    // MCP
    println!("  {}", "MCP Servers:".bold());
    println!("    {} (none configured)", "•".dimmed());
    println!();

    // Policy mode (brief)
    println!("  {}", "Policy:".bold());
    println!("    {} mode: {}", "•".dimmed(), cfg.policy.mode.cyan());
    println!("    {} (use /policy for full details)", "•".dimmed());
}

/// Display sandbox policy & permissions.

/// Display context details.
fn show_context(agent: &crate::agent::Agent) {
    let context_tokens = agent.total_context_tokens();
    let context_window = agent.config.context_window;
    let pct = if context_window > 0 {
        (context_tokens as f64 / context_window as f64 * 100.0) as u32
    } else {
        0
    };
    println!("{}", "Context Details:".bold());
    println!("  {} messages: {}", "•".dimmed(), agent.message_count());
    println!("  {} chars: ~{}", "•".dimmed(), agent.context_chars());
    println!(
        "  {} context: {}/{} tokens ({}%)",
        "•".dimmed(),
        context_tokens,
        context_window,
        pct
    );
    println!(
        "  {} tokens consumed: {}",
        "•".dimmed(),
        agent.tokens_used()
    );
    println!();
    println!("  {}", "By role:".bold());
    for (role, count) in agent.context_summary() {
        println!("    {} {}: {}", "•".dimmed(), role, count);
    }
    println!();
    if pct > 70 {
        println!(
            "  {} Context at {}% — will auto-compact at {}%",
            "⚠".yellow(),
            pct,
            (agent.config.compact_threshold * 100.0) as u32
        );
    } else {
        println!("  {} Use /compact to summarize older messages", "ℹ".cyan());
    }
}
fn show_policy_summary(cfg: &config::RuneConfig) {
    let p = &cfg.policy;
    println!("{}", "Policy Summary:".bold());
    println!("  {} mode: {}", "•".dimmed(), p.mode.cyan());
    println!(
        "  {} allowed commands: {}",
        "•".dimmed(),
        if p.allowed_commands.is_empty() {
            "(none — all blocked in allowlist mode)".to_string()
        } else {
            format!("{}", p.allowed_commands.join(", "))
        }
    );
    println!(
        "  {} allowed domains: {}",
        "•".dimmed(),
        if p.allowed_domains.is_empty() {
            "(none — network blocked)".to_string()
        } else {
            p.allowed_domains.join(", ")
        }
    );
    println!(
        "  {} denied syscalls: {}",
        "•".dimmed(),
        p.allowed_syscalls.join(", ")
    );
    println!(
        "  {} paths rw: {}",
        "•".dimmed(),
        p.allowed_paths_rw.join(", ")
    );
    println!(
        "  {} paths ro: {}",
        "•".dimmed(),
        p.allowed_paths_ro.join(", ")
    );
    println!(
        "  {} denied paths: {}",
        "•".dimmed(),
        p.denied_paths.join(", ")
    );
    println!(
        "  {} memory limit: {}MB | max pids: {}",
        "•".dimmed(),
        p.max_memory_mb,
        p.max_pids
    );
    println!();
    println!(
        "  {} Use {} for full sandbox status",
        "ℹ".cyan(),
        "/policy full".bold()
    );
}

fn show_policy_full(cfg: &config::RuneConfig) {
    println!("{}", "Sandbox & Permissions Info:".bold());
    println!();

    // Check unshare capability
    let unshare_ok = std::process::Command::new("unshare")
        .args(["--user", "--net", "--", "true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    println!("  {}", "Network Isolation:".bold());
    if unshare_ok {
        println!("    {} Active (unshare --user --net)", "✓".green());
        println!(
            "    {} All tool commands run in isolated network namespace",
            "•".dimmed()
        );
        println!(
            "    {} No DNS resolution available inside sandbox",
            "•".dimmed()
        );
        println!("    {} No outbound connections possible", "•".dimmed());
    } else {
        println!("    {} DEGRADED — unshare not available", "⚠".yellow());
        println!(
            "    {} Commands run WITHOUT network isolation",
            "•".dimmed()
        );
    }
    println!();

    println!("  {}", "Filesystem Access:".bold());
    println!("    {} User namespace UID remapping active", "•".dimmed());
    println!(
        "    {} Cannot read: /etc/shadow, /root, privileged files",
        "✓".green()
    );
    println!(
        "    {} Cannot write: /root, /etc, system directories",
        "✓".green()
    );
    println!("    {} Can read: general user-readable files", "•".dimmed());
    println!("    {} Can write: /tmp, project directories", "•".dimmed());
    println!();

    println!("  {}", "Tool Restrictions:".bold());
    println!(
        "    {} read_file    — sandboxed, 32KB truncation",
        "•".dimmed()
    );
    println!(
        "    {} write_file   — sandboxed, allowed dirs only",
        "•".dimmed()
    );
    println!("    {} list_dir     — sandboxed", "•".dimmed());
    println!(
        "    {} execute_cmd — sandboxed, network blocked",
        "•".dimmed()
    );
    println!(
        "    {} fetch_url    — sandboxed, {} (network blocked)",
        "•".dimmed(),
        "ALWAYS FAILS".red()
    );
    println!();

    println!("  {}", "Timeouts:".bold());
    println!(
        "    {} Default command timeout: {}",
        "•".dimmed(),
        cfg.timeout_secs
            .map_or("unlimited".to_string(), |t| format!("{}s", t))
    );
    println!(
        "    {} Max agent steps: {}",
        "•".dimmed(),
        cfg.max_steps
            .map_or("unlimited".to_string(), |s| s.to_string())
    );
    println!(
        "    {} Token budget: {}",
        "•".dimmed(),
        cfg.token_budget
            .map_or("unlimited".to_string(), |b| b.to_string())
    );
    println!();

    println!("  {}", "LLM Provider:".bold());
    if let Some(ref p) = cfg.provider {
        match p.as_str() {
            "github-copilot" | "copilot" => {
                println!("    {} GitHub Copilot (auto token refresh)", "•".dimmed())
            }
            "gemini" | "google" => println!("    {} Google Gemini", "•".dimmed()),
            "openrouter" => println!("    {} OpenRouter", "•".dimmed()),
            "anthropic" => println!("    {} Anthropic", "•".dimmed()),
            "ollama" => println!("    {} Ollama (local)", "•".dimmed()),
            "openai" => println!("    {} OpenAI", "•".dimmed()),
            other => println!("    {} {} (custom)", "•".dimmed(), other),
        }
    } else if let Some(ref key) = cfg.api_key {
        if key.starts_with("ghu_") || key.starts_with("ghp_") {
            println!("    {} GitHub Copilot (auto token refresh)", "•".dimmed());
        } else if key.starts_with("AIza") {
            println!("    {} Google Gemini", "•".dimmed());
        } else if key.starts_with("sk-or-") {
            println!("    {} OpenRouter", "•".dimmed());
        } else if key.starts_with("sk-") {
            println!("    {} OpenAI", "•".dimmed());
        } else {
            println!("    {} Custom provider", "•".dimmed());
        }
    } else {
        println!("    {} No API key configured", "✗".red());
    }
    println!("    {} Model: {}", "•".dimmed(), cfg.model.green());
    if let Some(ref url) = cfg.base_url {
        println!("    {} Endpoint: {}", "•".dimmed(), url);
    }
    println!(
        "    {} Provider calls are NOT sandboxed (need network for LLM)",
        "ℹ".cyan()
    );
    println!();

    println!("  {}", "Summary:".bold());
    println!(
        "    Tools: network={}, filesystem={}, timeout={}",
        if unshare_ok {
            "BLOCKED".red().to_string()
        } else {
            "OPEN (degraded)".yellow().to_string()
        },
        "RESTRICTED".green(),
        cfg.timeout_secs
            .map_or("unlimited".to_string(), |t| format!("{}s", t))
    );
}

/// Read multi-line input until ";;" on its own line.
fn read_multiline(editor: &mut DefaultEditor) -> Option<String> {
    println!(
        "{}",
        "Multi-line mode. Enter ';;' on its own line to submit:".dimmed()
    );
    println!("{}", "─".repeat(40).dimmed());

    let mut buffer = Vec::new();
    loop {
        match editor.readline("... ") {
            Ok(line) => {
                if line.trim() == ";;" {
                    break;
                }
                buffer.push(line);
            }
            Err(ReadlineError::Interrupted) => {
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(_) => break,
        }
    }

    if buffer.is_empty() {
        None
    } else {
        Some(buffer.join("\n"))
    }
}

/// Run a prompt through the agent with spinner feedback.
fn is_json_mode(cfg: &config::RuneConfig) -> bool {
    cfg.json_output
}

async fn execute_prompt(agent: &mut Agent, input: &str) -> StopReason {
    let spinner = if agent.config.policy.mode == "confirm"
        || is_json_mode(&agent.config)
        || agent.is_interactive()
    {
        None
    } else {
        Some(create_spinner("Thinking..."))
    };
    let result = agent.run(input).await;
    if is_json_mode(&agent.config) {
        let payload = match &result {
            StopReason::FinalAnswer(a) => serde_json::json!({
                "answer": a,
                "tools_used": agent.tool_call_names(),
                "steps": agent.step_count(),
                "tokens": agent.tokens_used()
            }),
            StopReason::Error(e) => serde_json::json!({"error": e}),
            other => serde_json::json!({"error": format!("{:?}", other)}),
        };
        println!("{}", payload);
        return result;
    }
    if let Some(s) = spinner {
        s.finish_and_clear();
    }
    display_result(&result, agent.is_interactive());
    // Show executed commands summary
    let cmds = agent.executed_commands();
    if !cmds.is_empty() {
        println!("  {} commands executed: {}", "📋".dimmed(), cmds.len());
        for c in cmds {
            println!("    {} {}", "▸".dimmed(), c.dimmed());
        }
    }
    // Run summary
    if agent.step_count() > 0 {
        println!(
            "  {} [{} steps | {} tokens | {} tool calls]",
            "⚡".dimmed(),
            agent.step_count(),
            agent.tokens_used(),
            agent.tool_call_count()
        );
    }
    result
}

/// Initialize the provider registry from config.
fn init_provider(cfg: &config::RuneConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    if let Some(ref key) = cfg.api_key {
        // If the user explicitly set --provider / RUNE_PROVIDER, prefer it.
        // Otherwise auto-detect from api_key prefixes or base_url.
        let provider_name = cfg.provider.as_deref().unwrap_or_else(|| {
            if key.starts_with("ghu_")
                || key.starts_with("ghp_")
                || cfg
                    .base_url
                    .as_deref()
                    .map(|u| u.contains("githubcopilot"))
                    .unwrap_or(false)
            {
                "github-copilot"
            } else if key.starts_with("AIza")
                || cfg
                    .base_url
                    .as_deref()
                    .map(|u| u.contains("generativelanguage.googleapis.com"))
                    .unwrap_or(false)
            {
                "gemini"
            } else if key.starts_with("sk-or-") {
                "openrouter"
            } else {
                "openai"
            }
        });

        match provider_name {
            "github-copilot" | "copilot" => {
                registry.register(Box::new(CopilotProvider::new(key.clone())));
            }
            "gemini" | "google" => {
                registry.register(Box::new(GeminiProvider::new(
                    key.clone(),
                    Some(cfg.model.clone()),
                    cfg.base_url.clone(),
                )));
            }
            // Default: OpenAI-compatible provider (OpenAI, OpenRouter, Anthropic proxy, Ollama)
            other => {
                registry.register(Box::new(OpenAiProvider::new(
                    other.to_string(),
                    key.clone(),
                    cfg.base_url.clone(),
                )));
            }
        }
    }

    registry
}

/// Load an image file and return base64-encoded data + MIME type.
fn load_image_as_base64(path: &str) -> Result<(String, String), String> {
    use base64::Engine;
    let resolved = resolve_path(path);
    let data = std::fs::read(&resolved).map_err(|e| format!("{}: {}", resolved, e))?;
    let mime = match resolved
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok((encoded, mime.to_string()))
}

/// Resolve a path argument: expand ~ and make absolute.
fn resolve_path(path: &str) -> String {
    let expanded = if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            format!("{}/{}", home, &path[2..])
        } else {
            path.to_string()
        }
    } else if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else {
        path.to_string()
    };

    // Make absolute if relative
    let abs = if expanded.starts_with('/') {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| format!("{}/{}", cwd.display(), expanded))
            .unwrap_or(expanded)
    };

    // Normalize: collapse . and .. components
    let mut parts: Vec<&str> = Vec::new();
    for component in abs.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(component),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Main CLI entry point.
pub async fn run() {
    let cfg = config::load().unwrap_or_default();
    let provider = init_provider(&cfg);
    let stdin_is_terminal = std::io::stdin().is_terminal();

    if stdin_is_terminal && !is_json_mode(&cfg) {
        print_banner();
    }

    // Start MCP servers if configured
    let mut mcp_manager = crate::mcp::McpManager::new();
    if !cfg.mcp_servers.is_empty() {
        if !is_json_mode(&cfg) {
            eprintln!(
                "  {} Starting {} MCP server(s)...",
                "⚙".dimmed(),
                cfg.mcp_servers.len()
            );
        }
        if let Err(e) = mcp_manager.start_all(cfg.mcp_servers.clone()).await {
            eprintln!("  {} MCP startup failed: {}", "✗".red(), e);
        } else if !is_json_mode(&cfg) {
            let tools = mcp_manager.all_tools();
            if !tools.is_empty() {
                eprintln!("  {} {} MCP tool(s) available", "✓".green(), tools.len());
            }
        }
    }
    if provider.is_empty() {
        if is_json_mode(&cfg) {
            eprintln!(
                "{}",
                "⚠ No API key configured. Set RUNE_API_KEY or use --api-key to connect.".yellow()
            );
            eprintln!(
                "{}",
                "  The agent will not be able to call an LLM without a key.".dimmed()
            );
        } else {
            println!(
                "{}",
                "⚠ No API key configured. Set RUNE_API_KEY or use --api-key to connect.".yellow()
            );
            println!(
                "{}",
                "  The agent will not be able to call an LLM without a key.".dimmed()
            );
            println!();
        }
    }

    // Implicitly add CWD to allowed_paths_ro (runtime only, not persisted)
    let mut cfg = cfg;
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if !cfg.policy.allowed_paths_ro.contains(&cwd_str) {
            cfg.policy.allowed_paths_ro.push(cwd_str);
        }
    }

    // Non-interactive (pipe) mode defaults to allowlist policy unless explicitly overridden
    if !stdin_is_terminal && cfg.policy.mode == "confirm" {
        eprintln!(
            "  {} pipe mode: defaulting policy to allowlist (use --policy-mode to override)",
            "ℹ".dimmed()
        );
        cfg.policy.mode = "allowlist".to_string();
    }

    let embedding_engine = if cfg.embedding.enabled {
        let mut emb_cfg = cfg.embedding.clone();
        // Fallback: use main api_key if embedding-specific key not set
        if emb_cfg.api_key.is_none() {
            emb_cfg.api_key = cfg.api_key.clone();
        }
        // Auto-detect base_url for Copilot and use token refresh
        let is_copilot = cfg
            .api_key
            .as_ref()
            .map(|k| k.starts_with("ghu_") || k.starts_with("ghp_"))
            .unwrap_or(false);
        if is_copilot {
            if emb_cfg.base_url.is_none() {
                emb_cfg.base_url = Some("https://api.githubcopilot.com".to_string());
            }
            let pat = cfg.api_key.clone().unwrap_or_default();
            Some(crate::embedding::EmbeddingEngine::new_copilot(emb_cfg, pat))
        } else {
            Some(crate::embedding::EmbeddingEngine::new(emb_cfg))
        }
    } else {
        None
    };

    let mut agent = Agent::new(cfg.clone(), provider, stdin_is_terminal, embedding_engine);
    agent.set_system_prompt(
        "You are Rune, a high-performance AI agent running in a terminal. \
         You have access to tools: read_file, write_file, list_dir, execute_cmd, fetch_url. \
         Use them when needed. Be concise and accurate.",
    );

    let history_path = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".rune").join("history"));

    let mut editor = if stdin_is_terminal {
        Some(DefaultEditor::new().expect("failed to initialize line editor"))
    } else {
        None
    };
    if let Some(ref mut ed) = editor {
        ed.set_auto_add_history(false);
        // Load persistent history
        if let Some(ref path) = history_path {
            let _ = ed.load_history(path);
        }
    }

    if !stdin_is_terminal {
        use tokio::io::{self, AsyncReadExt};

        let mut input = String::new();
        let mut stdin = io::stdin();
        if let Err(e) = stdin.read_to_string(&mut input).await {
            eprintln!("{} {}", "Read error:".red(), e);
            std::process::exit(1);
        }

        let input = input.trim();
        if input.is_empty() {
            eprintln!("{}", "No piped input received on stdin.".red());
            std::process::exit(1);
        }

        let result = execute_prompt(&mut agent, input).await;
        if !matches!(result, StopReason::FinalAnswer(_)) {
            std::process::exit(1);
        }
        return;
    }

    if !is_json_mode(&cfg) {
        println!(
            "{} Type {} for commands.",
            "Ready.".green().bold(),
            "/help".bold()
        );
        println!();
    }

    let editor = editor.as_mut().expect("interactive editor unavailable");

    let mut pending_image_data: Option<(String, String)> = None;
    loop {
        let cmd = match editor.readline("ᚱ› ") {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if !trimmed.starts_with('/') {
                    let _ = editor.add_history_entry(trimmed.as_str());
                }
                trimmed
            }
            Err(ReadlineError::Interrupted) => {
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => {
                if !is_json_mode(&cfg) {
                    println!("\n{}", "EOF — Goodbye! ᚱ".cyan());
                }
                break;
            }
            Err(e) => {
                eprintln!("{} {}", "Read error:".red(), e);
                break;
            }
        };

        match cmd.as_str() {
            "/exit" | "/quit" => {
                if !is_json_mode(&cfg) {
                    println!("{}", "Goodbye! ᚱ".cyan());
                }
                break;
            }
            "/help" | "/h" => print_help(),
            cmd if cmd.starts_with("/image ") || cmd.starts_with("/img ") => {
                let parts_str = cmd.splitn(2, ' ').nth(1).unwrap_or("");
                if parts_str.is_empty() {
                    eprintln!("  Usage: /image <path>");
                } else {
                    match load_image_as_base64(parts_str.trim()) {
                        Ok((b64, mime)) => {
                            pending_image_data = Some((b64, mime));
                            eprintln!(
                                "  {} Image attached: {} (send with next message)",
                                "📎".green(),
                                parts_str.trim()
                            );
                        }
                        Err(e) => eprintln!("  {} {}", "✗".red(), e),
                    }
                }
            }
            "/config" => show_config(&cfg),
            "/tools" => show_tools(),
            "/skills" => show_skills(&cfg),
            "/trace" => {
                println!("{} {}", "Trace dir:".bold(), ".rune/traces/");
                println!(
                    "  {} {}",
                    "status:".dimmed(),
                    if cfg.trace {
                        "enabled".green().to_string()
                    } else {
                        "disabled (use --trace to enable)".dimmed().to_string()
                    }
                );
            }
            cmd if cmd == "/thinking" || cmd.starts_with("/thinking ") => {
                let arg = cmd.strip_prefix("/thinking").unwrap().trim();
                if arg.is_empty() {
                    let current = agent.config.thinking.as_deref().unwrap_or("none");
                    println!("{} {}", "Thinking:".bold(), current.cyan());
                } else {
                    match arg {
                        "none" | "off" | "low" | "medium" | "high" | "xhigh" => {
                            agent.config.thinking = if arg == "none" || arg == "off" {
                                None
                            } else {
                                Some(arg.to_string())
                            };
                            println!("  {} Thinking set to: {}", "✓".green(), arg.cyan());
                        }
                        _ => {
                            eprintln!(
                                "  {} Invalid level. Use: off, low, medium, high, xhigh",
                                "⚠".yellow()
                            );
                        }
                    }
                }
            }
            "/version" => {
                println!("{} v{}", "Rune".cyan().bold(), VERSION);
                println!("  {} {}", "edition:".dimmed(), "2021");
                println!("  {} {}", "model:".dimmed(), cfg.model.green());
            }
            "/clear" => {
                print!("\x1B[2J\x1B[1;1H");
                print_banner();
            }
            "/reset" => {
                agent.reset();
                println!("{}", "Conversation reset.".green());
            }
            "/info" => show_info(&cfg, &agent),
            "/info context" => show_context(&agent),
            "/compact" => {
                let before = agent.message_count();
                agent.compact().await;
                let after = agent.message_count();
                println!(
                    "{} Context compacted: {} → {} messages",
                    "✓".green(),
                    before,
                    after
                );
            }
            "/policy" => show_policy_summary(&cfg),
            "/policy full" => show_policy_full(&cfg),
            cmd if cmd.starts_with("/add-dir ") => {
                let path_arg = cmd.strip_prefix("/add-dir ").unwrap().trim();
                let resolved = resolve_path(path_arg);
                if resolved.is_empty() {
                    eprintln!("{}", "Usage: /add-dir <path>".yellow());
                } else if agent.config.policy.allowed_paths_ro.contains(&resolved) {
                    eprintln!(
                        "  {} '{}' already in allowed_paths_ro",
                        "ℹ".cyan(),
                        resolved
                    );
                } else {
                    agent.config.policy.allowed_paths_ro.push(resolved.clone());
                    crate::config::persist_path_ro(&resolved);
                    eprintln!(
                        "  {} '{}' added to allowed_paths_ro (saved to config)",
                        "✓".green(),
                        resolved
                    );
                }
            }
            cmd if cmd.starts_with("/add-rw-dir ") => {
                let path_arg = cmd.strip_prefix("/add-rw-dir ").unwrap().trim();
                let resolved = resolve_path(path_arg);
                if resolved.is_empty() {
                    eprintln!("{}", "Usage: /add-rw-dir <path>".yellow());
                } else if agent.config.policy.allowed_paths_rw.contains(&resolved) {
                    eprintln!(
                        "  {} '{}' already in allowed_paths_rw",
                        "ℹ".cyan(),
                        resolved
                    );
                } else {
                    agent.config.policy.allowed_paths_rw.push(resolved.clone());
                    crate::config::persist_path_rw(&resolved);
                    eprintln!(
                        "  {} '{}' added to allowed_paths_rw (saved to config)",
                        "✓".green(),
                        resolved
                    );
                }
            }
            "/multi" => {
                if let Some(input) = read_multiline(editor) {
                    if !input.trim().is_empty() {
                        let _ = editor.add_history_entry(input.as_str());
                        let _ = execute_prompt(&mut agent, &input).await;
                    }
                }
            }
            _ => {
                let input = &cmd;
                if let Some((b64, mime)) = pending_image_data.take() {
                    use crate::provider::{ContentPart, ImageUrlDetail};
                    let url = format!("data:{};base64,{}", mime, b64);
                    let parts = vec![
                        ContentPart::Text {
                            text: input.to_string(),
                        },
                        ContentPart::ImageUrl {
                            image_url: ImageUrlDetail { url, detail: None },
                        },
                    ];
                    agent.push_user_message_with_parts(input.to_string(), parts);
                    let result = agent.run(input).await;
                    display_result(&result, true);
                } else {
                    let _ = execute_prompt(&mut agent, input).await;
                }
            }
        }
    }

    // Save persistent history
    if let Some(ref path) = history_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = editor.save_history(path);
    }
}
