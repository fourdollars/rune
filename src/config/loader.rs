use super::types::{PartialConfig, PolicyConfig, RuneConfig};
use super::util::{expand_tilde, expand_tilde_vec};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// CLI argument overrides.
#[derive(Debug, clap::Parser)]
#[command(
    name = "rune",
    version = concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("GIT_HASH"),
        " ",
        env!("BUILD_DATE"),
        ")"
    ),
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
  read_file, write_file, list_dir, execute_cmd, fetch_url\n\
\n\
SANDBOX LAYERS:\n\
  1. cgroups v2 (memory + process limits)\n\
  2. net-guard (seccomp user notification — per-domain network filter)\n\
  3. seccomp BPF (syscall filter)\n\
  4. landlock (filesystem restriction)\n\
  5. DNS allowlist (wildcard domain support)"
)]
pub(crate) struct CliArgs {
    /// Path to rune.toml config file [highest priority, hard-fails if missing or invalid]
    #[arg(
        long,
        short = 'c',
        env = "RUNE_CONFIG",
        value_name = "path/rune.toml",
        help_heading = "Configuration"
    )]
    pub(crate) config: Option<String>,

    /// LLM provider [github-copilot, gemini, openai, openrouter, ollama, anthropic]
    #[arg(long, env = "RUNE_PROVIDER", help_heading = "Provider")]
    pub(crate) provider: Option<String>,

    /// Model name [e.g. gpt-4o, gemini-2.0-flash, claude-3.5-sonnet]
    #[arg(long, env = "RUNE_MODEL", help_heading = "Provider")]
    pub(crate) model: Option<String>,

    /// API key for the LLM provider
    #[arg(long, env = "RUNE_API_KEY", help_heading = "Provider")]
    pub(crate) api_key: Option<String>,

    /// Provider base URL (auto-detected for Copilot/Gemini)
    #[arg(long, env = "RUNE_BASE_URL", help_heading = "Provider")]
    pub(crate) base_url: Option<String>,

    /// Disable all security policy checks (sandbox, allowlists, confirm prompts)
    #[arg(long, help_heading = "Security")]
    pub(crate) unrestricted: bool,

    /// Auto-approve dangerous tool calls (does NOT bypass policy allowlist)
    #[arg(long, short = 'y', action = clap::ArgAction::SetTrue, help_heading = "Security")]
    pub(crate) yes: bool,

    /// Mount specified folder over real HOME directory with RW access (defaults to CWD if omitted)
    #[arg(
        long = "mount-home",
        short = 'H',
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = ".",
        help_heading = "Security"
    )]
    pub(crate) mount_home: Option<String>,

    /// Mount path(s) or file(s) as Read-Write in sandbox (defaults to CWD if omitted)
    #[arg(
        long = "mount-rw",
        alias = "mount-pwd",
        short = 'M',
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = ".",
        action = clap::ArgAction::Append,
        help_heading = "Security"
    )]
    pub(crate) mount_rw: Vec<String>,

    /// Mount path(s) or file(s) as Read-Only in sandbox (defaults to CWD if omitted)
    #[arg(
        long = "mount-ro",
        short = 'm',
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = ".",
        action = clap::ArgAction::Append,
        help_heading = "Security"
    )]
    pub(crate) mount_ro: Vec<String>,

    /// Maximum agent loop iterations [default: 50, 0 = unlimited]
    #[arg(long, env = "RUNE_MAX_STEPS", help_heading = "Limits")]
    pub(crate) max_steps: Option<u32>,

    /// Maximum tokens per run [default: 256k, 0 = unlimited]
    #[arg(long, env = "RUNE_TOKEN_BUDGET", help_heading = "Limits")]
    pub(crate) token_budget: Option<u32>,

    /// Command timeout in seconds [default: 30, 0 = unlimited]
    #[arg(long, env = "RUNE_TIMEOUT_SECS", help_heading = "Limits")]
    pub(crate) timeout_secs: Option<u64>,

    /// Output in JSON format (machine-readable, for scripting)
    #[arg(long, action = clap::ArgAction::SetTrue, help_heading = "Output")]
    pub(crate) json: bool,

    /// Enable trace recording to specified directory [empty = disabled]
    #[arg(long, env = "RUNE_TRACE", help_heading = "Output")]
    pub(crate) trace: Option<String>,

    /// Log level [trace, debug, info, warn, error]
    #[arg(long, env = "RUNE_LOG_LEVEL", help_heading = "Output")]
    pub(crate) log_level: Option<String>,

    /// Thinking/reasoning effort level [off|low|medium|high|xhigh]
    #[arg(long, env = "RUNE_THINKING", help_heading = "Advanced")]
    pub(crate) thinking: Option<String>,

    /// Directory containing skill definitions
    #[arg(long, env = "RUNE_SKILLS_DIR", help_heading = "Advanced")]
    pub(crate) skills_dir: Option<String>,

    /// Preload specific skills by name (comma-separated). Only these skills
    /// will be injected; @ref and semantic search are disabled.
    /// Example: --skills jira,launchpad
    #[arg(
        long,
        env = "RUNE_SKILLS",
        help_heading = "Advanced",
        value_delimiter = ','
    )]
    pub(crate) skills: Vec<String>,

    /// Custom system prompt (replaces default, AGENTS.md still appended)
    #[arg(long, env = "RUNE_SYSTEM_PROMPT", help_heading = "Advanced")]
    pub(crate) system_prompt: Option<String>,

    /// Enforce Zero Data Retention (ZDR) model filtering for OpenRouter
    #[arg(
        long,
        env = "RUNE_OPENROUTER_ZDR",
        action = clap::ArgAction::SetTrue,
        help_heading = "Advanced"
    )]
    pub(crate) openrouter_zdr: bool,

    /// Do not auto-load AGENTS.md from the current directory
    #[arg(
        long = "no-agents-md",
        env = "RUNE_NO_AGENTS_MD",
        action = clap::ArgAction::SetTrue,
        help_heading = "Input"
    )]
    pub(crate) no_agents_md: bool,

    /// Prompt to send (one-shot mode). Alternative to piping stdin.
    #[arg(long = "prompt", short = 'p', help_heading = "Input")]
    pub(crate) prompt: Option<String>,
}

