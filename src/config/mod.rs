use anyhow::Context;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::PathBuf;

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
        }
    }
}

/// Partial config used for layered merging.
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
}

/// CLI argument overrides.
#[derive(Debug, clap::Parser)]
#[command(name = "rune", about = "High-performance zero-trust AI Agent")]
struct CliArgs {
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    skills_dir: Option<String>,
    #[arg(long)]
    log_level: Option<String>,
    #[arg(long)]
    max_steps: Option<u32>,
    #[arg(long)]
    token_budget: Option<u32>,
    #[arg(long)]
    timeout_secs: Option<u64>,
    #[arg(long)]
    base_url: Option<String>,
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
    };

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
    })
}
