use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Expand a leading `~` or `~/` in a path string to the value of `$HOME`.
/// Returns the original string unchanged if `HOME` is not set or the path
/// does not start with `~`.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            format!("{}/{}", home, rest)
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    }
}

/// Apply tilde expansion to every element of a `Vec<String>`.
fn expand_tilde_vec(v: &mut Vec<String>) {
    for item in v.iter_mut() {
        *item = expand_tilde(item);
    }
}

/// Unified sandbox/security policy.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// Execution mode:
    /// - "confirm": interactive CLI default — prompts user before dangerous tools
    /// - "allowlist": pipe/Concourse CI default — auto-executes within allowlist, blocks the rest
    /// - "unrestricted": all policy checks skipped (opt-in via --policy-mode or config)
    #[serde(default = "default_policy_mode")]
    pub mode: String,
    /// Commands allowed to execute (enforced in "confirm" and "allowlist" modes).
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    /// Network domains allowed (empty = block all).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Dangerous syscalls to ALLOW through seccomp (empty = block all dangerous syscalls).
    /// Dangerous syscalls: ptrace, mount, kexec_load, bpf, setns, unshare
    #[serde(default)]
    pub allowed_syscalls: Vec<String>,
    /// Paths with read-write access.
    #[serde(default)]
    pub allowed_paths_rw: Vec<String>,
    /// Paths with read-only access.
    #[serde(default)]
    pub allowed_paths_ro: Vec<String>,
    /// Individual files with read-only access (absolute paths).
    #[serde(default)]
    pub allowed_files_ro: Vec<String>,
    /// Individual files with read-write access (absolute paths).
    #[serde(default)]
    pub allowed_files_rw: Vec<String>,
    /// Paths explicitly denied.
    #[serde(default)]
    pub denied_paths: Vec<String>,
    /// Memory limit in MB (0 = no limit).
    #[serde(default)]
    pub max_memory_mb: u64,
    /// Max child processes (0 = no limit).
    #[serde(default)]
    pub max_pids: u32,
    /// Tmpfs size limit in MB for sandbox /tmp (default 100, 0 = use host /tmp).
    #[serde(default = "default_max_tmp_mb")]
    pub max_tmp_mb: u64,
}

fn default_policy_mode() -> String {
    "confirm".to_string()
}

fn default_max_tmp_mb() -> u64 {
    100
}

/// Configuration for `rune serve` mode.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotesConfig {
    /// Port to listen on (default: 9527).
    pub port: Option<u16>,
    /// Bind address (default: 127.0.0.1).
    pub bind: Option<String>,
    /// User token required from clients. None = no user access possible.
    pub user_token: Option<String>,
    /// Admin token: clients with this token get admin role (can approve tool requests).
    pub admin_token: Option<String>,
    /// Guest token: read-only access. Cannot chat, create, edit, or delete anything.
    pub guest_token: Option<String>,
    /// Model to use for notes mode. If not set or empty, defaults to auto-detecting the first OpenRouter model.
    pub model: Option<String>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: "confirm".to_string(),
            allowed_commands: Vec::new(),
            allowed_domains: Vec::new(),
            allowed_syscalls: Vec::new(), // empty = block all dangerous syscalls
            allowed_paths_rw: vec!["/tmp".to_string()],
            allowed_paths_ro: vec!["/bin".to_string(), "/usr".to_string(), "/lib".to_string()],
            allowed_files_ro: Vec::new(),
            allowed_files_rw: Vec::new(),
            denied_paths: vec!["/root".to_string(), "/etc/shadow".to_string()],
            max_memory_mb: 512,
            max_pids: 64,
            max_tmp_mb: 100,
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
    /// Trace output directory. None = disabled, Some(path) = enabled.
    pub trace: Option<String>,
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
    /// Thinking/reasoning level: "none", "low", "medium", "high". None = provider default.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Custom system prompt. When set, replaces the default hardcoded prompt.
    /// AGENTS.md is still appended if present.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Skills to preload at startup (comma-separated names from --skills flag).
    /// When set, only these skills are available; semantic/@ discovery is skipped.
    #[serde(skip)]
    pub preload_skills: Vec<String>,
    /// Notes mode configuration ([notes] section in rune.toml).
    #[serde(default)]
    pub notes: NotesConfig,
    /// CLI positional prompt (for one-shot mode). Not from config file.
    #[serde(skip)]
    pub cli_prompt: Option<String>,
}

impl Default for RuneConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            api_key: None,
            provider: None,
            skills_dir: "./skills".to_string(),
            log_level: "error".to_string(),
            max_steps: Some(50),
            // No hard token-budget cap by default; context_window + compaction drive
            // memory management instead. IMPORTANT: this means unbounded LLM costs are
            // possible for long-running sessions with slowly-growing context (e.g. many
            // short messages that never trigger compaction). Operators who want a safety
            // ceiling must set `token_budget` explicitly in rune.toml, e.g.:
            //   token_budget = 262144   # 256 K tokens ≈ original default
            token_budget: None,
            timeout_secs: Some(30),
            base_url: None,
            trace: None,
            json_output: false,
            auto_approve: false,
            context_window: 128000,
            compact_threshold: 0.85,
            compact_keep_last: 6,
            policy: PolicyConfig::default(),
            mcp_servers: Vec::new(),
            embedding: crate::embedding::EmbeddingConfig::default(),
            thinking: None,
            system_prompt: None,
            preload_skills: Vec::new(),
            notes: NotesConfig::default(),
            cli_prompt: None,
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
    trace: Option<String>,
    context_window: Option<usize>,
    compact_threshold: Option<f64>,
    compact_keep_last: Option<usize>,
    policy: Option<PolicyConfig>,
    mcp_servers: Option<Vec<crate::mcp::McpServerConfig>>,
    embedding: Option<crate::embedding::EmbeddingConfig>,
    system_prompt: Option<String>,
    thinking: Option<String>,
    notes: Option<NotesConfig>,
}