pub(crate) fn parse_boolish(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

/// Pick the first Some value from a chain of options, falling back to a default.
pub(crate) fn pick<T: Clone>(sources: &[&Option<T>], default: T) -> T {
    for src in sources {
        if let Some(v) = src {
            return v.clone();
        }
    }
    default
}

/// Like pick but returns Option<T> — None if no source provides a value.
pub(crate) fn pick_option<T: Clone>(sources: &[&Option<T>]) -> Option<T> {
    for src in sources {
        if let Some(v) = src {
            return Some(v.clone());
        }
    }
    None
}

/// Load a TOML partial config from a path, returning None if it doesn't exist.
pub(crate) fn load_toml(path: &Path) -> Option<PartialConfig> {
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

/// Apply CLI mount flags (-H, -M, -m) to a PolicyConfig.
pub fn apply_mount_flags(
    policy: &mut PolicyConfig,
    mount_home: Option<&str>,
    mount_rw: &[String],
    mount_ro: &[String],
) {
    // CLI -H / --mount-home flag sets mount_home policy (default RW)
    if let Some(home_path) = mount_home {
        let expanded = expand_tilde(home_path);
        let resolved =
            std::fs::canonicalize(&expanded).unwrap_or_else(|_| PathBuf::from(&expanded));
        let resolved_str = resolved.to_string_lossy().to_string();
        if !policy
            .allowed_paths_rw
            .iter()
            .any(|p| resolved_str.starts_with(p.trim_end_matches('/')))
        {
            policy.allowed_paths_rw.push(resolved_str.clone());
        }
        policy.mount_home = Some(resolved_str);
    }

    // CLI -M / --mount-rw flag(s) set mount_pwd and add paths/files to RW allowlist
    if !mount_rw.is_empty() {
        policy.mount_pwd = true;
        for path_str in mount_rw {
            let expanded = expand_tilde(path_str);
            let resolved =
                std::fs::canonicalize(&expanded).unwrap_or_else(|_| PathBuf::from(&expanded));
            let abs_path = resolved.to_string_lossy().to_string();
            if resolved.is_file() {
                if !policy.allowed_files_rw.contains(&abs_path) {
                    policy.allowed_files_rw.push(abs_path);
                }
            } else if !policy
                .allowed_paths_rw
                .iter()
                .any(|p| abs_path.starts_with(p.trim_end_matches('/')))
            {
                policy.allowed_paths_rw.push(abs_path);
            }
        }
    }

    // CLI -m / --mount-ro flag(s) add paths/files to RO allowlist
    for path_str in mount_ro {
        let expanded = expand_tilde(path_str);
        let resolved =
            std::fs::canonicalize(&expanded).unwrap_or_else(|_| PathBuf::from(&expanded));
        let abs_path = resolved.to_string_lossy().to_string();
        if resolved.is_file() {
            if !policy.allowed_files_ro.contains(&abs_path) {
                policy.allowed_files_ro.push(abs_path);
            }
        } else if !policy
            .allowed_paths_ro
            .iter()
            .any(|p| abs_path.starts_with(p.trim_end_matches('/')))
        {
            policy.allowed_paths_ro.push(abs_path);
        }
    }

    if policy.mount_pwd {
        if let Ok(cwd) = env::current_dir() {
            let cwd_str = cwd.to_string_lossy().to_string();
            if !policy
                .allowed_paths_rw
                .iter()
                .any(|p| cwd_str.starts_with(p.trim_end_matches('/')))
            {
                policy.allowed_paths_rw.push(cwd_str);
            }
        }
    }
}

pub(crate) fn post_process_config(cfg: &mut RuneConfig) {
    cfg.skills_dir = expand_tilde(&cfg.skills_dir);
    if let Some(ref mut t) = cfg.trace {
        *t = expand_tilde(t);
    }
    expand_tilde_vec(&mut cfg.policy.allowed_paths_rw);
    expand_tilde_vec(&mut cfg.policy.allowed_paths_ro);
    expand_tilde_vec(&mut cfg.policy.allowed_files_ro);
    expand_tilde_vec(&mut cfg.policy.allowed_files_rw);
    expand_tilde_vec(&mut cfg.policy.denied_paths);
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
        compact_token_limit: env::var("RUNE_COMPACT_TOKEN_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok()),
        compact_keep_last: env::var("RUNE_COMPACT_KEEP_LAST")
            .ok()
            .and_then(|v| v.parse().ok()),
        policy: None, // Policy loaded from TOML only (too complex for single env var)
        mcp: None,
        embedding: None,
        thinking: env::var("RUNE_THINKING").ok(),
        system_prompt: env::var("RUNE_SYSTEM_PROMPT").ok(),
        monthly_budget: env::var("RUNE_MONTHLY_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok()),
        openrouter_zdr: env::var("RUNE_OPENROUTER_ZDR")
            .ok()
            .and_then(|v| parse_boolish(&v)),
        no_agents_md: env::var("RUNE_NO_AGENTS_MD")
            .ok()
            .and_then(|v| parse_boolish(&v)),
        notes: None,
        agents: None,
        loop_config: None,
    };
    let env_json_output = env::var("RUNE_JSON_OUTPUT")
        .ok()
        .and_then(|v| parse_boolish(&v));
    let env_auto_approve = env::var("RUNE_YES").ok().and_then(|v| parse_boolish(&v));
    let env_openrouter_zdr = env::var("RUNE_OPENROUTER_ZDR")
        .ok()
        .and_then(|v| parse_boolish(&v));
    let env_no_agents_md = env::var("RUNE_NO_AGENTS_MD")
        .ok()
        .and_then(|v| parse_boolish(&v));

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

    apply_mount_flags(
        &mut policy,
        cli.mount_home.as_deref(),
        &cli.mount_rw,
        &cli.mount_ro,
    );
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
        openrouter_zdr: cli.openrouter_zdr
            || env_openrouter_zdr.unwrap_or(
                ec.and_then(|c| c.openrouter_zdr)
                    .or(env_partial.openrouter_zdr)
                    .or(cwdc.and_then(|c| c.openrouter_zdr))
                    .or(lc.and_then(|c| c.openrouter_zdr))
                    .or(uc.and_then(|c| c.openrouter_zdr))
                    .unwrap_or(defaults.openrouter_zdr),
            ),
        no_agents_md: cli.no_agents_md
            || env_no_agents_md.unwrap_or(
                ec.and_then(|c| c.no_agents_md)
                    .or(env_partial.no_agents_md)
                    .or(cwdc.and_then(|c| c.no_agents_md))
                    .or(lc.and_then(|c| c.no_agents_md))
                    .or(uc.and_then(|c| c.no_agents_md))
                    .unwrap_or(defaults.no_agents_md),
            ),
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
        compact_token_limit: ec
            .and_then(|c| c.compact_token_limit)
            .or(env_partial.compact_token_limit)
            .or(cwdc.and_then(|c| c.compact_token_limit))
            .or(lc.and_then(|c| c.compact_token_limit))
            .or(uc.and_then(|c| c.compact_token_limit))
            .or(defaults.compact_token_limit),
        compact_keep_last: ec
            .and_then(|c| c.compact_keep_last)
            .or(env_partial.compact_keep_last)
            .or(cwdc.and_then(|c| c.compact_keep_last))
            .or(lc.and_then(|c| c.compact_keep_last))
            .or(uc.and_then(|c| c.compact_keep_last))
            .unwrap_or(defaults.compact_keep_last),
        policy,
        mcp: ec
            .and_then(|c| c.mcp.clone())
            .or_else(|| cwdc.and_then(|c| c.mcp.clone()))
            .or_else(|| lc.and_then(|c| c.mcp.clone()))
            .or_else(|| uc.and_then(|c| c.mcp.clone()))
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
        monthly_budget: ec
            .and_then(|c| c.monthly_budget)
            .or(cwdc.and_then(|c| c.monthly_budget))
            .or(lc.and_then(|c| c.monthly_budget))
            .or(uc.and_then(|c| c.monthly_budget)),
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
        agents: ec
            .and_then(|c| c.agents.clone())
            .or_else(|| cwdc.and_then(|c| c.agents.clone()))
            .or_else(|| lc.and_then(|c| c.agents.clone()))
            .or_else(|| uc.and_then(|c| c.agents.clone()))
            .unwrap_or_default(),
        loop_config: ec
            .and_then(|c| c.loop_config.clone())
            .or_else(|| cwdc.and_then(|c| c.loop_config.clone()))
            .or_else(|| lc.and_then(|c| c.loop_config.clone()))
            .or_else(|| uc.and_then(|c| c.loop_config.clone()))
            .unwrap_or_default(),
        cli_prompt: cli.prompt.clone(),
    };

    post_process_config(&mut cfg);

    Ok(cfg)
}

/// Load configuration without clap CLI arg parsing.
/// Used by `rune serve` to avoid clap choking on unknown subcommands.
/// Reads: env vars > ./rune.toml > .rune/rune.toml > ~/.rune/rune.toml > defaults
pub fn load_without_clap_path(
    override_path: Option<&std::path::Path>,
) -> anyhow::Result<RuneConfig> {
    if let Some(path) = override_path {
        if let Some(partial) = load_toml(path) {
            let mut cfg = load_without_clap()?;
            if let Some(notes) = partial.notes {
                cfg.notes = notes;
            }
            return Ok(cfg);
        }
    }
    load_without_clap()
}

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
        compact_token_limit: env::var("RUNE_COMPACT_TOKEN_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok()),
        compact_keep_last: env::var("RUNE_COMPACT_KEEP_LAST")
            .ok()
            .and_then(|v| v.parse().ok()),
        policy: None,
        mcp: None,
        embedding: None,
        thinking: env::var("RUNE_THINKING").ok(),
        system_prompt: env::var("RUNE_SYSTEM_PROMPT").ok(),
        monthly_budget: env::var("RUNE_MONTHLY_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok()),
        openrouter_zdr: env::var("RUNE_OPENROUTER_ZDR")
            .ok()
            .and_then(|v| parse_boolish(&v)),
        no_agents_md: env::var("RUNE_NO_AGENTS_MD")
            .ok()
            .and_then(|v| parse_boolish(&v)),
        notes: None,
        agents: None,
        loop_config: None,
    };
    let env_openrouter_zdr = env::var("RUNE_OPENROUTER_ZDR")
        .ok()
        .and_then(|v| parse_boolish(&v));
    let env_no_agents_md = env::var("RUNE_NO_AGENTS_MD")
        .ok()
        .and_then(|v| parse_boolish(&v));

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

    let mut cfg = RuneConfig {
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
        openrouter_zdr: env_openrouter_zdr.unwrap_or(
            cwdc.and_then(|c| c.openrouter_zdr)
                .or(env_partial.openrouter_zdr)
                .or(lc.and_then(|c| c.openrouter_zdr))
                .or(uc.and_then(|c| c.openrouter_zdr))
                .unwrap_or(defaults.openrouter_zdr),
        ),
        no_agents_md: env_no_agents_md.unwrap_or(
            cwdc.and_then(|c| c.no_agents_md)
                .or(env_partial.no_agents_md)
                .or(lc.and_then(|c| c.no_agents_md))
                .or(uc.and_then(|c| c.no_agents_md))
                .unwrap_or(defaults.no_agents_md),
        ),
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
        compact_token_limit: env_partial
            .compact_token_limit
            .or_else(|| cwdc.and_then(|c| c.compact_token_limit))
            .or_else(|| lc.and_then(|c| c.compact_token_limit))
            .or_else(|| uc.and_then(|c| c.compact_token_limit))
            .or(defaults.compact_token_limit),
        compact_keep_last: env_partial
            .compact_keep_last
            .or_else(|| cwdc.and_then(|c| c.compact_keep_last))
            .or_else(|| lc.and_then(|c| c.compact_keep_last))
            .or_else(|| uc.and_then(|c| c.compact_keep_last))
            .unwrap_or(defaults.compact_keep_last),
        policy,
        mcp: cwdc
            .and_then(|c| c.mcp.clone())
            .or_else(|| lc.and_then(|c| c.mcp.clone()))
            .or_else(|| uc.and_then(|c| c.mcp.clone()))
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
        monthly_budget: env_partial
            .monthly_budget
            .or_else(|| cwdc.and_then(|c| c.monthly_budget))
            .or_else(|| lc.and_then(|c| c.monthly_budget))
            .or_else(|| uc.and_then(|c| c.monthly_budget)),
        preload_skills: Vec::new(),
        notes: cwdc
            .and_then(|c| c.notes.clone())
            .or_else(|| lc.and_then(|c| c.notes.clone()))
            .or_else(|| uc.and_then(|c| c.notes.clone()))
            .unwrap_or_default(),
        agents: cwdc
            .and_then(|c| c.agents.clone())
            .or_else(|| lc.and_then(|c| c.agents.clone()))
            .or_else(|| uc.and_then(|c| c.agents.clone()))
            .unwrap_or_default(),
        loop_config: cwdc
            .and_then(|c| c.loop_config.clone())
            .or_else(|| lc.and_then(|c| c.loop_config.clone()))
            .or_else(|| uc.and_then(|c| c.loop_config.clone()))
            .unwrap_or_default(),
        cli_prompt: None,
    };

    post_process_config(&mut cfg);

    Ok(cfg)
}
