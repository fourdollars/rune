use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Unified sandbox/security policy.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// Execution mode: "confirm" | "allowlist" | "unrestricted"
    pub mode: String,
    /// Commands allowed to execute (only enforced in "allowlist" mode).
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// Network domains allowed (empty = block all).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Syscalls to deny via seccomp.
    #[serde(default)]
    pub denied_syscalls: Vec<String>,
    /// Paths with read-write access.
    #[serde(default)]
    pub allowed_paths_rw: Vec<String>,
    /// Paths with read-only access.
    #[serde(default)]
    pub allowed_paths_ro: Vec<String>,
    /// Paths explicitly denied.
    #[serde(default)]
    pub denied_paths: Vec<String>,
    /// Memory limit in MB (0 = no limit).
    #[serde(default)]
    pub max_memory_mb: u64,
    /// Max child processes (0 = no limit).
    #[serde(default)]
    pub max_pids: u32,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: "confirm".to_string(),
            allowed_commands: Vec::new(),
            allowed_domains: Vec::new(),
            denied_syscalls: vec![
                "ptrace".to_string(),
                "mount".to_string(),
                "kexec_load".to_string(),
                "bpf".to_string(),
                "setns".to_string(),
            ],
            allowed_paths_rw: vec!["/tmp".to_string()],
            allowed_paths_ro: vec![
                "/bin".to_string(),
                "/usr".to_string(),
                "/lib".to_string(),
            ],
            denied_paths: vec![
                "/root".to_string(),
                "/etc/shadow".to_string(),
            ],
            max_memory_mb: 512,
            max_pids: 64,
        }
    }
}

/// Rune runtime configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct RuneConfig {
    pub model: String,
    pub api_key: Option<String>,
    pub skills_dir: String,
    pub log_level: String,
    pub max_steps: u32,
    pub token_budget: u32,
    pub timeout_secs: u64,
    pub base_url: Option<String>,
    pub trace: bool,
    pub json_output: bool,
    pub auto_approve: bool,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
}

impl Default for RuneConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            api_key: None,
            skills_dir: "./skills".to_string(),
            log_level: "info".to_string(),
            max_steps: 100,
            token_budget: 4096,
            timeout_secs: 60,
            base_url: None,
            trace: false,
            json_output: false,
            auto_approve: false,
            policy: PolicyConfig::default(),
            mcp_servers: Vec::new(),
        }
    }
}

/// Partial config for layered merging.
#[derive(Debug, Deserialize, Default)]
struct PartialConfig {
    model: Option<String>,
    api_key: Option<String>,
    skills_dir: Option<String>,
    log_level: Option<String>,
    max_steps: Option<u32>,
    token_budget: Option<u32>,
    timeout_secs: Option<u64>,
    base_url: Option<String>,
    trace: Option<bool>,
    policy: Option<PolicyConfig>,
    mcp_servers: Option<Vec<crate::mcp::McpServerConfig>>,
}

/// CLI argument overrides.
#[derive(Debug, clap::Parser)]
#[command(
    name = "rune",
    version,
    about = "ᚱ Rune — High-performance zero-trust AI Agent",
    long_about = "ᚱ Rune — High-performance zero-trust AI Agent\n\n\
        Single binary, dual mode: interactive CLI assistant and Concourse CI resource type.\n\
        All tool executions are sandboxed with network isolation (unshare --user --net).\n\n\
        SUBCOMMANDS:\n  \
        rune init    Interactive setup wizard to configure LLM provider\n\n\
        EXAMPLES:\n  \
        rune                 Start interactive CLI\n  \
        rune init            Run setup wizard\n  \
        rune --model gpt-4o  Start with a specific model\n\n\
        CONFIG PRECEDENCE:\n  \
        CLI flags > env vars (RUNE_*) > .rune/rune.toml > ~/.rune/rune.toml > defaults"
)]
struct CliArgs {
    /// LLM model name (e.g. gpt-4o-mini, anthropic/claude-3.5-sonnet)
    #[arg(long, env = "RUNE_MODEL")]
    model: Option<String>,

