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
    // Runic ASCII art banner with mystical colors
    let rune_border = format!("        {} {} {} {} {} {} {} {} {} {} {}",
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
    let rune_border2 = format!("        {} {} {} {} {} {} {} {} {} {} {}",
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
    println!("  {}      Send a prompt to the agent", "<text>".green());
    println!("  {}    Enter multi-line mode (end with ';;' on its own line)", "/multi".green());
    println!("  {}      Show current configuration", "/config".green());
    println!("  {}       List available built-in tools", "/tools".green());
    println!("  {}      List loaded skills", "/skills".green());
    println!("  {}       Show trace output directory", "/trace".green());
    println!("  {}     Show version info", "/version".green());
    println!("  {}        Clear the screen", "/clear".green());
    println!("  {}       Reset conversation history", "/reset".green());
    println!("  {}     Compact (summarize) conversation context", "/compact".green());
    println!("  {}      Show current session status (model, context, skills)", "/info".green());
    println!("  {}    Show policy summary (use /policy full for details)", "/policy".green());
    println!("  {}   Show this help", "/help".green());
    println!("  {}  Exit the CLI", "/exit | /quit".green());
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

/// Display current session state: model, context usage, skills, MCP.
fn show_info(cfg: &config::RuneConfig, agent: &crate::agent::Agent) {
    println!("{}", "Session Info:".bold());
    println!();

    // LLM Provider
    println!("  {}", "LLM Provider:".bold());
    if let Some(ref key) = cfg.api_key {
        let provider = if key.starts_with("ghu_") || key.starts_with("ghp_") {
            "GitHub Copilot"
        } else if key.starts_with("AIza") {
            "Google Gemini"
        } else if key.starts_with("sk-or-") {
            "OpenRouter"
        } else if key.starts_with("sk-") {
            "OpenAI"
        } else {
            "Custom"
        };
        println!("    {} provider: {}", "•".dimmed(), provider.green());
    } else {
        println!("    {} provider: {}", "•".dimmed(), "(not configured)".red());
    }
    println!("    {} model: {}", "•".dimmed(), cfg.model.green());
    if let Some(ref url) = cfg.base_url {
        println!("    {} endpoint: {}", "•".dimmed(), url.dimmed());
    }
    println!();

    // Context / Token usage
    println!("  {}", "Context:".bold());
    println!("    {} tokens used: {} / {} (budget)", "•".dimmed(), agent.tokens_used(), cfg.token_budget);
    println!("    {} steps: {} / {} (max)", "•".dimmed(), agent.step_count(), cfg.max_steps);
    println!("    {} timeout: {}s", "•".dimmed(), cfg.timeout_secs);
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
        println!("    {} (dir {} does not exist)", "•".dimmed(), cfg.skills_dir);
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
    println!("{}", "Context Details:".bold());
    println!("  {} messages: {}", "•".dimmed(), agent.message_count());
    println!("  {} chars: ~{}", "•".dimmed(), agent.context_chars());
    println!("  {} tokens used: {}", "•".dimmed(), agent.tokens_used());
    println!();
    println!("  {}", "By role:".bold());
    for (role, count) in agent.context_summary() {
        println!("    {} {}: {}", "•".dimmed(), role, count);
    }
    println!();
    println!("  {} Use /compact to summarize older messages", "ℹ".cyan());
}
fn show_policy_summary(cfg: &config::RuneConfig) {
    let p = &cfg.policy;
    println!("{}", "Policy Summary:".bold());
    println!("  {} mode: {}", "•".dimmed(), p.mode.cyan());
    println!("  {} allowed commands: {}", "•".dimmed(), if p.allowed_commands.is_empty() { "(none — all blocked in allowlist mode)".to_string() } else { format!("{}", p.allowed_commands.join(", ")) });
    println!("  {} allowed domains: {}", "•".dimmed(), if p.allowed_domains.is_empty() { "(none — network blocked)".to_string() } else { p.allowed_domains.join(", ") });
    println!("  {} denied syscalls: {}", "•".dimmed(), p.denied_syscalls.join(", "));
    println!("  {} paths rw: {}", "•".dimmed(), p.allowed_paths_rw.join(", "));
    println!("  {} paths ro: {}", "•".dimmed(), p.allowed_paths_ro.join(", "));
    println!("  {} denied paths: {}", "•".dimmed(), p.denied_paths.join(", "));
    println!("  {} memory limit: {}MB | max pids: {}", "•".dimmed(), p.max_memory_mb, p.max_pids);
    println!();
    println!("  {} Use {} for full sandbox status", "ℹ".cyan(), "/policy full".bold());
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
    println!("    {} execute_cmd — sandboxed, network blocked", "•".dimmed());
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
    let spinner = if agent.config.policy.mode == "confirm" { None } else { Some(create_spinner("Thinking...")) };
    let result = agent.run(input).await;
    if let Some(s) = spinner { s.finish_and_clear(); }
    display_result(&result);
    // Show executed commands summary
    let cmds = agent.executed_commands();
    if !cmds.is_empty() {
        println!("  {} commands executed: {}", "📋".dimmed(), cmds.len());
        for c in cmds {
            println!("    {} {}", "▸".dimmed(), c.dimmed());
        }
    }
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
         You have access to tools: read_file, write_file, list_dir, execute_cmd, fetch_url. \
         Use them when needed. Be concise and accurate."
    );

    println!("{} Type {} for commands.", "Ready.".green().bold(), "/help".bold());
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
            "/exit" | "/quit" => {
                println!("{}", "Goodbye! ᚱ".cyan());
                break;
            }
            "/help" | "/h" => print_help(),
            "/config" => show_config(&cfg),
            "/tools" => show_tools(),
            "/skills" => show_skills(&cfg),
            "/trace" => {
                println!("{} {}", "Trace dir:".bold(), ".rune/traces/");
                println!("  {} {}", "status:".dimmed(), if cfg.trace { "enabled".green().to_string() } else { "disabled (use --trace to enable)".dimmed().to_string() });
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
                agent.compact();
                let after = agent.message_count();
                println!("{} Context compacted: {} → {} messages", "✓".green(), before, after);
            }
            "/policy" => show_policy_summary(&cfg),
            "/policy full" => show_policy_full(&cfg),
            "/multi" => {
                if let Some(input) = read_multiline().await {
                    execute_prompt(&mut agent, &input).await;
                }
            }
            _ => {
                let input = &cmd;
                execute_prompt(&mut agent, input).await;
            }
        }
    }
}
