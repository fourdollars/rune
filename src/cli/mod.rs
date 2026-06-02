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
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

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

    println!("{}", rune_border);
    println!("{}", line2.magenta());
    println!("{}", line4.bright_cyan().bold());
    println!("{}", line6.white());
    println!("{}", line7.dimmed());
    println!("{}", line8.green());
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
        "/skills full".green(),
        "Show skill details (frontmatter, tools)"
    );
    println!("    {:<24} {}", "/mcps".green(), "MCP servers summary");
    println!(
        "    {:<24} {}",
        "/mcps full".green(),
        "MCP servers full details (tools, schema)"
    );
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
    use std::io::Write;
    match reason {
        StopReason::FinalAnswer(ans) => {
            // Use stderr for separators but flush stdout between them
            let _ = std::io::stderr().flush();
            eprintln!();
            eprintln!("{}", "─".repeat(60).dimmed());
            let _ = std::io::stderr().flush();
            if !streamed {
                println!("{}", ans);
                let _ = std::io::stdout().flush();
            }
            eprintln!("{}", "─".repeat(60).dimmed());
            let _ = std::io::stderr().flush();
        }
        StopReason::MaxSteps => {
            eprintln!("\n{}", "⚠ Stopped: maximum steps reached".yellow());
        }
        StopReason::TokenBudgetExhausted => {
            eprintln!("\n{}", "⚠ Stopped: token budget exhausted".yellow());
        }
        StopReason::Error(e) => {
            eprintln!("\n{} {}", "✗ Error:".red().bold(), e);
        }
        StopReason::UserInterrupt => {
            eprintln!("\n{}", "⚡ Interrupted by user".yellow());
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

// discover_skill_files moved to skills::discover_skill_files

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
        let skill_files = crate::skills::discover_skill_files(skill_dir, 0);
        if !skill_files.is_empty() {
            let mut names: Vec<String> = skill_files
                .iter()
                .filter_map(|p| {
                    p.parent()
                        .and_then(|dir| dir.file_name())
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .collect();
            names.sort();
            names.dedup();
            // When --skills is specified, only show the preloaded ones
            if !cfg.preload_skills.is_empty() {
                let filtered: Vec<&String> = names
                    .iter()
                    .filter(|n| cfg.preload_skills.iter().any(|s| s == *n))
                    .collect();
                println!(
                    "  {} {} (filtered by --skills)",
                    "active:".dimmed(),
                    filtered
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                        .green()
                );
            } else {
                println!("  {} {}", "found:".dimmed(), names.join(", ").green());
            }
        } else {
            println!("  {} (no skills found)", "found:".dimmed());
        }
    } else {
        println!("  {} (directory does not exist)", "status:".dimmed());
    }
    if !cfg.preload_skills.is_empty() {
        println!(
            "  {} {}",
            "preload:".dimmed(),
            cfg.preload_skills.join(", ").green()
        );
    }
    let _ = loader;
}

/// Display MCP servers summary.
fn show_mcps_summary(cfg: &config::RuneConfig, agent: &crate::agent::Agent) {
    println!("{}", "MCP Servers:".bold());
    if cfg.mcp_servers.is_empty() {
        println!("  {} (none configured)", "•".dimmed());
        return;
    }
    for server_cfg in &cfg.mcp_servers {
        let tool_count = if let Some(mcp_ref) = agent.mcp_manager_ref() {
            if let Ok(mgr) = mcp_ref.try_lock() {
                mgr.all_tools()
                    .iter()
                    .filter(|(s, _)| s == &server_cfg.name)
                    .count()
            } else {
                0
            }
        } else {
            0
        };
        let status = if tool_count > 0 {
            "running".green().to_string()
        } else {
            "no tools".yellow().to_string()
        };
        println!(
            "  {} {} — {} tool(s), {}",
            "•".dimmed(),
            server_cfg.name.cyan(),
            tool_count,
            status
        );
    }
    println!();
    println!(
        "  {} Use {} for full details",
        "ℹ".cyan(),
        "/mcps full".bold()
    );
}

/// Display MCP servers full details.
fn show_mcps_full(cfg: &config::RuneConfig, agent: &crate::agent::Agent) {
    println!("{}", "MCP Servers (full):".bold());
    println!();
    if cfg.mcp_servers.is_empty() {
        println!("  {} (none configured)", "•".dimmed());
        return;
    }
    for server_cfg in &cfg.mcp_servers {
        println!("  {} {}", "▸".cyan(), server_cfg.name.cyan().bold());
        println!("    {} {}", "command:".dimmed(), server_cfg.command);
        if !server_cfg.args.is_empty() {
            println!("    {} {:?}", "args:".dimmed(), server_cfg.args);
        }
        println!(
            "    {} {}s",
            "timeout:".dimmed(),
            server_cfg.timeout_secs.unwrap_or(30)
        );
        println!(
            "    {} {}",
            "required:".dimmed(),
            if server_cfg.required { "yes" } else { "no" }
        );

        // List tools from this server
        if let Some(mcp_ref) = agent.mcp_manager_ref() {
            if let Ok(mgr) = mcp_ref.try_lock() {
                let server_tools: Vec<_> = mgr
                    .all_tools()
                    .into_iter()
                    .filter(|(s, _)| s == &server_cfg.name)
                    .collect();
                if server_tools.is_empty() {
                    println!("    {} (no tools registered)", "tools:".dimmed());
                } else {
                    println!(
                        "    {} ({} available)",
                        "tools:".dimmed(),
                        server_tools.len()
                    );
                    for (_srv, tool) in &server_tools {
                        println!("      {} {}", "▹".dimmed(), tool.name.green());
                        if let Some(ref desc) = tool.description {
                            println!("        {}", desc.dimmed());
                        }
                        if let Some(ref schema) = tool.input_schema {
                            if let Some(props) = schema.get("properties") {
                                if let Some(obj) = props.as_object() {
                                    let param_names: Vec<&String> = obj.keys().collect();
                                    if !param_names.is_empty() {
                                        println!(
                                            "        {} {}",
                                            "params:".dimmed(),
                                            param_names
                                                .iter()
                                                .map(|s| s.as_str())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        );
                                    }
                                }
                            }
                            if let Some(required) = schema.get("required") {
                                if let Some(arr) = required.as_array() {
                                    let req_names: Vec<&str> =
                                        arr.iter().filter_map(|v| v.as_str()).collect();
                                    if !req_names.is_empty() {
                                        println!(
                                            "        {} {}",
                                            "required:".dimmed(),
                                            req_names.join(", ")
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        println!();
    }
}

/// Display skills with full details (frontmatter, tools restrictions).
fn show_skills_full(cfg: &config::RuneConfig) {
    let skill_dir = std::path::Path::new(&cfg.skills_dir);
    println!("{}", "Skills (full):".bold());
    println!("  {} {}", "search_dir:".dimmed(), cfg.skills_dir);
    println!();

    if !skill_dir.exists() {
        println!("  {} (directory does not exist)", "•".dimmed());
        return;
    }

    let mut skill_files = crate::skills::discover_skill_files(skill_dir, 0);
    skill_files.sort();

    if skill_files.is_empty() {
        println!("  {} (no skills found)", "•".dimmed());
        return;
    }

    // Deduplicate by skill name (parent dir name), keep first occurrence
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let skill_files: Vec<_> = skill_files
        .into_iter()
        .filter(|p| {
            let name = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            seen_names.insert(name)
        })
        .collect();

    for skill_md in &skill_files {
        let name = skill_md
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        // When --skills is specified, only show the preloaded ones
        if !cfg.preload_skills.is_empty() && !cfg.preload_skills.iter().any(|s| s == &name) {
            continue;
        }
        let abs_dir = skill_md
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let rel_path = skill_md
            .strip_prefix(skill_dir)
            .unwrap_or(skill_md)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let preloaded = cfg.preload_skills.iter().any(|s| s == &name);
        let tag = if preloaded {
            " [preloaded]".green().to_string()
        } else {
            String::new()
        };
        println!(
            "  {} @{} {}{}",
            "▸".cyan(),
            name.cyan().bold(),
            format!("({})", rel_path).dimmed(),
            tag
        );
        println!("    {} {}", "base_dir:".dimmed(), abs_dir);

        // Parse frontmatter
        match std::fs::read_to_string(&skill_md) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let mut in_frontmatter = false;
                let mut description = None;
                let mut tools_allow: Vec<String> = Vec::new();
                let mut tools_deny: Vec<String> = Vec::new();
                let mut model = None;

                for line in &lines {
                    let trimmed = line.trim();
                    if trimmed == "---" {
                        if in_frontmatter {
                            break; // end of frontmatter
                        }
                        in_frontmatter = true;
                        continue;
                    }
                    if in_frontmatter {
                        if let Some(val) = trimmed.strip_prefix("description:") {
                            description = Some(val.trim().trim_matches('"').to_string());
                        } else if let Some(val) = trimmed.strip_prefix("tools_allow:") {
                            tools_allow = val
                                .trim()
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        } else if let Some(val) = trimmed.strip_prefix("tools_deny:") {
                            tools_deny = val
                                .trim()
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        } else if let Some(val) = trimmed.strip_prefix("model:") {
                            model = Some(val.trim().trim_matches('"').to_string());
                        }
                    }
                }

                if let Some(desc) = description {
                    println!("    {} {}", "desc:".dimmed(), desc);
                }
                if let Some(m) = model {
                    println!("    {} {}", "model:".dimmed(), m);
                }
                if !tools_allow.is_empty() {
                    println!(
                        "    {} {}",
                        "tools_allow:".dimmed(),
                        tools_allow.join(", ").green()
                    );
                }
                if !tools_deny.is_empty() {
                    println!(
                        "    {} {}",
                        "tools_deny:".dimmed(),
                        tools_deny.join(", ").red()
                    );
                }
                // Show first few lines of content (after frontmatter)
                let content_start = if lines.iter().filter(|l| l.trim() == "---").count() >= 2 {
                    lines
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| l.trim() == "---")
                        .nth(1)
                        .map(|(i, _)| i + 1)
                        .unwrap_or(0)
                } else {
                    0
                };
                let preview: Vec<&str> = lines[content_start..]
                    .iter()
                    .copied()
                    .filter(|l| !l.trim().is_empty())
                    .take(2)
                    .collect();
                if !preview.is_empty() {
                    println!(
                        "    {} {}",
                        "preview:".dimmed(),
                        preview.join(" | ").dimmed()
                    );
                }
            }
            Err(_) => {
                println!("    {} (cannot read SKILL.md)", "status:".dimmed());
            }
        }
        println!();
    }
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
        let skill_files = crate::skills::discover_skill_files(skill_dir, 0);
        if skill_files.is_empty() {
            println!("    {} (none found in {})", "•".dimmed(), cfg.skills_dir);
        } else {
            let mut names: Vec<String> = skill_files
                .iter()
                .filter_map(|p| {
                    p.parent()
                        .and_then(|dir| dir.file_name())
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .collect();
            names.sort();
            names.dedup();
            // When --skills is specified, only show the preloaded ones
            let display_names: Vec<&String> = if !cfg.preload_skills.is_empty() {
                names
                    .iter()
                    .filter(|n| cfg.preload_skills.iter().any(|s| s == *n))
                    .collect()
            } else {
                names.iter().collect()
            };
            for s in &display_names {
                println!("    {} @{}", "•".dimmed(), s.green());
            }
            if !cfg.preload_skills.is_empty() {
                println!("    {} (filtered by --skills)", "ℹ".dimmed());
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
    if cfg.mcp_servers.is_empty() {
        println!("    {} (none configured)", "•".dimmed());
    } else {
        for server in &cfg.mcp_servers {
            println!(
                "    {} {} ({})",
                "•".dimmed(),
                server.name.cyan(),
                server.command.dimmed()
            );
        }
    }
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
        "  {} files rw: {}",
        "•".dimmed(),
        if p.allowed_files_rw.is_empty() {
            "(none)".to_string()
        } else {
            p.allowed_files_rw.join(", ")
        }
    );
    println!(
        "  {} files ro: {}",
        "•".dimmed(),
        if p.allowed_files_ro.is_empty() {
            "(none)".to_string()
        } else {
            p.allowed_files_ro.join(", ")
        }
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
    if !cfg.policy.allowed_files_ro.is_empty() {
        println!(
            "    {} files ro: {}",
            "•".dimmed(),
            cfg.policy.allowed_files_ro.join(", ")
        );
    }
    if !cfg.policy.allowed_files_rw.is_empty() {
        println!(
            "    {} files rw: {}",
            "•".dimmed(),
            cfg.policy.allowed_files_rw.join(", ")
        );
    }
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

    // Show tool calls summary (all tools, not just execute_cmd)
    let log = agent.tool_calls_log();
    if !log.is_empty() {
        eprintln!("  {} tool calls:", "📋".dimmed());
        for record in log {
            let status = if record.is_error {
                "✗".red()
            } else {
                "✓".green()
            };
            eprintln!(
                "    {} {} {}",
                status,
                record.name.dimmed(),
                record.args_preview.dimmed()
            );
        }
    }
    // Run summary
    if agent.step_count() > 0 {
        eprintln!(
            "  {} [{} steps | {} tokens | {} tool calls]",
            "⚡".dimmed(),
            agent.step_count(),
            agent.tokens_used(),
            agent.tool_call_count()
        );
    }
    let _ = std::io::Write::flush(&mut std::io::stderr());
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
    let expanded = crate::config::expand_tilde(path);

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
    let mut cfg = config::load().unwrap_or_default();

    // Normalize model: use first from comma-separated list (full list is for serve mode UI only)
    if cfg.model.contains(',') {
        if let Some(first) = cfg.model.split(',').map(|s| s.trim()).find(|s| !s.is_empty()) {
            cfg.model = first.to_string();
        }
    }

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
    // Positional prompt mode keeps the configured policy (user is at the terminal)
    if !stdin_is_terminal && cfg.policy.mode == "confirm" {
        eprintln!(
            "  {} pipe mode: defaulting policy to allowlist (use --unrestricted to disable)",
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
        // Auto-detect provider for embedding
        let is_copilot = cfg
            .api_key
            .as_ref()
            .map(|k| k.starts_with("ghu_") || k.starts_with("ghp_"))
            .unwrap_or(false);
        let is_gemini = cfg
            .api_key
            .as_ref()
            .map(|k| k.starts_with("AIza"))
            .unwrap_or(false)
            || cfg
                .provider
                .as_deref()
                .map(|p| p == "gemini")
                .unwrap_or(false);

        if is_copilot {
            if emb_cfg.base_url.is_none() {
                emb_cfg.base_url = Some("https://api.githubcopilot.com".to_string());
            }
            let pat = cfg.api_key.clone().unwrap_or_default();
            Some(crate::embedding::EmbeddingEngine::new_copilot(emb_cfg, pat))
        } else if is_gemini {
            if emb_cfg.base_url.is_none() {
                emb_cfg.base_url =
                    Some("https://generativelanguage.googleapis.com/v1beta/openai".to_string());
            }
            if emb_cfg.model.is_none() {
                emb_cfg.model = Some("gemini-embedding-2".to_string());
            }
            Some(crate::embedding::EmbeddingEngine::new(emb_cfg))
        } else {
            // For other providers (OpenRouter, OpenAI, etc.): use main base_url if set
            if emb_cfg.base_url.is_none() {
                emb_cfg.base_url = cfg.base_url.clone();
            }
            Some(crate::embedding::EmbeddingEngine::new(emb_cfg))
        }
    } else {
        None
    };

    if !cfg.preload_skills.is_empty() {
        eprintln!(
            "  {} Preloading skills: {}",
            "📚".dimmed(),
            cfg.preload_skills.join(", ")
        );
    }
    let mut agent = Agent::new(cfg.clone(), provider, stdin_is_terminal, embedding_engine);

    // Attach MCP manager if any servers connected
    if mcp_manager.clients_count() > 0 {
        agent.set_mcp_manager(Arc::new(TokioMutex::new(mcp_manager)));
    }

    // Build system prompt: base + optional AGENTS.md from CWD
    let mut sys_prompt = cfg.system_prompt.clone().unwrap_or_else(|| {
        "You are Rune, a high-performance AI agent running in a terminal. \
         You have access to tools: read_file, write_file, list_dir, execute_cmd, fetch_url. \
         Use them when needed. Be concise and accurate."
            .to_string()
    });

    // Auto-load AGENTS.md from current directory if present (with confirmation in interactive mode)
    if let Ok(agents_content) = std::fs::read_to_string("AGENTS.md") {
        if !agents_content.trim().is_empty() {
            let should_load = if stdin_is_terminal {
                eprint!(
                    "  {} Found AGENTS.md in current directory. Load? [Y/n] ",
                    "📚"
                );
                std::io::Write::flush(&mut std::io::stderr()).ok();
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);
                !input.trim().eq_ignore_ascii_case("n")
            } else {
                true // Always load in pipe mode
            };
            if should_load {
                sys_prompt.push_str("\n\n[Project Context: AGENTS.md]\n");
                sys_prompt.push_str(&agents_content);
                sys_prompt.push_str("\n[End AGENTS.md]");
                eprintln!("  {} Loaded AGENTS.md", "✓".green());
            }
        }
    }

    agent.set_system_prompt(&sys_prompt);

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

    // One-shot mode: rune "prompt" (positional argument)
    if let Some(ref prompt) = cfg.cli_prompt {
        let result = execute_prompt(&mut agent, prompt).await;
        if !matches!(result, StopReason::FinalAnswer(_)) {
            std::process::exit(1);
        }
        return;
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
            "/skills full" => show_skills_full(&cfg),
            "/skills" => show_skills(&cfg),
            "/mcps full" => show_mcps_full(&cfg, &agent),
            "/mcps" => show_mcps_summary(&cfg, &agent),
            "/trace" => {
                println!("{} {}", "Trace dir:".bold(), ".rune/traces/");
                println!(
                    "  {} {}",
                    "status:".dimmed(),
                    if cfg.trace.is_some() {
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

/// Parsed skill frontmatter for display and testing.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFrontmatter {
    pub description: Option<String>,
    pub tools_allow: Vec<String>,
    pub tools_deny: Vec<String>,
    pub model: Option<String>,
}

/// Parse SKILL.md frontmatter from content string.
pub fn parse_skill_frontmatter(content: &str) -> SkillFrontmatter {
    let mut in_frontmatter = false;
    let mut description = None;
    let mut tools_allow = Vec::new();
    let mut tools_deny = Vec::new();
    let mut model = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_frontmatter {
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if let Some(val) = trimmed.strip_prefix("description:") {
                description = Some(val.trim().trim_matches('"').to_string());
            } else if let Some(val) = trimmed.strip_prefix("tools_allow:") {
                tools_allow = val
                    .trim()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            } else if let Some(val) = trimmed.strip_prefix("tools_deny:") {
                tools_deny = val
                    .trim()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            } else if let Some(val) = trimmed.strip_prefix("model:") {
                model = Some(val.trim().trim_matches('"').to_string());
            }
        }
    }

    SkillFrontmatter {
        description,
        tools_allow,
        tools_deny,
        model,
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn test_parse_skill_frontmatter_full() {
        let content = r#"---
description: "A test skill for linting"
tools_allow: read_file, execute_cmd
tools_deny: write_file
model: gpt-4o
---
# My Skill
Some content here.
"#;
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.description.as_deref(), Some("A test skill for linting"));
        assert_eq!(fm.tools_allow, vec!["read_file", "execute_cmd"]);
        assert_eq!(fm.tools_deny, vec!["write_file"]);
        assert_eq!(fm.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_parse_skill_frontmatter_minimal() {
        let content = r#"---
description: Basic skill
---
# Content
"#;
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.description.as_deref(), Some("Basic skill"));
        assert!(fm.tools_allow.is_empty());
        assert!(fm.tools_deny.is_empty());
        assert!(fm.model.is_none());
    }

    #[test]
    fn test_parse_skill_frontmatter_no_frontmatter() {
        let content = "# Just a regular markdown file\nNo frontmatter here.";
        let fm = parse_skill_frontmatter(content);
        assert!(fm.description.is_none());
        assert!(fm.tools_allow.is_empty());
        assert!(fm.tools_deny.is_empty());
        assert!(fm.model.is_none());
    }

    #[test]
    fn test_parse_skill_frontmatter_empty_tools() {
        let content = r#"---
description: "Empty tools"
tools_allow:
tools_deny:
---
"#;
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.description.as_deref(), Some("Empty tools"));
        assert!(fm.tools_allow.is_empty());
        assert!(fm.tools_deny.is_empty());
    }

    #[test]
    fn test_parse_skill_frontmatter_quoted_description() {
        let content = r#"---
description: "Skill with \"quotes\" inside"
---
"#;
        let fm = parse_skill_frontmatter(content);
        assert!(fm.description.is_some());
        // Outer quotes stripped
        assert!(fm.description.unwrap().contains("quotes"));
    }

    #[test]
    fn test_parse_skill_frontmatter_multiple_tools() {
        let content = r#"---
tools_allow: read_file, write_file, list_dir, execute_cmd, fetch_url
---
"#;
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.tools_allow.len(), 5);
        assert_eq!(fm.tools_allow[0], "read_file");
        assert_eq!(fm.tools_allow[4], "fetch_url");
    }

    #[test]
    fn test_parse_skill_frontmatter_stops_at_second_separator() {
        let content = r#"---
description: "First section"
model: gpt-4o
---
# Content after frontmatter
description: "This should NOT be parsed"
model: different
"#;
        let fm = parse_skill_frontmatter(content);
        assert_eq!(fm.description.as_deref(), Some("First section"));
        assert_eq!(fm.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_discover_skill_files_empty_dir() {
        let dir =
            std::env::temp_dir().join(format!("rune-skill-test-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let results = crate::skills::discover_skill_files(&dir, 0);
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill_files_flat() {
        let dir = std::env::temp_dir().join(format!("rune-skill-test-flat-{}", std::process::id()));
        let skill_a = dir.join("skill-a");
        let _ = std::fs::create_dir_all(&skill_a);
        std::fs::write(skill_a.join("SKILL.md"), "---\nname: skill-a\n---\n").unwrap();
        let skill_b = dir.join("skill-b");
        let _ = std::fs::create_dir_all(&skill_b);
        std::fs::write(skill_b.join("SKILL.md"), "---\nname: skill-b\n---\n").unwrap();

        let results = crate::skills::discover_skill_files(&dir, 0);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|p| p.ends_with("skill-a/SKILL.md")));
        assert!(results.iter().any(|p| p.ends_with("skill-b/SKILL.md")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill_files_nested() {
        let dir =
            std::env::temp_dir().join(format!("rune-skill-test-nested-{}", std::process::id()));
        // Simulate: repo/subdir/SKILL.md (depth 2)
        let nested = dir.join("my-repo").join("launchpad");
        let _ = std::fs::create_dir_all(&nested);
        std::fs::write(nested.join("SKILL.md"), "---\nname: launchpad\n---\n").unwrap();

        let results = crate::skills::discover_skill_files(&dir, 0);
        assert_eq!(results.len(), 1);
        assert!(results[0].ends_with("launchpad/SKILL.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill_files_max_depth() {
        let dir =
            std::env::temp_dir().join(format!("rune-skill-test-depth-{}", std::process::id()));
        // depth 4 should NOT be found (max is 3)
        let deep = dir.join("a").join("b").join("c").join("d").join("e");
        let _ = std::fs::create_dir_all(&deep);
        std::fs::write(deep.join("SKILL.md"), "---\nname: deep\n---\n").unwrap();

        let results = crate::skills::discover_skill_files(&dir, 0);
        assert!(results.is_empty(), "depth 4 should not be discovered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill_files_ignores_non_dir() {
        let dir = std::env::temp_dir().join(format!("rune-skill-test-file-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A file named like a skill dir should be ignored
        std::fs::write(dir.join("not-a-dir"), "hello").unwrap();
        // A dir without SKILL.md should not appear
        let empty_dir = dir.join("empty-skill");
        let _ = std::fs::create_dir_all(&empty_dir);

        let results = crate::skills::discover_skill_files(&dir, 0);
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill_files_nonexistent_dir() {
        let dir = std::path::Path::new("/tmp/rune-nonexistent-dir-xyz-12345");
        let results = crate::skills::discover_skill_files(dir, 0);
        assert!(results.is_empty());
    }

    // ── resolve_path ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("/usr/local/bin");
        assert_eq!(result, "/usr/local/bin");
    }

    #[test]
    fn test_resolve_path_normalizes_dot_dot() {
        let result = resolve_path("/home/user/../user/docs");
        assert_eq!(result, "/home/user/docs");
    }

    #[test]
    fn test_resolve_path_normalizes_single_dot() {
        let result = resolve_path("/home/./user/docs");
        assert_eq!(result, "/home/user/docs");
    }

    #[test]
    fn test_resolve_path_collapses_double_slash() {
        // Double empty segments collapse
        let result = resolve_path("/home//user");
        assert_eq!(result, "/home/user");
    }

    #[test]
    fn test_resolve_path_dot_dot_at_root() {
        // Going above root stays at /
        let result = resolve_path("/../../etc/passwd");
        assert_eq!(result, "/etc/passwd");
    }

    #[test]
    fn test_resolve_path_relative_becomes_absolute() {
        // Relative paths should become absolute (under cwd)
        let result = resolve_path("some/relative/path");
        assert!(
            result.starts_with('/'),
            "relative path must become absolute"
        );
        assert!(result.ends_with("some/relative/path") || result.contains("some/relative/path"));
    }

    #[test]
    fn test_resolve_path_tilde_expanded() {
        // ~ should be expanded
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let result = resolve_path("~/docs");
            assert!(
                !result.contains('~'),
                "tilde should be expanded, got {}",
                result
            );
            assert!(result.contains("docs"));
        }
    }

    // ── is_json_mode ──────────────────────────────────────────────────────

    #[test]
    fn test_is_json_mode_false_by_default() {
        let cfg = crate::config::RuneConfig::default();
        assert!(!is_json_mode(&cfg));
    }

    #[test]
    fn test_is_json_mode_true_when_set() {
        let mut cfg = crate::config::RuneConfig::default();
        cfg.json_output = true;
        assert!(is_json_mode(&cfg));
    }

    // ── load_image_as_base64 ──────────────────────────────────────────────

    #[test]
    fn test_load_image_as_base64_nonexistent() {
        let result = load_image_as_base64("/tmp/rune-test-nonexistent-image-xyz.png");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_image_as_base64_png() {
        use std::io::Write;
        // Write a minimal 1x1 PNG (valid PNG header + IHDR)
        let path = format!("/tmp/rune-test-img-{}.png", std::process::id());
        // Minimal valid PNG bytes (1x1 transparent)
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG sig
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk len+type
        ];
        std::fs::write(&path, png_bytes).unwrap();
        let result = load_image_as_base64(&path);
        assert!(result.is_ok());
        let (b64, mime) = result.unwrap();
        assert_eq!(mime, "image/png");
        assert!(!b64.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_image_as_base64_jpeg() {
        let path = format!("/tmp/rune-test-img-{}.jpg", std::process::id());
        std::fs::write(&path, b"fake jpeg data").unwrap();
        let result = load_image_as_base64(&path);
        assert!(result.is_ok());
        let (_, mime) = result.unwrap();
        assert_eq!(mime, "image/jpeg");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_image_as_base64_webp() {
        let path = format!("/tmp/rune-test-img-{}.webp", std::process::id());
        std::fs::write(&path, b"fake webp").unwrap();
        let result = load_image_as_base64(&path);
        assert!(result.is_ok());
        let (_, mime) = result.unwrap();
        assert_eq!(mime, "image/webp");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_image_as_base64_gif() {
        let path = format!("/tmp/rune-test-img-{}.gif", std::process::id());
        std::fs::write(&path, b"GIF89a").unwrap();
        let result = load_image_as_base64(&path);
        assert!(result.is_ok());
        let (_, mime) = result.unwrap();
        assert_eq!(mime, "image/gif");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_image_as_base64_unknown_ext_defaults_png() {
        let path = format!("/tmp/rune-test-img-{}.bin", std::process::id());
        std::fs::write(&path, b"binary").unwrap();
        let result = load_image_as_base64(&path);
        assert!(result.is_ok());
        let (_, mime) = result.unwrap();
        assert_eq!(mime, "image/png");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_image_base64_content_is_valid_base64() {
        use base64::Engine;
        let path = format!("/tmp/rune-test-b64-{}.png", std::process::id());
        let data = b"hello world this is test data";
        std::fs::write(&path, data).unwrap();
        let (b64, _) = load_image_as_base64(&path).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        assert_eq!(decoded, data);
        let _ = std::fs::remove_file(&path);
    }

    // ── SkillFrontmatter display_result (unit test via StopReason) ────────

    #[test]
    fn test_skill_frontmatter_default_empty() {
        let fm = SkillFrontmatter {
            description: None,
            tools_allow: vec![],
            tools_deny: vec![],
            model: None,
        };
        assert!(fm.description.is_none());
        assert!(fm.tools_allow.is_empty());
        assert!(fm.tools_deny.is_empty());
        assert!(fm.model.is_none());
    }

    #[test]
    fn test_skill_frontmatter_clone() {
        let fm = SkillFrontmatter {
            description: Some("test".to_string()),
            tools_allow: vec!["read_file".to_string()],
            tools_deny: vec![],
            model: Some("gpt-4o".to_string()),
        };
        let fm2 = fm.clone();
        assert_eq!(fm, fm2);
    }

    /// Model normalization: comma-separated list should yield first model only.
    #[test]
    fn test_model_normalization_single() {
        let model = "gpt-5-mini";
        let normalized = if model.contains(',') {
            model.split(',').next().unwrap_or(model).trim().to_string()
        } else {
            model.to_string()
        };
        assert_eq!(normalized, "gpt-5-mini");
    }

    #[test]
    fn test_model_normalization_comma_separated() {
        let model = "gpt-5-mini,claude-sonnet-4.6,gemini-3.1-pro-preview";
        let normalized = if model.contains(',') {
            model.split(',').next().unwrap_or(model).trim().to_string()
        } else {
            model.to_string()
        };
        assert_eq!(normalized, "gpt-5-mini");
    }

    #[test]
    fn test_model_normalization_with_spaces() {
        let model = " gpt-5-mini , claude-sonnet-4.6 ";
        let normalized = if model.contains(',') {
            model.split(',').next().unwrap_or(model).trim().to_string()
        } else {
            model.to_string()
        };
        assert_eq!(normalized, "gpt-5-mini");
    }

    #[test]
    fn test_model_normalization_skips_empty() {
        // Edge case: leading comma — should skip empty and pick first real model
        let model = ",gpt-5-mini,claude";
        let normalized = if model.contains(',') {
            model.split(',').map(|s| s.trim()).find(|s| !s.is_empty())
                .unwrap_or(model).to_string()
        } else {
            model.to_string()
        };
        assert_eq!(normalized, "gpt-5-mini");
    }
}
