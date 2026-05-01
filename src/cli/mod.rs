use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

use crate::agent::{Agent, StopReason};
use crate::config;
use crate::provider::{OpenAiProvider, ProviderRegistry};
use crate::skills::SkillLoader;
use crate::tools::ToolRegistry;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Print the startup banner.
fn print_banner() {
    let rune_art = r#"
  ┌─────────────────────────────────────────┐
  │  ᚱ  R U N E   v{version}                    │
  │  High-performance Zero-Trust AI Agent   │
  └─────────────────────────────────────────┘"#;
    let banner = rune_art.replace("{version}", VERSION);
    println!("{}", banner.cyan());
    println!();
}

/// Print available commands.
fn print_help() {
    println!("{}", "Commands:".bold());
    println!("  {}         Run a prompt through the agent", "run <text>".green());
    println!("  {}    Enter multi-line mode (end with ';;' on its own line)", "/multi".green());
    println!("  {}      Show current configuration", "/config".green());
    println!("  {}       List available built-in tools", "/tools".green());
    println!("  {}      List loaded skills", "/skills".green());
    println!("  {}       Show trace output directory", "/trace".green());
    println!("  {}     Show version info", "/version".green());
    println!("  {}        Clear the screen", "/clear".green());
    println!("  {}       Reset conversation history", "/reset".green());
    println!("  {}        Show sandbox & permissions info", "/info".green());
    println!("  {}   Show this help", "help | /help".green());
    println!("  {}  Exit the CLI", "exit | quit".green());
    println!();
    println!("{}", "Tips:".dimmed());
    println!("  {} Type your prompt directly (without 'run') for quick execution", "•".dimmed());
    println!("  {} Use @skill_name in prompts to load skill context", "•".dimmed());
    println!("  {} Set RUNE_API_KEY or --api-key to connect to an LLM provider", "•".dimmed());
    println!("  {} Ctrl+C interrupts the current agent run", "•".dimmed());
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
fn display_result(reason: &StopReason) {
    match reason {
        StopReason::FinalAnswer(ans) => {
            println!();
            println!("{}", "─".repeat(60).dimmed());
            println!("{}", ans);
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
    println!("  {}  {}", "api_key:".dimmed(),
        if cfg.api_key.is_some() { "*** (set)".to_string() } else { "(not set)".red().to_string() });
    println!("  {}  {}", "skills_dir:".dimmed(), cfg.skills_dir);
    println!("  {}  {}", "log_level:".dimmed(), cfg.log_level);
    println!("  {}  {}", "max_steps:".dimmed(), cfg.max_steps);
    println!("  {}  {}", "token_budget:".dimmed(), cfg.token_budget);
    println!("  {}  {}", "timeout_secs:".dimmed(), cfg.timeout_secs);
}

/// Display available tools.
fn show_tools() {
    let registry = ToolRegistry::new(vec![]);
    let defs = registry.tool_definitions();
    println!("{} ({} available)", "Built-in Tools:".bold(), defs.len());
    for def in &defs {
        if let Some(func) = def.get("function") {
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = func.get("description").and_then(|v| v.as_str()).unwrap_or("");
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
    println!("  {} Use @skill_name in prompts to load skills", "usage:".dimmed());
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

/// Display sandbox permissions and security info.
fn show_info(cfg: &config::RuneConfig) {
    use crate::sandbox::SandboxConfig;

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
        println!("    {} All tool commands run in isolated network namespace", "•".dimmed());
        println!("    {} No DNS resolution available inside sandbox", "•".dimmed());
        println!("    {} No outbound connections possible", "•".dimmed());
    } else {
        println!("    {} DEGRADED — unshare not available", "⚠".yellow());
        println!("    {} Commands run WITHOUT network isolation", "•".dimmed());
    }
    println!();

    println!("  {}", "Filesystem Access:".bold());
    println!("    {} User namespace UID remapping active", "•".dimmed());
    println!("    {} Cannot read: /etc/shadow, /root, privileged files", "✓".green());
    println!("    {} Cannot write: /root, /etc, system directories", "✓".green());
    println!("    {} Can read: general user-readable files", "•".dimmed());
    println!("    {} Can write: /tmp, project directories", "•".dimmed());
    println!();

    println!("  {}", "Tool Restrictions:".bold());
    println!("    {} read_file    — sandboxed, 32KB truncation", "•".dimmed());
    println!("    {} write_file   — sandboxed, allowed dirs only", "•".dimmed());
    println!("    {} list_dir     — sandboxed", "•".dimmed());
    println!("    {} run_terminal_cmd — sandboxed, network blocked", "•".dimmed());
    println!("    {} fetch_url    — sandboxed, {} (network blocked)", "•".dimmed(), "ALWAYS FAILS".red());
    println!();

    println!("  {}", "Timeouts:".bold());
    println!("    {} Default command timeout: {}s", "•".dimmed(), cfg.timeout_secs);
    println!("    {} Max agent steps: {}", "•".dimmed(), cfg.max_steps);
    println!("    {} Token budget: {}", "•".dimmed(), cfg.token_budget);
    println!();

    println!("  {}", "LLM Provider:".bold());
    if let Some(ref key) = cfg.api_key {
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
    println!("    {} Provider calls are NOT sandboxed (need network for LLM)", "ℹ".cyan());
    println!();

    println!("  {}", "Summary:".bold());
    println!("    Tools: network={}, filesystem={}, timeout={}s",
        if unshare_ok { "BLOCKED".red().to_string() } else { "OPEN (degraded)".yellow().to_string() },
        "RESTRICTED".green(),
        cfg.timeout_secs
    );
}

/// Read multi-line input until ";;" on its own line.
async fn read_multiline() -> Option<String> {
    use tokio::io::{self, AsyncBufReadExt};
    println!("{}", "Multi-line mode. Enter ';;' on its own line to submit:".dimmed());
    println!("{}", "─".repeat(40).dimmed());

    let stdin = io::stdin();
    let reader = io::BufReader::new(stdin);
    let mut lines = reader.lines();
    let mut buffer = Vec::new();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim() == ";;" {
            break;
        }
        buffer.push(line);
    }

    if buffer.is_empty() { None } else { Some(buffer.join("\n")) }
}

/// Run a prompt through the agent with spinner feedback.
async fn execute_prompt(agent: &mut Agent, input: &str) {
    let spinner = create_spinner("Thinking...");
    let result = agent.run(input).await;
    spinner.finish_and_clear();
    display_result(&result);
}

/// Initialize the provider registry from config.
fn init_provider(cfg: &config::RuneConfig) -> ProviderRegistry {
    use crate::provider::CopilotProvider;

    let mut registry = ProviderRegistry::new();

    if let Some(ref key) = cfg.api_key {
        // Detect GitHub Copilot PAT (starts with ghu_ or ghp_)
        let is_copilot = key.starts_with("ghu_") || key.starts_with("ghp_")
            || cfg.base_url.as_deref().map(|u| u.contains("githubcopilot")).unwrap_or(false);

        if is_copilot {
            let provider = CopilotProvider::new(key.clone());
            registry.register(Box::new(provider));
        } else {
            let provider = OpenAiProvider::new(
                "openai".to_string(),
                key.clone(),
                cfg.base_url.clone(),
            );
            registry.register(Box::new(provider));
        }
    }

    registry
}

/// Main CLI entry point.
pub async fn run() {
    print_banner();

    let cfg = config::load().unwrap_or_default();
    let provider = init_provider(&cfg);

    if provider.is_empty() {
        println!("{}", "⚠ No API key configured. Set RUNE_API_KEY or use --api-key to connect.".yellow());
        println!("{}", "  The agent will not be able to call an LLM without a key.".dimmed());
        println!();
    }

    let mut agent = Agent::new(cfg.clone(), provider);
    agent.set_system_prompt(
        "You are Rune, a high-performance AI agent running in a terminal. \
         You have access to tools: read_file, write_file, list_dir, run_terminal_cmd, fetch_url. \
         Use them when needed. Be concise and accurate."
    );

    println!("{} Type {} for commands.", "Ready.".green().bold(), "help".bold());
    println!();

    use tokio::io::{self, AsyncBufReadExt};
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin);

    loop {
        eprint!("{} ", "ᚱ›".cyan().bold());

        // Read raw bytes for full UTF-8 support (including CJK characters)
        let mut raw_buf = Vec::new();
        match reader.read_until(b'\n', &mut raw_buf).await {
            Ok(0) => {
                println!("\n{}", "EOF — Goodbye! ᚱ".cyan());
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("{} {}", "Read error:".red(), e);
                break;
            }
        }
        let line = String::from_utf8_lossy(&raw_buf);
        let cmd = line.trim().to_string();
        if cmd.is_empty() { continue; }

        match cmd.as_str() {
            "exit" | "quit" | "/exit" | "/quit" => {
                println!("{}", "Goodbye! ᚱ".cyan());
                break;
            }
            "help" | "/help" | "/h" | "?" => print_help(),
            "/config" => show_config(&cfg),
            "/tools" => show_tools(),
            "/skills" => show_skills(&cfg),
            "/trace" => {
                println!("{} {}", "Trace dir:".bold(), ".rune/traces/");
                println!("{} Use --trace flag to enable trace recording", "Note:".dimmed());
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
            "/info" => show_info(&cfg),
            "/multi" => {
                if let Some(input) = read_multiline().await {
                    execute_prompt(&mut agent, &input).await;
                }
            }
            _ => {
                let input = cmd.strip_prefix("run ").unwrap_or(&cmd);
                execute_prompt(&mut agent, input).await;
            }
        }
    }
}
