use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Unified sandbox/security policy.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// Execution mode:
    /// - "confirm": interactive CLI default — prompts user before dangerous tools
    /// - "allowlist": pipe/Concourse default — auto-executes within allowlist, blocks the rest
    /// - "unrestricted": all policy checks skipped (opt-in via --policy-mode or config)
    pub mode: String,
    /// Commands allowed to execute (enforced in "confirm" and "allowlist" modes).
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
            allowed_paths_ro: vec!["/bin".to_string(), "/usr".to_string(), "/lib".to_string()],
            denied_paths: vec!["/root".to_string(), "/etc/shadow".to_string()],
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
    /// Explicit provider selection. Auto-detected from api_key if not set.
    /// Values: "github-copilot", "gemini", "openai", "openrouter", "ollama", "anthropic"
    #[serde(default)]
    pub provider: Option<String>,
    pub skills_dir: String,
    pub log_level: String,
    pub max_steps: Option<u32>,
    pub token_budget: Option<u32>,
    pub timeout_secs: Option<u64>,
    pub base_url: Option<String>,
    pub trace: bool,
    pub json_output: bool,
    pub auto_approve: bool,
    /// Approximate model context window in tokens.
    pub context_window: usize,
    /// Trigger automatic compaction once this fraction of context_window is reached.
    pub compact_threshold: f64,
    /// Keep the last N messages when compacting context.
    pub compact_keep_last: usize,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
    #[serde(default)]
    pub embedding: crate::embedding::EmbeddingConfig,
}

impl Default for RuneConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            api_key: None,
            provider: None,
            skills_dir: "./skills".to_string(),
            log_level: "info".to_string(),
            max_steps: None,
            token_budget: None,
            timeout_secs: None,
            base_url: None,
            trace: false,
            json_output: false,
            auto_approve: false,
            context_window: 128000,
            compact_threshold: 0.85,
            compact_keep_last: 6,
            policy: PolicyConfig::default(),
            mcp_servers: Vec::new(),
            embedding: crate::embedding::EmbeddingConfig::default(),
        }
    }
}

/// Partial config for layered merging.
#[derive(Debug, Deserialize, Default)]
struct PartialConfig {
    model: Option<String>,
    api_key: Option<String>,
    provider: Option<String>,
    skills_dir: Option<String>,
    log_level: Option<String>,
    max_steps: Option<u32>,
    token_budget: Option<u32>,
    timeout_secs: Option<u64>,
    base_url: Option<String>,
    trace: Option<bool>,
    context_window: Option<usize>,
    compact_threshold: Option<f64>,
    compact_keep_last: Option<usize>,
    policy: Option<PolicyConfig>,
    mcp_servers: Option<Vec<crate::mcp::McpServerConfig>>,
    embedding: Option<crate::embedding::EmbeddingConfig>,
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

/// Like pick but returns Option<T> — None if no source provides a value.
fn pick_option<T: Clone>(sources: &[&Option<T>]) -> Option<T> {
    for src in sources {
        if let Some(v) = src {
            return Some(v.clone());
        }
    }
    None
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
        token_budget: env::var("RUNE_TOKEN_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok()),
        timeout_secs: env::var("RUNE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok()),
        base_url: env::var("RUNE_BASE_URL").ok(),
        provider: env::var("RUNE_PROVIDER").ok(),
        trace: env::var("RUNE_TRACE").ok().and_then(|v| v.parse().ok()),
        context_window: env::var("RUNE_CONTEXT_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok()),
        compact_threshold: env::var("RUNE_COMPACT_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok()),
        compact_keep_last: env::var("RUNE_COMPACT_KEEP_LAST")
            .ok()
            .and_then(|v| v.parse().ok()),
        policy: None, // Policy loaded from TOML only (too complex for single env var)
        mcp_servers: None,
        embedding: None,
    };
    let env_json_output = env::var("RUNE_JSON_OUTPUT")
        .ok()
        .and_then(|v| parse_boolish(&v));
    let env_auto_approve = env::var("RUNE_YES").ok().and_then(|v| parse_boolish(&v));

    // Project-local config: rune.toml, then .rune/rune.toml
    let cwd_cfg = env::current_dir()
        .ok()
        .map(|cwd| cwd.join("rune.toml"))
        .and_then(|p| load_toml(&p));
    let local_cfg = env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));

    // User-level config: ~/.rune/rune.toml
    let user_cfg = env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));

    let cwdc = cwd_cfg.as_ref();
    let lc = local_cfg.as_ref();
    let uc = user_cfg.as_ref();
    let defaults = RuneConfig::default();