/// CLI argument overrides.
#[derive(Debug, clap::Parser)]
#[command(
    name = "rune",
    version,
    about = "ᚱ Rune — High-performance zero-trust AI Agent",
    long_about = "ᚱ Rune — High-performance zero-trust AI Agent\n\
\n\
Single binary, dual mode: interactive CLI assistant and Concourse CI\n\
resource type. Every tool execution is sandboxed through 5 kernel-level\n\
isolation layers (cgroups, seccomp, landlock, net-guard, namespace).\n\
\n\
SUBCOMMANDS:\n\
  rune init              Interactive setup wizard\n\
\n\
EXAMPLES:\n\
  rune                   Start interactive CLI (streaming, confirm mode)\n\
  rune init              Run first-time setup wizard\n\
  rune --provider gemini Start with Google Gemini\n\
  rune --model gpt-4o    Override model\n\
  rune --yes             Auto-approve tool execution\n\
  rune --json            Machine-readable JSON output\n\
  echo \"...\" | rune     Pipe mode (one-shot, non-interactive)\n\
\n\
CONFIG PRECEDENCE:\n\
  --config file > CLI flags > env vars (RUNE_*) > ./rune.toml > .rune/rune.toml > ~/.rune/rune.toml > defaults\n\
\n\
TOOLS (built-in, all sandboxed):\n\
  read_file, write_file, list_dir, execute_cmd, fetch_url, inspect_process\n\
\n\
SANDBOX LAYERS:\n\
  1. cgroups v2 (memory + process limits)\n\
  2. net-guard (seccomp user notification — per-domain network filter)\n\
  3. seccomp BPF (syscall filter)\n\
  4. landlock (filesystem restriction)\n\
  5. DNS allowlist (wildcard domain support)"
)]
struct CliArgs {
    /// Path to rune.toml config file [highest priority, hard-fails if missing or invalid]
    #[arg(
        long,
        short = 'c',
        env = "RUNE_CONFIG",
        value_name = "path/rune.toml",
        help_heading = "Configuration"
    )]
    config: Option<String>,

    /// LLM provider [github-copilot, gemini, openai, openrouter, ollama, anthropic]
    #[arg(long, env = "RUNE_PROVIDER", help_heading = "Provider")]
    provider: Option<String>,

    /// Model name [e.g. gpt-4o, gemini-2.0-flash, claude-3.5-sonnet]
    #[arg(long, env = "RUNE_MODEL", help_heading = "Provider")]
    model: Option<String>,

    /// API key for the LLM provider
    #[arg(long, env = "RUNE_API_KEY", help_heading = "Provider")]
    api_key: Option<String>,

    /// Provider base URL (auto-detected for Copilot/Gemini)
    #[arg(long, env = "RUNE_BASE_URL", help_heading = "Provider")]
    base_url: Option<String>,

    /// Disable all security policy checks (sandbox, allowlists, confirm prompts)
    #[arg(long, help_heading = "Security")]
    unrestricted: bool,

    /// Auto-approve dangerous tool calls (does NOT bypass policy allowlist)
    #[arg(long, short = 'y', action = clap::ArgAction::SetTrue, help_heading = "Security")]
    yes: bool,

    /// Maximum agent loop iterations [default: 50, 0 = unlimited]
    #[arg(long, env = "RUNE_MAX_STEPS", help_heading = "Limits")]
    max_steps: Option<u32>,

    /// Maximum tokens per run [default: 256k, 0 = unlimited]
    #[arg(long, env = "RUNE_TOKEN_BUDGET", help_heading = "Limits")]
    token_budget: Option<u32>,

    /// Command timeout in seconds [default: 30, 0 = unlimited]
    #[arg(long, env = "RUNE_TIMEOUT_SECS", help_heading = "Limits")]
    timeout_secs: Option<u64>,

    /// Output in JSON format (machine-readable, for scripting)
    #[arg(long, action = clap::ArgAction::SetTrue, help_heading = "Output")]
    json: bool,

    /// Enable trace recording to specified directory [empty = disabled]
    #[arg(long, env = "RUNE_TRACE", help_heading = "Output")]
    trace: Option<String>,

    /// Log level [trace, debug, info, warn, error]
    #[arg(long, env = "RUNE_LOG_LEVEL", help_heading = "Output")]
    log_level: Option<String>,

    /// Thinking/reasoning effort level [off|low|medium|high|xhigh]
    #[arg(long, env = "RUNE_THINKING", help_heading = "Advanced")]
    thinking: Option<String>,

    /// Directory containing skill definitions
    #[arg(long, env = "RUNE_SKILLS_DIR", help_heading = "Advanced")]
    skills_dir: Option<String>,

    /// Preload specific skills by name (comma-separated). Only these skills
    /// will be injected; @ref and semantic search are disabled.
    /// Example: --skills jira,launchpad
    #[arg(
        long,
        env = "RUNE_SKILLS",
        help_heading = "Advanced",
        value_delimiter = ','
    )]
    skills: Vec<String>,

    /// Custom system prompt (replaces default, AGENTS.md still appended)
    #[arg(long, env = "RUNE_SYSTEM_PROMPT", help_heading = "Advanced")]
    system_prompt: Option<String>,

    /// Prompt to send (one-shot mode). Alternative to piping stdin.
    #[arg(trailing_var_arg = true, help_heading = "Input")]
    prompt: Vec<String>,
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
        thinking: env::var("RUNE_THINKING").ok(),
        system_prompt: env::var("RUNE_SYSTEM_PROMPT").ok(),
        notes: None,
    };
    let env_json_output = env::var("RUNE_JSON_OUTPUT")
        .ok()
        .and_then(|v| parse_boolish(&v));
    let env_auto_approve = env::var("RUNE_YES").ok().and_then(|v| parse_boolish(&v));

    // Explicit config file (--config / -c / RUNE_CONFIG)
    // Highest priority: hard-fails if the file is missing or has parse errors.
    // When specified, skip the default search chain entirely.
    let explicit_cfg: Option<PartialConfig> = cli.config.as_ref().map(|p| {
        let path = PathBuf::from(p);
        if !path.exists() {
            eprintln!("error: config file not found: {}", path.display());
            std::process::exit(1);
        }
        let content = fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("error: cannot read config file {}: {}", path.display(), e);
            std::process::exit(1);
        });
        toml::from_str::<PartialConfig>(&content).unwrap_or_else(|e| {
            eprintln!("error: invalid config file {}: {}", path.display(), e);
            std::process::exit(1);
        })
    });

    // Project-local config: rune.toml, then .rune/rune.toml (skipped when --config is set)
    let cwd_cfg = if cli.config.is_some() {
        None
    } else {
        env::current_dir()
            .ok()
            .map(|cwd| cwd.join("rune.toml"))
            .and_then(|p| load_toml(&p))
    };
    let local_cfg = if cli.config.is_some() {
        None
    } else {
        env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".rune").join("rune.toml"))
            .and_then(|p| load_toml(&p))
    };

    // User-level config: ~/.rune/rune.toml (skipped when --config is set)
    let user_cfg = if cli.config.is_some() {
        None
    } else {
        env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".rune").join("rune.toml"))
            .and_then(|p| load_toml(&p))
    };

    let ec = explicit_cfg.as_ref();
    let cwdc = cwd_cfg.as_ref();
    let lc = local_cfg.as_ref();
    let uc = user_cfg.as_ref();
    let defaults = RuneConfig::default();

    // Merge policy: first non-None wins, otherwise default
    let mut policy = ec
        .and_then(|c| c.policy.clone())
        .or_else(|| cwdc.and_then(|c| c.policy.clone()))
        .or_else(|| lc.and_then(|c| c.policy.clone()))
        .or_else(|| uc.and_then(|c| c.policy.clone()))
        .unwrap_or_default();

    // CLI --unrestricted flag overrides policy mode
    if cli.unrestricted {
        policy.mode = "unrestricted".to_string();
    }
    // Env var override for mode (legacy support)
    if let Some(mode) = env::var("RUNE_POLICY_MODE").ok() {
        policy.mode = mode;
    }

    let mut cfg = RuneConfig {
        model: pick(
            &[
                &ec.and_then(|c| c.model.clone()),
                &cli.model,
                &env_partial.model,
                &cwdc.and_then(|c| c.model.clone()),
                &lc.and_then(|c| c.model.clone()),
                &uc.and_then(|c| c.model.clone()),
            ],
            defaults.model,
        ),
        api_key: ec
            .and_then(|c| c.api_key.clone())
            .or(cli.api_key)
            .or(env_partial.api_key)
            .or(cwdc.and_then(|c| c.api_key.clone()))
            .or(lc.and_then(|c| c.api_key.clone()))
            .or(uc.and_then(|c| c.api_key.clone())),
        provider: pick_option(&[
            &ec.and_then(|c| c.provider.clone()),
            &cli.provider,
            &env_partial.provider,
            &cwdc.and_then(|c| c.provider.clone()),
            &lc.and_then(|c| c.provider.clone()),
            &uc.and_then(|c| c.provider.clone()),
        ]),
        skills_dir: pick(
            &[
                &ec.and_then(|c| c.skills_dir.clone()),
                &cli.skills_dir,
                &env_partial.skills_dir,
                &cwdc.and_then(|c| c.skills_dir.clone()),
                &lc.and_then(|c| c.skills_dir.clone()),
                &uc.and_then(|c| c.skills_dir.clone()),
            ],
            defaults.skills_dir,
        ),
        log_level: pick(
            &[
                &ec.and_then(|c| c.log_level.clone()),
                &cli.log_level,
                &env_partial.log_level,
                &cwdc.and_then(|c| c.log_level.clone()),
                &lc.and_then(|c| c.log_level.clone()),
                &uc.and_then(|c| c.log_level.clone()),
            ],
            defaults.log_level,
        ),
        max_steps: pick_option(&[
            &ec.and_then(|c| c.max_steps),
            &cli.max_steps,
            &env_partial.max_steps,
            &cwdc.and_then(|c| c.max_steps),
            &lc.and_then(|c| c.max_steps),
            &uc.and_then(|c| c.max_steps),
        ]),
        token_budget: pick_option(&[
            &ec.and_then(|c| c.token_budget),
            &cli.token_budget,
            &env_partial.token_budget,
            &cwdc.and_then(|c| c.token_budget),
            &lc.and_then(|c| c.token_budget),
            &uc.and_then(|c| c.token_budget),
        ]),
        timeout_secs: pick_option(&[
            &ec.and_then(|c| c.timeout_secs),
            &cli.timeout_secs,
            &env_partial.timeout_secs,
            &cwdc.and_then(|c| c.timeout_secs),
            &lc.and_then(|c| c.timeout_secs),
            &uc.and_then(|c| c.timeout_secs),
        ]),
        base_url: ec
            .and_then(|c| c.base_url.clone())
            .or(cli.base_url)
            .or(env_partial.base_url)
            .or(cwdc.and_then(|c| c.base_url.clone()))
            .or(lc.and_then(|c| c.base_url.clone()))
            .or(uc.and_then(|c| c.base_url.clone())),
        trace: ec
            .and_then(|c| c.trace.clone())
            .or(cli.trace)
            .or(env_partial.trace)
            .or(cwdc.and_then(|c| c.trace.clone()))
            .or(lc.and_then(|c| c.trace.clone()))
            .or(uc.and_then(|c| c.trace.clone()))
            .or(defaults.trace),
        json_output: cli.json || env_json_output.unwrap_or(defaults.json_output),
        auto_approve: cli.yes || env_auto_approve.unwrap_or(defaults.auto_approve),
        context_window: ec
            .and_then(|c| c.context_window)
            .or(env_partial.context_window)
            .or(cwdc.and_then(|c| c.context_window))
            .or(lc.and_then(|c| c.context_window))
            .or(uc.and_then(|c| c.context_window))
            .unwrap_or(defaults.context_window),
        compact_threshold: ec
            .and_then(|c| c.compact_threshold)
            .or(env_partial.compact_threshold)
            .or(cwdc.and_then(|c| c.compact_threshold))
            .or(lc.and_then(|c| c.compact_threshold))
            .or(uc.and_then(|c| c.compact_threshold))
            .unwrap_or(defaults.compact_threshold),
        compact_keep_last: ec
            .and_then(|c| c.compact_keep_last)
            .or(env_partial.compact_keep_last)
            .or(cwdc.and_then(|c| c.compact_keep_last))
            .or(lc.and_then(|c| c.compact_keep_last))
            .or(uc.and_then(|c| c.compact_keep_last))
            .unwrap_or(defaults.compact_keep_last),
        policy,
        mcp_servers: ec
            .and_then(|c| c.mcp_servers.clone())
            .or_else(|| cwdc.and_then(|c| c.mcp_servers.clone()))
            .or_else(|| lc.and_then(|c| c.mcp_servers.clone()))
            .or_else(|| uc.and_then(|c| c.mcp_servers.clone()))
            .unwrap_or_default(),
        embedding: ec
            .and_then(|c| c.embedding.clone())
            .or_else(|| cwdc.and_then(|c| c.embedding.clone()))
            .or_else(|| lc.and_then(|c| c.embedding.clone()))
            .or_else(|| uc.and_then(|c| c.embedding.clone()))
            .unwrap_or_default(),
        thinking: pick_option(&[
            &ec.and_then(|c| c.thinking.clone()),
            &cli.thinking,
            &env_partial.thinking,
            &cwdc.and_then(|c| c.thinking.clone()),
            &lc.and_then(|c| c.thinking.clone()),
            &uc.and_then(|c| c.thinking.clone()),
        ]),
        system_prompt: pick_option(&[
            &ec.and_then(|c| c.system_prompt.clone()),
            &cli.system_prompt,
            &env_partial.system_prompt,
            &cwdc.and_then(|c| c.system_prompt.clone()),
            &lc.and_then(|c| c.system_prompt.clone()),
            &uc.and_then(|c| c.system_prompt.clone()),
        ]),
        preload_skills: cli
            .skills
            .iter()
            .flat_map(|s| {
                s.split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
            .collect(),
        notes: ec
            .and_then(|c| c.notes.clone())
            .or_else(|| cwdc.and_then(|c| c.notes.clone()))
            .or_else(|| lc.and_then(|c| c.notes.clone()))
            .or_else(|| uc.and_then(|c| c.notes.clone()))
            .unwrap_or_default(),
        cli_prompt: if cli.prompt.is_empty() {
            None
        } else {
            Some(cli.prompt.join(" "))
        },
    };

    // Post-processing: expand ~ in all path-like config fields
    cfg.skills_dir = expand_tilde(&cfg.skills_dir);
    if let Some(ref mut t) = cfg.trace {
        *t = expand_tilde(t);
    }
    expand_tilde_vec(&mut cfg.policy.allowed_paths_rw);
    expand_tilde_vec(&mut cfg.policy.allowed_paths_ro);
    expand_tilde_vec(&mut cfg.policy.allowed_files_ro);
    expand_tilde_vec(&mut cfg.policy.allowed_files_rw);
    expand_tilde_vec(&mut cfg.policy.denied_paths);

    Ok(cfg)
}

/// Persist a new domain to the user's ~/.rune/rune.toml allowed_domains list.
/// Best-effort: if the file can't be read/written, silently skip.
/// Load configuration without clap CLI arg parsing.
/// Used by `rune serve` to avoid clap choking on unknown subcommands.
/// Reads: env vars > ./rune.toml > .rune/rune.toml > ~/.rune/rune.toml > defaults
pub fn load_without_clap() -> anyhow::Result<RuneConfig> {
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
        policy: None,
        mcp_servers: None,
        embedding: None,
        thinking: env::var("RUNE_THINKING").ok(),
        system_prompt: env::var("RUNE_SYSTEM_PROMPT").ok(),
        notes: None,
    };

    // Load TOML files
    let cwd_cfg = env::current_dir()
        .ok()
        .map(|cwd| cwd.join("rune.toml"))
        .and_then(|p| load_toml(&p));
    let local_cfg = env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));
    let user_cfg = env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".rune").join("rune.toml"))
        .and_then(|p| load_toml(&p));

    let cwdc = cwd_cfg.as_ref();
    let lc = local_cfg.as_ref();
    let uc = user_cfg.as_ref();
    let defaults = RuneConfig::default();

    let policy = cwdc
        .and_then(|c| c.policy.clone())
        .or_else(|| lc.and_then(|c| c.policy.clone()))
        .or_else(|| uc.and_then(|c| c.policy.clone()))
        .unwrap_or_default();

    let cfg = RuneConfig {
        model: pick(
            &[
                &env_partial.model,
                &cwdc.and_then(|c| c.model.clone()),
                &lc.and_then(|c| c.model.clone()),
                &uc.and_then(|c| c.model.clone()),
            ],
            defaults.model,
        ),
        api_key: env_partial
            .api_key
            .or_else(|| cwdc.and_then(|c| c.api_key.clone()))
            .or_else(|| lc.and_then(|c| c.api_key.clone()))
            .or_else(|| uc.and_then(|c| c.api_key.clone())),
        provider: env_partial
            .provider
            .or_else(|| cwdc.and_then(|c| c.provider.clone()))
            .or_else(|| lc.and_then(|c| c.provider.clone()))
            .or_else(|| uc.and_then(|c| c.provider.clone())),
        skills_dir: pick(
            &[
                &env_partial.skills_dir,
                &cwdc.and_then(|c| c.skills_dir.clone()),
                &lc.and_then(|c| c.skills_dir.clone()),
                &uc.and_then(|c| c.skills_dir.clone()),
            ],
            defaults.skills_dir,
        ),
        log_level: pick(
            &[
                &env_partial.log_level,
                &cwdc.and_then(|c| c.log_level.clone()),
                &lc.and_then(|c| c.log_level.clone()),
                &uc.and_then(|c| c.log_level.clone()),
            ],
            defaults.log_level,
        ),
        max_steps: env_partial
            .max_steps
            .or_else(|| cwdc.and_then(|c| c.max_steps))
            .or_else(|| lc.and_then(|c| c.max_steps))
            .or_else(|| uc.and_then(|c| c.max_steps))
            .or(defaults.max_steps),
        token_budget: env_partial
            .token_budget
            .or_else(|| cwdc.and_then(|c| c.token_budget))
            .or_else(|| lc.and_then(|c| c.token_budget))
            .or_else(|| uc.and_then(|c| c.token_budget))
            .or(defaults.token_budget),
        timeout_secs: env_partial
            .timeout_secs
            .or_else(|| cwdc.and_then(|c| c.timeout_secs))
            .or_else(|| lc.and_then(|c| c.timeout_secs))
            .or_else(|| uc.and_then(|c| c.timeout_secs))
            .or(defaults.timeout_secs),
        base_url: env_partial
            .base_url
            .or_else(|| cwdc.and_then(|c| c.base_url.clone()))
            .or_else(|| lc.and_then(|c| c.base_url.clone()))
            .or_else(|| uc.and_then(|c| c.base_url.clone())),
        trace: env_partial
            .trace
            .or_else(|| cwdc.and_then(|c| c.trace.clone()))
            .or_else(|| lc.and_then(|c| c.trace.clone()))
            .or_else(|| uc.and_then(|c| c.trace.clone())),
        json_output: false,
        auto_approve: false,
        context_window: env_partial
            .context_window
            .or_else(|| cwdc.and_then(|c| c.context_window))
            .or_else(|| lc.and_then(|c| c.context_window))
            .or_else(|| uc.and_then(|c| c.context_window))
            .unwrap_or(defaults.context_window),
        compact_threshold: env_partial
            .compact_threshold
            .or_else(|| cwdc.and_then(|c| c.compact_threshold))
            .or_else(|| lc.and_then(|c| c.compact_threshold))
            .or_else(|| uc.and_then(|c| c.compact_threshold))
            .unwrap_or(defaults.compact_threshold),
        compact_keep_last: env_partial
            .compact_keep_last
            .or_else(|| cwdc.and_then(|c| c.compact_keep_last))
            .or_else(|| lc.and_then(|c| c.compact_keep_last))
            .or_else(|| uc.and_then(|c| c.compact_keep_last))
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
        thinking: env_partial
            .thinking
            .or_else(|| cwdc.and_then(|c| c.thinking.clone()))
            .or_else(|| lc.and_then(|c| c.thinking.clone()))
            .or_else(|| uc.and_then(|c| c.thinking.clone())),
        system_prompt: env_partial
            .system_prompt
            .or_else(|| cwdc.and_then(|c| c.system_prompt.clone()))
            .or_else(|| lc.and_then(|c| c.system_prompt.clone()))
            .or_else(|| uc.and_then(|c| c.system_prompt.clone())),
        preload_skills: Vec::new(),
        notes: cwdc
            .and_then(|c| c.notes.clone())
            .or_else(|| lc.and_then(|c| c.notes.clone()))
            .or_else(|| uc.and_then(|c| c.notes.clone()))
            .unwrap_or_default(),
        cli_prompt: None,
    };

    Ok(cfg)
}
pub fn persist_domain(domain: &str) {
    persist_policy_array("allowed_domains", domain);
}