    /// LLM provider API key
    #[arg(long, env = "RUNE_API_KEY")]
    api_key: Option<String>,

    /// Provider base URL (default: https://api.openai.com/v1)
    #[arg(long, env = "RUNE_BASE_URL")]
    base_url: Option<String>,

    /// Directory containing skill definitions
    #[arg(long, env = "RUNE_SKILLS_DIR")]
    skills_dir: Option<String>,

    /// Log level: trace, debug, info, warn, error
    #[arg(long, env = "RUNE_LOG_LEVEL")]
    log_level: Option<String>,

    /// Maximum agent loop iterations per run
    #[arg(long, env = "RUNE_MAX_STEPS")]
    max_steps: Option<u32>,

    /// Maximum tokens per run
    #[arg(long, env = "RUNE_TOKEN_BUDGET")]
    token_budget: Option<u32>,

    /// Default command timeout in seconds
    #[arg(long, env = "RUNE_TIMEOUT_SECS")]
    timeout_secs: Option<u64>,

    /// Enable trace recording to .rune/traces/
    #[arg(long, env = "RUNE_TRACE")]
    trace: Option<bool>,

    /// Policy mode: confirm, allowlist, or unrestricted
    #[arg(long, env = "RUNE_POLICY_MODE")]
    policy_mode: Option<String>,

    /// Output in JSON format (machine-readable)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    json: bool,

    /// Auto-approve tool execution prompts (does not bypass policy allowlist checks)
    #[arg(long, short = 'y', action = clap::ArgAction::SetTrue)]
    yes: bool,
}

fn parse_boolish(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

/// Pick the first Some value from a chain of options, falling back to a default.
fn pick<T: Clone>(sources: &[&Option<T>], default: T) -> T {
    for src in sources {
        if let Some(v) = src {
            return v.clone();
        }
    }
    default
}

/// Load a TOML partial config from a path, returning None if it doesn't exist.
fn load_toml(path: &PathBuf) -> Option<PartialConfig> {
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

/// Load configuration with precedence:
/// CLI flags > env vars > .rune/rune.toml (cwd) > ~/.rune/rune.toml > defaults
pub fn load() -> anyhow::Result<RuneConfig> {
    let cli = <CliArgs as clap::Parser>::parse();

    // Environment variables
    let env_partial = PartialConfig {
        model: env::var("RUNE_MODEL").ok(),
        api_key: env::var("RUNE_API_KEY").ok(),
        skills_dir: env::var("RUNE_SKILLS_DIR").ok(),
        log_level: env::var("RUNE_LOG_LEVEL").ok(),
        max_steps: env::var("RUNE_MAX_STEPS").ok().and_then(|v| v.parse().ok()),
        token_budget: env::var("RUNE_TOKEN_BUDGET").ok().and_then(|v| v.parse().ok()),
        timeout_secs: env::var("RUNE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()),
        base_url: env::var("RUNE_BASE_URL").ok(),
        trace: env::var("RUNE_TRACE").ok().and_then(|v| v.parse().ok()),
        policy: None, // Policy loaded from TOML only (too complex for single env var)
        mcp_servers: None,
    };
    let env_json_output = env::var("RUNE_JSON_OUTPUT").ok().and_then(|v| parse_boolish(&v));
    let env_auto_approve = env::var("RUNE_YES").ok().and_then(|v| parse_boolish(&v));

    // Project-local config: .rune/rune.toml
    let local_cfg = env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));

    // User-level config: ~/.rune/rune.toml
    let user_cfg = env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));

    let lc = local_cfg.as_ref();
    let uc = user_cfg.as_ref();
    let defaults = RuneConfig::default();

    // Merge policy: first non-None wins, otherwise default
    let mut policy = lc.and_then(|c| c.policy.clone())
        .or_else(|| uc.and_then(|c| c.policy.clone()))
        .unwrap_or_default();

    // CLI --policy-mode overrides
    if let Some(ref mode) = cli.policy_mode {
        policy.mode = mode.clone();
    }
    // Env var override for mode
    if let Some(mode) = env::var("RUNE_POLICY_MODE").ok() {
        policy.mode = mode;
    }

    Ok(RuneConfig {
        model: pick(
            &[&cli.model, &env_partial.model, &lc.and_then(|c| c.model.clone()), &uc.and_then(|c| c.model.clone())],
            defaults.model,
        ),
        api_key: cli.api_key
            .or(env_partial.api_key)
            .or(lc.and_then(|c| c.api_key.clone()))
            .or(uc.and_then(|c| c.api_key.clone())),
        skills_dir: pick(
            &[&cli.skills_dir, &env_partial.skills_dir, &lc.and_then(|c| c.skills_dir.clone()), &uc.and_then(|c| c.skills_dir.clone())],
            defaults.skills_dir,
        ),
        log_level: pick(
            &[&cli.log_level, &env_partial.log_level, &lc.and_then(|c| c.log_level.clone()), &uc.and_then(|c| c.log_level.clone())],
            defaults.log_level,
        ),
        max_steps: pick(
            &[&cli.max_steps, &env_partial.max_steps, &lc.and_then(|c| c.max_steps), &uc.and_then(|c| c.max_steps)],
            defaults.max_steps,
        ),
        token_budget: pick(
            &[&cli.token_budget, &env_partial.token_budget, &lc.and_then(|c| c.token_budget), &uc.and_then(|c| c.token_budget)],
            defaults.token_budget,
        ),
        timeout_secs: pick(
            &[&cli.timeout_secs, &env_partial.timeout_secs, &lc.and_then(|c| c.timeout_secs), &uc.and_then(|c| c.timeout_secs)],
            defaults.timeout_secs,
        ),
        base_url: cli.base_url
            .or(env_partial.base_url)
            .or(lc.and_then(|c| c.base_url.clone()))
            .or(uc.and_then(|c| c.base_url.clone())),
        trace: cli.trace
            .or(env_partial.trace)
            .or(lc.and_then(|c| c.trace))
            .or(uc.and_then(|c| c.trace))
            .unwrap_or(defaults.trace),
        json_output: cli.json || env_json_output.unwrap_or(defaults.json_output),
        auto_approve: cli.yes || env_auto_approve.unwrap_or(defaults.auto_approve),
        policy,
        mcp_servers: lc.and_then(|c| c.mcp_servers.clone())
            .or_else(|| uc.and_then(|c| c.mcp_servers.clone()))
            .unwrap_or_default(),
    })
}