    // Merge policy: first non-None wins, otherwise default
    let mut policy = lc
        .and_then(|c| c.policy.clone())
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
            &[
                &cli.model,
                &env_partial.model,
                &lc.and_then(|c| c.model.clone()),
                &uc.and_then(|c| c.model.clone()),
            ],
            defaults.model,
        ),
        api_key: cli
            .api_key
            .or(env_partial.api_key)
            .or(lc.and_then(|c| c.api_key.clone()))
            .or(uc.and_then(|c| c.api_key.clone())),
        provider: pick_option(&[
            &env_partial.provider,
            &cwdc.and_then(|c| c.provider.clone()),
            &lc.and_then(|c| c.provider.clone()),
            &uc.and_then(|c| c.provider.clone()),
        ]),
        skills_dir: pick(
            &[
                &cli.skills_dir,
                &env_partial.skills_dir,
                &lc.and_then(|c| c.skills_dir.clone()),
                &uc.and_then(|c| c.skills_dir.clone()),
            ],
            defaults.skills_dir,
        ),
        log_level: pick(
            &[
                &cli.log_level,
                &env_partial.log_level,
                &lc.and_then(|c| c.log_level.clone()),
                &uc.and_then(|c| c.log_level.clone()),
            ],
            defaults.log_level,
        ),
        max_steps: pick_option(&[
            &cli.max_steps,
            &env_partial.max_steps,
            &cwdc.and_then(|c| c.max_steps),
            &lc.and_then(|c| c.max_steps),
            &uc.and_then(|c| c.max_steps),
        ]),
        token_budget: pick_option(&[
            &cli.token_budget,
            &env_partial.token_budget,
            &cwdc.and_then(|c| c.token_budget),
            &lc.and_then(|c| c.token_budget),
            &uc.and_then(|c| c.token_budget),
        ]),
        timeout_secs: pick_option(&[
            &cli.timeout_secs,
            &env_partial.timeout_secs,
            &cwdc.and_then(|c| c.timeout_secs),
            &lc.and_then(|c| c.timeout_secs),
            &uc.and_then(|c| c.timeout_secs),
        ]),
        base_url: cli
            .base_url
            .or(env_partial.base_url)
            .or(lc.and_then(|c| c.base_url.clone()))
            .or(uc.and_then(|c| c.base_url.clone())),
        trace: cli
            .trace
            .or(env_partial.trace)
            .or(cwdc.and_then(|c| c.trace))
            .or(lc.and_then(|c| c.trace))
            .or(uc.and_then(|c| c.trace))
            .unwrap_or(defaults.trace),
        json_output: cli.json || env_json_output.unwrap_or(defaults.json_output),
        auto_approve: cli.yes || env_auto_approve.unwrap_or(defaults.auto_approve),
        context_window: env_partial
            .context_window
            .or(cwdc.and_then(|c| c.context_window))
            .or(lc.and_then(|c| c.context_window))
            .or(uc.and_then(|c| c.context_window))
            .unwrap_or(defaults.context_window),
        compact_threshold: env_partial
            .compact_threshold
            .or(cwdc.and_then(|c| c.compact_threshold))
            .or(lc.and_then(|c| c.compact_threshold))
            .or(uc.and_then(|c| c.compact_threshold))
            .unwrap_or(defaults.compact_threshold),
        compact_keep_last: env_partial
            .compact_keep_last
            .or(cwdc.and_then(|c| c.compact_keep_last))
            .or(lc.and_then(|c| c.compact_keep_last))
            .or(uc.and_then(|c| c.compact_keep_last))
            .unwrap_or(defaults.compact_keep_last),
        policy,
        mcp_servers: cwdc
            .and_then(|c| c.mcp_servers.clone())
            .or_else(|| lc.and_then(|c| c.mcp_servers.clone()))
            .or_else(|| uc.and_then(|c| c.mcp_servers.clone()))
            .unwrap_or_default(),
        embedding: cwdc
            .and_then(|c| c.embedding.clone())
            .or_else(|| lc.and_then(|c| c.embedding.clone()))
            .or_else(|| uc.and_then(|c| c.embedding.clone()))
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

    let policy = doc
        .entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let domains = policy
        .entry("allowed_domains")
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

    let policy = doc
        .entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let commands = policy
        .entry("allowed_commands")
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

/// Persist a path to allowed_paths_ro in ~/.rune/rune.toml.
pub fn persist_path_ro(path: &str) {
    persist_policy_array("allowed_paths_ro", path);
}