/// Persist a new command to the user's ~/.rune/rune.toml allowed_commands list.
/// Best-effort: if the file can't be read/written, silently skip.
pub fn persist_command(command: &str) {
    persist_policy_array("allowed_commands", command);
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
pub fn persist_policy_array(field: &str, value: &str) {
    let config_path = match env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".rune").join("rune.toml"),
        Err(_) => return,
    };
    persist_policy_array_at(&config_path, field, value);
}

/// Inner helper: persist a value into a policy array field at an explicit config path.
/// Used by tests to avoid mutating the global HOME env var (which is not thread-safe).
pub fn persist_policy_array_at(config_path: &std::path::Path, field: &str, value: &str) {
    let content = match fs::read_to_string(config_path) {
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
    let _ = fs::write(config_path, new_content);
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
        assert!(p.allowed_syscalls.is_empty()); // empty = block all dangerous syscalls
        assert!(p.allowed_paths_rw.contains(&"/tmp".to_string()));
        assert!(p.allowed_paths_ro.contains(&"/bin".to_string()));
        assert!(p.denied_paths.contains(&"/root".to_string()));
        assert_eq!(p.max_memory_mb, 512);
        assert_eq!(p.max_pids, 64);
    }

    #[test]
    fn test_rune_config_default() {
        let c = RuneConfig::default();
        assert_eq!(c.model, "");
        assert!(c.api_key.is_none());
        assert_eq!(c.skills_dir, "./skills");
        assert_eq!(c.log_level, "error");
        assert_eq!(c.max_steps, Some(50));
        assert_eq!(c.token_budget, None); // Default: no cost guard limit
        assert_eq!(c.timeout_secs, Some(30));
        assert!(c.base_url.is_none());
        assert!(c.trace.is_none());
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

        persist_policy_array_at(&config_path, "allowed_domains", "new-domain.com");

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

        persist_policy_array_at(&config_path, "allowed_domains", "github.com"); // already exists

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

        persist_policy_array_at(&config_path, "allowed_commands", "cargo");

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

        persist_policy_array_at(&config_path, "allowed_paths_ro", "/home/user/project");

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("/home/user/project"));
        assert!(content.contains("allowed_paths_ro"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_provider_field() {
        let toml_str = r#"
model = "gemini-pro"
api_key = "AIzaXXXX"
skills_dir = "./skills"
log_level = "info"
provider = "gemini"
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.provider, Some("gemini".to_string()));
    }

    #[test]
    fn test_config_provider_field_missing_is_none() {
        let toml_str = r#"
model = "gpt-4"
api_key = "sk-xxx"
skills_dir = "./skills"
log_level = "info"
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.provider, None);
    }

    #[test]
    fn test_preload_skills_default_empty() {
        let cfg = RuneConfig::default();
        assert!(cfg.preload_skills.is_empty());
    }

    #[test]
    fn test_preload_skills_not_serialized() {
        // preload_skills is marked #[serde(skip)] so it should not appear in TOML
        let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        // PartialConfig doesn't have preload_skills, confirming it's CLI-only
        assert!(cfg.skills_dir.is_some());
    }

    #[test]
    fn test_system_prompt_from_toml() {
        let toml_str = r#"
model = "gpt-4"
system_prompt = "You are a custom agent."
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            cfg.system_prompt,
            Some("You are a custom agent.".to_string())
        );
    }

    #[test]
    fn test_system_prompt_default_none() {
        let cfg = RuneConfig::default();
        assert!(cfg.system_prompt.is_none());
    }

    #[test]
    fn test_system_prompt_missing_in_toml_is_none() {
        let toml_str = r#"
model = "gpt-4"
log_level = "info"
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.system_prompt.is_none());
    }

    // --- expand_tilde tests ---

    #[test]
    fn test_expand_tilde_home_prefix() {
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(expand_tilde("~/skills"), "/home/testuser/skills");
        assert_eq!(expand_tilde("~/a/b/c"), "/home/testuser/a/b/c");
    }

    #[test]
    fn test_expand_tilde_bare_tilde() {
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(expand_tilde("~"), "/home/testuser");
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
        assert_eq!(expand_tilde("./local"), "./local");
        assert_eq!(expand_tilde(""), "");
    }

    #[test]
    fn test_expand_tilde_not_at_start() {
        // ~ not at start should not be expanded
        assert_eq!(expand_tilde("/home/~/weird"), "/home/~/weird");
        assert_eq!(expand_tilde("foo~/bar"), "foo~/bar");
    }

    #[test]
    fn test_expand_tilde_vec_mixed() {
        std::env::set_var("HOME", "/home/u");
        let mut v = vec![
            "~/skills".to_string(),
            "/absolute".to_string(),
            "relative".to_string(),
            "~/other/dir".to_string(),
        ];
        expand_tilde_vec(&mut v);
        assert_eq!(
            v,
            vec![
                "/home/u/skills",
                "/absolute",
                "relative",
                "/home/u/other/dir",
            ]
        );
    }

    // =========================================================
    // Additional tests for increased coverage
    // =========================================================

    #[test]
    fn test_serve_config_default() {
        let s = NotesConfig::default();
        assert!(s.port.is_none());
        assert!(s.bind.is_none());
        assert!(s.user_token.is_none());
        assert!(s.admin_token.is_none());
    }

    #[test]
    fn test_serve_config_toml_parsing() {
        let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[notes]
port = 9527
bind = "0.0.0.0"
user_token = "secret"
admin_token = "admin_secret"
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        let serve = cfg.notes.unwrap();
        assert_eq!(serve.port, Some(9527));
        assert_eq!(serve.bind.as_deref(), Some("0.0.0.0"));
        assert_eq!(serve.user_token.as_deref(), Some("secret"));
        assert_eq!(serve.admin_token.as_deref(), Some("admin_secret"));
    }

    #[test]
    fn test_partial_config_all_fields() {
        let toml_str = r#"
model = "claude-opus-4"
api_key = "sk-ant-xxx"
provider = "anthropic"
skills_dir = "~/.rune/skills"
log_level = "debug"
max_steps = 100
token_budget = 500000
timeout_secs = 60
base_url = "https://api.anthropic.com/v1"
trace = "/tmp/rune-trace"
context_window = 200000
compact_threshold = 0.9
compact_keep_last = 10
thinking = "high"
system_prompt = "You are an expert."
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(cfg.api_key.as_deref(), Some("sk-ant-xxx"));
        assert_eq!(cfg.provider.as_deref(), Some("anthropic"));
        assert_eq!(cfg.skills_dir.as_deref(), Some("~/.rune/skills"));
        assert_eq!(cfg.log_level.as_deref(), Some("debug"));
        assert_eq!(cfg.max_steps, Some(100));
        assert_eq!(cfg.token_budget, Some(500000));
        assert_eq!(cfg.timeout_secs, Some(60));
        assert_eq!(
            cfg.base_url.as_deref(),
            Some("https://api.anthropic.com/v1")
        );
        assert_eq!(cfg.trace.as_deref(), Some("/tmp/rune-trace"));
        assert_eq!(cfg.context_window, Some(200000));
        assert_eq!(cfg.compact_threshold, Some(0.9));
        assert_eq!(cfg.compact_keep_last, Some(10));
        assert_eq!(cfg.thinking.as_deref(), Some("high"));
        assert_eq!(cfg.system_prompt.as_deref(), Some("You are an expert."));
    }

    #[test]
    fn test_rune_config_default_compact_fields() {
        let c = RuneConfig::default();
        assert_eq!(c.context_window, 128000);
        assert!((c.compact_threshold - 0.85).abs() < 1e-9);
        assert_eq!(c.compact_keep_last, 6);
    }

    #[test]
    fn test_rune_config_default_thinking_none() {
        let c = RuneConfig::default();
        assert!(c.thinking.is_none());
    }

    #[test]
    fn test_rune_config_default_provider_none() {
        let c = RuneConfig::default();
        assert!(c.provider.is_none());
    }

    #[test]
    fn test_policy_config_deserialization_minimal() {
        let toml_str = r#"
mode = "allowlist"
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.mode, "allowlist");
        assert!(policy.allowed_commands.is_empty());
        assert_eq!(policy.max_memory_mb, 0);
        assert_eq!(policy.max_pids, 0);
    }

    #[test]
    fn test_policy_config_deserialization_full() {
        let toml_str = r#"
mode = "unrestricted"
allowed_commands = ["git", "cargo"]
allowed_domains = ["github.com"]
allowed_syscalls = ["ptrace"]
allowed_paths_rw = ["/workspace", "/tmp"]
allowed_paths_ro = ["/usr", "/bin"]
allowed_files_ro = ["/etc/hostname"]
allowed_files_rw = ["/tmp/out.txt"]
denied_paths = ["/etc/shadow", "/root"]
max_memory_mb = 1024
max_pids = 128
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.mode, "unrestricted");
        assert_eq!(policy.allowed_commands, vec!["git", "cargo"]);
        assert_eq!(policy.allowed_domains, vec!["github.com"]);
        assert_eq!(policy.allowed_syscalls, vec!["ptrace"]);
        assert_eq!(policy.max_memory_mb, 1024);
        assert_eq!(policy.max_pids, 128);
    }

    #[test]
    fn test_load_toml_with_serve_section() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-srv-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4"