/// Persist a new domain to the user's ~/.rune/rune.toml allowed_domains list.
/// Best-effort: if the file can't be read/written, silently skip.
pub fn persist_domain(domain: &str) {
    let config_path = match env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".rune").join("rune.toml"),
        Err(_) => return,
    };
    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Parse and update
    let mut doc: toml::Table = match content.parse() {
        Ok(d) => d,
        Err(_) => return,
    };

    let policy = doc.entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let domains = policy.entry("allowed_domains")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut();
    let domains = match domains {
        Some(d) => d,
        None => return,
    };

    // Don't duplicate
    if domains.iter().any(|v| v.as_str() == Some(domain)) {
        return;
    }

    domains.push(toml::Value::String(domain.to_string()));

    // Write back
    let new_content = doc.to_string();
    let _ = fs::write(&config_path, new_content);
}

/// Persist a new command to the user's ~/.rune/rune.toml allowed_commands list.
/// Best-effort: if the file can't be read/written, silently skip.
pub fn persist_command(command: &str) {
    let config_path = match env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".rune").join("rune.toml"),
        Err(_) => return,
    };
    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut doc: toml::Table = match content.parse() {
        Ok(d) => d,
        Err(_) => return,
    };

    let policy = doc.entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let commands = policy.entry("allowed_commands")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut();
    let commands = match commands {
        Some(c) => c,
        None => return,
    };

    // Don't duplicate
    if commands.iter().any(|v| v.as_str() == Some(command)) {
        return;
    }

    commands.push(toml::Value::String(command.to_string()));

    let new_content = doc.to_string();
    let _ = fs::write(&config_path, new_content);
}