/// Persist a path to allowed_paths_rw in ~/.rune/rune.toml.
pub fn persist_path_rw(path: &str) {
    persist_policy_array("allowed_paths_rw", path);
}

/// Generic helper to persist a value into a policy array field.
fn persist_policy_array(field: &str, value: &str) {
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

    let policy = doc
        .entry("policy")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut();
    let policy = match policy {
        Some(p) => p,
        None => return,
    };

    let arr = policy
        .entry(field)
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut();
    let arr = match arr {
        Some(a) => a,
        None => return,
    };

    if arr.iter().any(|v| v.as_str() == Some(value)) {
        return;
    }

    arr.push(toml::Value::String(value.to_string()));

    let new_content = doc.to_string();
    let _ = fs::write(&config_path, new_content);
}

// Unit tests for config module: pick, pick_option, parse_boolish, defaults
#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn test_pick_returns_first_some() {
        let a = Some("first".to_string());
        let b = Some("second".to_string());
        let c: Option<String> = None;
        assert_eq!(pick(&[&a, &b], "default".to_string()), "first");
        assert_eq!(pick(&[&c, &b], "default".to_string()), "second");
    }

    #[test]
    fn test_pick_returns_default_when_all_none() {
        let a: Option<String> = None;
        let b: Option<String> = None;
        assert_eq!(pick(&[&a, &b], "default".to_string()), "default");
    }

    #[test]
    fn test_pick_option_returns_first_some() {
        let a: Option<u32> = None;
        let b = Some(42u32);
        let c = Some(99u32);
        assert_eq!(pick_option(&[&a, &b, &c]), Some(42));
    }

    #[test]
    fn test_pick_option_returns_none_when_all_none() {
        let a: Option<u32> = None;
        let b: Option<u32> = None;
        assert_eq!(pick_option(&[&a, &b]), None);
    }

    #[test]
    fn test_parse_boolish_true_variants() {
        assert_eq!(parse_boolish("1"), Some(true));
        assert_eq!(parse_boolish("true"), Some(true));
        assert_eq!(parse_boolish("TRUE"), Some(true));
        assert_eq!(parse_boolish("yes"), Some(true));
        assert_eq!(parse_boolish("Yes"), Some(true));
        assert_eq!(parse_boolish("y"), Some(true));
        assert_eq!(parse_boolish("on"), Some(true));
        assert_eq!(parse_boolish("ON"), Some(true));
    }

    #[test]
    fn test_parse_boolish_false_variants() {
        assert_eq!(parse_boolish("0"), Some(false));
        assert_eq!(parse_boolish("false"), Some(false));
        assert_eq!(parse_boolish("FALSE"), Some(false));
        assert_eq!(parse_boolish("no"), Some(false));
        assert_eq!(parse_boolish("n"), Some(false));
        assert_eq!(parse_boolish("off"), Some(false));
    }

    #[test]
    fn test_parse_boolish_invalid() {
        assert_eq!(parse_boolish("maybe"), None);
        assert_eq!(parse_boolish(""), None);
        assert_eq!(parse_boolish("2"), None);
        assert_eq!(parse_boolish("yep"), None);
    }

    #[test]
    fn test_parse_boolish_with_whitespace() {
        assert_eq!(parse_boolish("  true  "), Some(true));
        assert_eq!(parse_boolish(" false "), Some(false));
    }

    #[test]
    fn test_policy_config_default() {
        let p = PolicyConfig::default();
        assert_eq!(p.mode, "confirm");
        assert!(p.allowed_commands.is_empty());
        assert!(p.allowed_domains.is_empty());
        assert!(p.denied_syscalls.contains(&"ptrace".to_string()));
        assert!(p.denied_syscalls.contains(&"mount".to_string()));
        assert!(p.denied_syscalls.contains(&"bpf".to_string()));
        assert!(p.allowed_paths_rw.contains(&"/tmp".to_string()));
        assert!(p.allowed_paths_ro.contains(&"/bin".to_string()));
        assert!(p.denied_paths.contains(&"/root".to_string()));
        assert_eq!(p.max_memory_mb, 512);
        assert_eq!(p.max_pids, 64);
    }

    #[test]
    fn test_rune_config_default() {
        let c = RuneConfig::default();
        assert_eq!(c.model, "gpt-4");
        assert!(c.api_key.is_none());
        assert_eq!(c.skills_dir, "./skills");
        assert_eq!(c.log_level, "info");
        assert!(c.max_steps.is_none());
        assert!(c.token_budget.is_none());
        assert!(c.timeout_secs.is_none());
        assert!(c.base_url.is_none());
        assert!(!c.trace);
        assert!(!c.json_output);
        assert!(!c.auto_approve);
    }

    #[test]
    fn test_load_toml_nonexistent_path() {
        let path = PathBuf::from("/nonexistent/rune.toml");
        assert!(load_toml(&path).is_none());
    }

    #[test]
    fn test_load_toml_valid() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4o"