[notes]
port = 8080
bind = "127.0.0.1"
"#,
        )
        .unwrap();
        let partial = load_toml(&path).expect("should parse");
        let serve = partial.notes.unwrap();
        assert_eq!(serve.port, Some(8080));
        assert_eq!(serve.bind.as_deref(), Some("127.0.0.1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_policy_array_file_not_found() {
        // Should silently skip, not panic
        let path = std::path::Path::new("/nonexistent/path/rune.toml");
        persist_policy_array_at(path, "allowed_domains", "test.com");
        // No panic = success
    }

    #[test]
    fn test_persist_policy_array_invalid_toml() {
        let dir = std::env::temp_dir().join(format!("rune-persist-inv-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        fs::write(&config_path, "not valid toml {{{{").unwrap();
        // Should silently skip
        persist_policy_array_at(&config_path, "allowed_domains", "test.com");
        // Content unchanged (parse failed, skip)
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("not valid toml"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_policy_array_creates_policy_section() {
        let dir = std::env::temp_dir().join(format!("rune-persist-cre-{}", std::process::id()));
        let rune_dir = dir.join(".rune");
        let _ = fs::create_dir_all(&rune_dir);
        let config_path = rune_dir.join("rune.toml");
        // File without any policy section
        fs::write(&config_path, r#"model = "gpt-4""#).unwrap();
        persist_policy_array_at(&config_path, "allowed_commands", "rustfmt");
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("rustfmt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_persist_multiple_fields() {
        let dir = std::env::temp_dir().join(format!("rune-persist-mf-{}", std::process::id()));
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
        persist_policy_array_at(&config_path, "allowed_commands", "git");
        persist_policy_array_at(&config_path, "allowed_commands", "cargo");
        persist_policy_array_at(&config_path, "allowed_domains", "github.com");
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(
            content.contains("\"git\"") || content.contains("'git'") || content.contains("git")
        );
        assert!(content.contains("github.com"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_expand_tilde_home_not_set() {
        // Temporarily unset HOME — if HOME is absent, return original
        let original = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let result = expand_tilde("~/skills");
        assert_eq!(result, "~/skills"); // unchanged since HOME absent
                                        // Restore
        if let Some(h) = original {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn test_expand_tilde_bare_home_not_set() {
        let original = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let result = expand_tilde("~");
        assert_eq!(result, "~");
        if let Some(h) = original {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn test_rune_config_clone() {
        let c = RuneConfig::default();
        let c2 = c.clone();
        assert_eq!(c2.model, c.model);
        assert_eq!(c2.log_level, c.log_level);
    }

    #[test]
    fn test_policy_config_clone() {
        let p = PolicyConfig::default();
        let p2 = p.clone();
        assert_eq!(p2.mode, p.mode);
        assert_eq!(p2.allowed_paths_rw, p.allowed_paths_rw);
    }

    #[test]
    fn test_load_toml_with_context_window() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-cw-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4o"
context_window = 32000
compact_threshold = 0.75
compact_keep_last = 4
"#,
        )
        .unwrap();
        let partial = load_toml(&path).unwrap();
        assert_eq!(partial.context_window, Some(32000));
        assert_eq!(partial.compact_threshold, Some(0.75));
        assert_eq!(partial.compact_keep_last, Some(4));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_without_clap_returns_ok() {
        // Should not panic/fail even without any config files
        // We just test that it returns Ok
        let result = load_without_clap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_without_clap_env_model_override() {
        std::env::set_var("RUNE_MODEL", "env-model-test-xyz");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.model, "env-model-test-xyz");
        std::env::remove_var("RUNE_MODEL");
    }

    #[test]
    fn test_load_without_clap_env_api_key() {
        std::env::set_var("RUNE_API_KEY", "sk-test-env-key");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test-env-key"));
        std::env::remove_var("RUNE_API_KEY");
    }

    #[test]
    fn test_load_without_clap_env_provider() {
        std::env::set_var("RUNE_PROVIDER", "ollama");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.provider.as_deref(), Some("ollama"));
        std::env::remove_var("RUNE_PROVIDER");
    }

    #[test]
    fn test_load_without_clap_env_log_level() {
        std::env::set_var("RUNE_LOG_LEVEL", "trace");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.log_level, "trace");
        std::env::remove_var("RUNE_LOG_LEVEL");
    }

    #[test]
    fn test_load_without_clap_env_max_steps() {
        std::env::set_var("RUNE_MAX_STEPS", "99");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.max_steps, Some(99));
        std::env::remove_var("RUNE_MAX_STEPS");
    }

    #[test]
    fn test_load_without_clap_env_context_window() {
        std::env::set_var("RUNE_CONTEXT_WINDOW", "64000");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.context_window, 64000);
        std::env::remove_var("RUNE_CONTEXT_WINDOW");
    }

    #[test]
    fn test_load_without_clap_env_thinking() {
        std::env::set_var("RUNE_THINKING", "medium");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.thinking.as_deref(), Some("medium"));
        std::env::remove_var("RUNE_THINKING");
    }

    #[test]
    fn test_load_without_clap_json_auto_approve_false() {
        let cfg = load_without_clap().unwrap();
        assert!(!cfg.json_output);
        assert!(!cfg.auto_approve);
    }

    #[test]
    fn test_load_toml_with_thinking() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-th-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "claude-opus-4"
thinking = "high"
"#,
        )
        .unwrap();
        let partial = load_toml(&path).unwrap();
        assert_eq!(partial.thinking.as_deref(), Some("high"));
        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================
    // Additional tests for broader coverage
    // =========================================================

    #[test]
    fn test_load_without_clap_env_token_budget() {
        std::env::set_var("RUNE_TOKEN_BUDGET", "50000");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.token_budget, Some(50000));
        std::env::remove_var("RUNE_TOKEN_BUDGET");
    }

    #[test]
    fn test_load_without_clap_env_timeout_secs() {
        std::env::set_var("RUNE_TIMEOUT_SECS", "120");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.timeout_secs, Some(120));
        std::env::remove_var("RUNE_TIMEOUT_SECS");
    }

    #[test]
    fn test_load_without_clap_env_base_url() {
        std::env::set_var("RUNE_BASE_URL", "http://localhost:11434");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.base_url.as_deref(), Some("http://localhost:11434"));
        std::env::remove_var("RUNE_BASE_URL");
    }

    #[test]
    fn test_load_without_clap_env_system_prompt() {
        std::env::set_var("RUNE_SYSTEM_PROMPT", "Be concise.");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.system_prompt.as_deref(), Some("Be concise."));
        std::env::remove_var("RUNE_SYSTEM_PROMPT");
    }

    #[test]
    fn test_load_without_clap_env_compact_threshold() {
        std::env::set_var("RUNE_COMPACT_THRESHOLD", "0.70");
        let cfg = load_without_clap().unwrap();
        assert!((cfg.compact_threshold - 0.70).abs() < 1e-5);
        std::env::remove_var("RUNE_COMPACT_THRESHOLD");
    }

    #[test]
    fn test_load_without_clap_env_compact_keep_last() {
        std::env::set_var("RUNE_COMPACT_KEEP_LAST", "8");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.compact_keep_last, 8);
        std::env::remove_var("RUNE_COMPACT_KEEP_LAST");
    }

    #[test]
    fn test_load_without_clap_env_trace() {
        std::env::set_var("RUNE_TRACE", "/tmp/rune-trace-test");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.trace.as_deref(), Some("/tmp/rune-trace-test"));
        std::env::remove_var("RUNE_TRACE");
    }

    #[test]
    fn test_load_without_clap_env_skills_dir() {
        std::env::set_var("RUNE_SKILLS_DIR", "/custom/skills");
        let cfg = load_without_clap().unwrap();
        assert_eq!(cfg.skills_dir, "/custom/skills");
        std::env::remove_var("RUNE_SKILLS_DIR");
    }

    #[test]
    fn test_mcp_server_config_toml_parsing() {
        let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[[mcp_servers]]
name = "my-mcp"
command = "/usr/bin/my-mcp"
args = ["--port", "9000"]
required = true
timeout_secs = 10

[[mcp_servers]]
name = "optional-mcp"
command = "optional-mcp-server"
required = false
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        let servers = cfg.mcp_servers.unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "my-mcp");
        assert_eq!(servers[0].command, "/usr/bin/my-mcp");
        assert_eq!(servers[0].args, vec!["--port", "9000"]);
        assert!(servers[0].required);
        assert_eq!(servers[0].timeout_secs, Some(10));
        assert_eq!(servers[1].name, "optional-mcp");
        assert!(!servers[1].required);
    }

    #[test]
    fn test_mcp_server_config_default_timeout() {
        let toml_str = r#"
name = "test"
command = "test-cmd"
"#;
        let srv: crate::mcp::McpServerConfig = toml::from_str(toml_str).unwrap();
        // default_timeout returns Some(30)
        assert_eq!(srv.timeout_secs, Some(30));
        assert!(!srv.required);
        assert!(srv.args.is_empty());
    }

    #[test]
    fn test_embedding_config_toml_parsing() {
        let toml_str = r#"
model = "gpt-4"
skills_dir = "./skills"
log_level = "info"

[embedding]
enabled = true
model = "text-embedding-3-small"
base_url = "https://api.openai.com/v1"
api_key = "sk-embed-xxx"
threshold = 0.5
max_skills = 5
"#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        let emb = cfg.embedding.unwrap();
        assert!(emb.enabled);
        assert_eq!(emb.model.as_deref(), Some("text-embedding-3-small"));
        assert_eq!(emb.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(emb.api_key.as_deref(), Some("sk-embed-xxx"));
        assert!((emb.threshold - 0.5).abs() < 1e-5);
        assert_eq!(emb.max_skills, 5);
    }

    #[test]
    fn test_embedding_config_default() {
        let emb = crate::embedding::EmbeddingConfig::default();
        assert!(!emb.enabled); // default is false
        assert!((emb.threshold - 0.3).abs() < 1e-5);
    }

    #[test]
    fn test_policy_config_allowed_files() {
        let toml_str = r#"
mode = "allowlist"
allowed_files_ro = ["/etc/hostname", "/etc/resolv.conf"]
allowed_files_rw = ["/tmp/output.log"]
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            policy.allowed_files_ro,
            vec!["/etc/hostname", "/etc/resolv.conf"]
        );
        assert_eq!(policy.allowed_files_rw, vec!["/tmp/output.log"]);
    }

    #[test]
    fn test_policy_config_denied_paths() {
        let toml_str = r#"
mode = "allowlist"
denied_paths = ["/root", "/home/secret"]
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.denied_paths, vec!["/root", "/home/secret"]);
    }

    #[test]
    fn test_policy_config_memory_limits() {
        let toml_str = r#"
mode = "confirm"
max_memory_mb = 2048
max_pids = 256
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.max_memory_mb, 2048);
        assert_eq!(policy.max_pids, 256);
    }

    #[test]
    fn test_policy_mode_default_when_omitted() {
        // When policy is deserialized without explicit mode, default_policy_mode applies
        let toml_str = r#"
allowed_commands = ["ls"]
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.mode, "confirm");
    }

    #[test]
    fn test_load_toml_with_mcp_servers() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-mcp-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4"

[[mcp_servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
required = false
"#,
        )
        .unwrap();
        let partial = load_toml(&path).unwrap();
        let servers = partial.mcp_servers.unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "filesystem");
        assert_eq!(servers[0].command, "npx");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_toml_with_embedding() {
        let dir = std::env::temp_dir().join(format!("rune-cfg-emb-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rune.toml");
        fs::write(
            &path,
            r#"
model = "gpt-4"

[embedding]
enabled = false
model = "nomic-embed-text"
threshold = 0.4
"#,
        )
        .unwrap();
        let partial = load_toml(&path).unwrap();
        let emb = partial.embedding.unwrap();
        assert!(!emb.enabled);
        assert_eq!(emb.model.as_deref(), Some("nomic-embed-text"));
        assert!((emb.threshold - 0.4).abs() < 1e-5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_expand_tilde_vec_empty() {
        let mut v: Vec<String> = vec![];
        expand_tilde_vec(&mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn test_expand_tilde_vec_single_no_tilde() {
        let mut v = vec!["/absolute/path".to_string()];
        expand_tilde_vec(&mut v);
        assert_eq!(v, vec!["/absolute/path"]);
    }

    #[test]
    fn test_policy_syscalls_default_empty() {
        let p = PolicyConfig::default();
        assert!(p.allowed_syscalls.is_empty());
    }

    #[test]
    fn test_policy_config_allowed_syscalls() {
        let toml_str = r#"
mode = "allowlist"
allowed_syscalls = ["ptrace", "bpf"]
"#;
        let policy: PolicyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.allowed_syscalls, vec!["ptrace", "bpf"]);
    }

    #[test]
    fn test_rune_config_default_mcp_empty() {
        let c = RuneConfig::default();
        assert!(c.mcp_servers.is_empty());
    }

    #[test]
    fn test_rune_config_default_serve() {
        let c = RuneConfig::default();
        assert!(c.notes.port.is_none());
        assert!(c.notes.user_token.is_none());
    }

    #[test]
    fn test_load_without_clap_preload_skills_empty() {
        let cfg = load_without_clap().unwrap();
        assert!(cfg.preload_skills.is_empty());
    }

    #[test]
    fn test_partial_config_missing_optional_fields() {
        // Only model required to parse — all optional fields should be None
        let toml_str = r#"model = "gpt-4""#;
        let cfg: PartialConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
        assert!(cfg.api_key.is_none());
        assert!(cfg.provider.is_none());
        assert!(cfg.skills_dir.is_none());
        assert!(cfg.log_level.is_none());
        assert!(cfg.max_steps.is_none());
        assert!(cfg.token_budget.is_none());
        assert!(cfg.timeout_secs.is_none());
        assert!(cfg.base_url.is_none());
        assert!(cfg.trace.is_none());
        assert!(cfg.context_window.is_none());
        assert!(cfg.compact_threshold.is_none());
        assert!(cfg.compact_keep_last.is_none());
        assert!(cfg.policy.is_none());
        assert!(cfg.mcp_servers.is_none());
        assert!(cfg.embedding.is_none());
        assert!(cfg.thinking.is_none());
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.notes.is_none());
    }

    #[test]
    fn test_cli_prompt_default_none() {
        let cfg = RuneConfig::default();
        assert!(cfg.cli_prompt.is_none());
    }

    #[test]
    fn test_cli_prompt_not_deserialized() {
        // cli_prompt is #[serde(skip)], should never come from TOML
        let toml = r#"
            model = "gpt-4"
            cli_prompt = "should be ignored"
        "#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        // PartialConfig doesn't have cli_prompt at all, confirming it's CLI-only
        assert!(partial.model == Some("gpt-4".to_string()));
    }
}