log_level = "debug"
max_steps = 50
token_budget = 8000
"#,
        )
        .unwrap();

        let partial = load_toml(&path).expect("should parse");
        assert_eq!(partial.model.as_deref(), Some("gpt-4o"));
        assert_eq!(partial.log_level.as_deref(), Some("debug"));
        assert_eq!(partial.max_steps, Some(50));
        assert_eq!(partial.token_budget, Some(8000));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_toml_with_policy() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-pol-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4"

[policy]
mode = "allowlist"
allowed_commands = ["ls", "cat", "grep"]
allowed_domains = ["github.com", "*.openai.com"]
allowed_paths_rw = ["/workspace"]
allowed_paths_ro = ["/usr", "/bin"]
denied_paths = ["/etc/shadow"]
"#,
        )
        .unwrap();

        let partial = load_toml(&path).expect("should parse");
        let policy = partial.policy.unwrap();
        assert_eq!(policy.mode, "allowlist");
        assert_eq!(policy.allowed_commands, vec!["ls", "cat", "grep"]);
        assert_eq!(policy.allowed_domains, vec!["github.com", "*.openai.com"]);
        assert_eq!(policy.allowed_paths_rw, vec!["/workspace"]);
        assert_eq!(policy.denied_paths, vec!["/etc/shadow"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_policy_mode_is_confirm() {
        let policy = PolicyConfig::default();
        assert_eq!(
            policy.mode, "confirm",
            "default policy should be confirm for interactive"
        );
    }

    #[test]
    fn test_load_toml_unrestricted_mode() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-unr-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4"

[policy]
mode = "unrestricted"
"#,
        )
        .unwrap();

        let partial = load_toml(&path).expect("should parse");
        let policy = partial.policy.unwrap();
        assert_eq!(policy.mode, "unrestricted");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_toml_invalid_content() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-bad-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(&path, "this is not valid toml {{{{").unwrap();

        assert!(load_toml(&path).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_domain_creates_entry() {
        let dir = std::env::temp_dir().join(format!("rune-persist-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-4"

[policy]
mode = "confirm"
allowed_domains = ["existing.com"]
"#,
        )
        .unwrap();

        // Set HOME to our test dir so persist_domain finds the right file
        let old_home = env::var("HOME").ok();
        env::set_var("HOME", &dir);

        persist_domain("new-domain.com");

        // Restore HOME
        if let Some(h) = old_home {
            env::set_var("HOME", h);
        }

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("new-domain.com"));
        assert!(content.contains("existing.com"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_domain_no_duplicate() {
        let dir = std::env::temp_dir().join(format!("rune-persist-dup-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        fs::write(
            &config_path,
            r#"
[policy]
allowed_domains = ["github.com"]
"#,
        )
        .unwrap();

        let old_home = env::var("HOME").ok();
        env::set_var("HOME", &dir);

        persist_domain("github.com"); // already exists

        if let Some(h) = old_home {
            env::set_var("HOME", h);
        }

        let content = fs::read_to_string(&config_path).unwrap();
        // Should only appear once
        assert_eq!(content.matches("github.com").count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_command_creates_entry() {
        let dir = std::env::temp_dir().join(format!("rune-persist-cmd-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        fs::write(
            &config_path,
            r#"
[policy]
mode = "confirm"
"#,
        )
        .unwrap();

        let old_home = env::var("HOME").ok();
        env::set_var("HOME", &dir);

        persist_command("cargo");

        if let Some(h) = old_home {
            env::set_var("HOME", h);
        }

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("cargo"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_path_ro() {
        let dir = std::env::temp_dir().join(format!("rune-persist-path-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        fs::write(
            &config_path,
            r#"
[policy]
mode = "confirm"
"#,
        )
        .unwrap();

        let old_home = env::var("HOME").ok();
        env::set_var("HOME", &dir);

        persist_path_ro("/home/user/project");

        if let Some(h) = old_home {
            env::set_var("HOME", h);
        }

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("/home/user/project"));
        assert!(content.contains("allowed_paths_ro"));

        let _ = fs::remove_dir_all(&dir);
    }
}
