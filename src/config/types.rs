use serde::Deserialize;

fn default_policy_mode() -> String {
    "allowlist".to_string()
}

fn default_max_tmp_mb() -> u64 {
    100
}

/// Unified sandbox/security policy.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    /// Execution mode:
    /// - "allowlist": default — auto-executes within allowlist, blocks the rest
    /// - "confirm": interactive prompt mode — prompts user before dangerous tools
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
    /// Dynamically mount working directory as read-write and set default sandbox pwd to CWD.
    #[serde(default)]
    pub mount_pwd: bool,
    /// Mount custom directory as HOME in sandbox.
    #[serde(default)]
    pub mount_home: Option<String>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: "allowlist".to_string(),
            allowed_commands: Vec::new(),
            allowed_domains: Vec::new(),
            allowed_syscalls: Vec::new(),
            allowed_paths_rw: Vec::new(),
            allowed_paths_ro: Vec::new(),
            allowed_files_ro: Vec::new(),
            allowed_files_rw: Vec::new(),
            denied_paths: vec!["/root".to_string(), "/etc/shadow".to_string()],
            max_memory_mb: 512,
            max_pids: 64,
            max_tmp_mb: 100,
            mount_pwd: false,
            mount_home: None,
        }
    }
}

fn default_mcp_lenient_true() -> bool {
    true
}

/// Configuration for `rune serve` mode.
#[derive(Debug, Clone, Deserialize)]
pub struct NotesConfig {
    /// Port to listen on (default: 9527).
    pub port: Option<u16>,
    /// Bind address (default: 127.0.0.1).
    pub bind: Option<String>,
    /// Model(s) to use for notes mode. Can be a single model or comma-separated list of allowed models (e.g. "openrouter/auto" or "openrouter/auto,deepseek/deepseek-chat"). If not set or empty, defaults to auto-detecting models from provider.
    pub model: Option<String>,
    /// GitHub OAuth configuration. Required for serve mode.
    pub github: Option<GitHubOAuthConfig>,
    /// Local credentials configuration.
    pub local: Option<LocalConfig>,
    /// Generic third-party OAuth2/OIDC providers configuration.
    #[serde(default)]
    pub oauth: Vec<OAuthProviderConfig>,

    /// Enable general agent tools (read_file, write_file, execute_cmd, fetch_url)
    /// and skills in serve mode. Default: false (pure markdown notebook mode for lower token cost and security).
    #[serde(default)]
    pub agent_skills: bool,

    /// Enable "Lenient Legacy Client Mode" for the MCP Streamable HTTP endpoint:
    /// requests with NO MCP-Protocol-Version/Mcp-Method/Mcp-Name headers at all skip
    /// header-body consistency validation (body-only dispatch), to support standard MCP
    /// clients (Hermes, Claude Desktop, Cursor) that do not include draft HTTP headers.
    /// Default: true (enabled; standard MCP client compatibility).
    #[serde(default = "default_mcp_lenient_true")]
    pub mcp_lenient_legacy_clients: bool,

    /// Custom title for Rune Notes web UI / pages.
    pub title: Option<String>,
    /// Custom description for Rune Notes web UI / pages.
    #[serde(alias = "description")]
    pub desc: Option<String>,
    /// Optional monthly spending budget in USD (e.g. 50.0).
    #[serde(default)]
    pub monthly_budget: Option<f64>,
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            port: None,
            bind: None,
            model: None,
            github: None,
            local: None,
            oauth: Vec::new(),
            agent_skills: false,
            mcp_lenient_legacy_clients: true,
            title: None,
            desc: None,
            monthly_budget: None,
        }
    }
}

/// Local credentials configuration for Rune Notes.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LocalConfig {
    /// Local username:password credentials granted admin role.
    #[serde(default)]
    pub admins: Vec<String>,
    /// Local username:password credentials granted user role.
    #[serde(default)]
    pub users: Vec<String>,
    /// Local username:password credentials granted guest (read-only) role.
    #[serde(default)]
    pub guests: Vec<String>,
}

/// GitHub OAuth 2.0 configuration for Rune Notes.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GitHubOAuthConfig {
    /// GitHub OAuth App client ID.
    pub client_id: String,
    /// GitHub OAuth App client secret.
    pub client_secret: String,
    /// GitHub logins or `"org:org/team"` refs granted admin role.
    #[serde(default)]
    pub admins: Vec<String>,
    /// GitHub logins or `"org:org/team"` refs granted user role.
    #[serde(default)]
    pub users: Vec<String>,
    /// GitHub logins or `"org:org/team"` refs granted guest (read-only) role.
    #[serde(default)]
    pub guests: Vec<String>,
}

fn default_oauth_scopes() -> Vec<String> {
    vec!["openid".to_string(), "profile".to_string()]
}

fn default_oauth_groups_claim() -> String {
    "groups".to_string()
}

/// Generic third-party OAuth2/OIDC configuration for Rune Notes.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OAuthProviderConfig {
    /// Stable provider key used in URL path `/auth/oauth/{name}`.
    pub name: String,
    /// Human-readable provider name shown on login page.
    pub display_name: Option<String>,
    /// Optional emoji or short text prefix shown before the button label (e.g. "🐧").
    pub icon: Option<String>,
    /// OAuth app client id.
    pub client_id: String,
    /// OAuth app client secret.
    pub client_secret: String,
    /// OIDC issuer URL for discovery (`/.well-known/openid-configuration`).
    pub issuer: Option<String>,
    /// Explicit OAuth authorization endpoint (fallback when discovery is absent/fails).
    pub authorization_url: Option<String>,
    /// Explicit OAuth token endpoint (fallback when discovery is absent/fails).
    pub token_url: Option<String>,
    /// Explicit OAuth userinfo endpoint (fallback when discovery is absent/fails).
    pub userinfo_url: Option<String>,
    /// OAuth scopes sent to the provider authorize endpoint.
    #[serde(default = "default_oauth_scopes")]
    pub scopes: Vec<String>,
    /// Userinfo claim that contains group names (string or array).
    #[serde(default = "default_oauth_groups_claim")]
    pub groups_claim: String,
    /// Role mapping entries: plain values match user identity, `grp:<name>` matches group value.
    #[serde(default)]
    pub admins: Vec<String>,
    /// Role mapping entries: plain values match user identity, `grp:<name>` matches group value.
    #[serde(default)]
    pub users: Vec<String>,
    /// Role mapping entries: plain values match user identity, `grp:<name>` matches group value.
    #[serde(default)]
    pub guests: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentProfile {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
}

fn default_max_iterations() -> u32 {
    20
}
fn default_confidence_threshold() -> f64 {
    0.7
}
fn default_state_dir() -> String {
    "~/.rune/loops".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoopConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    #[serde(default = "default_state_dir")]
    pub state_dir: String,
    pub implementer_agent: Option<String>,
    pub verifier_agent: Option<String>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 20,
            confidence_threshold: 0.7,
            state_dir: "~/.rune/loops".to_string(),
            implementer_agent: None,
            verifier_agent: None,
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
    /// Enforce Zero Data Retention (ZDR) model filtering for OpenRouter.
    #[serde(default)]
    pub openrouter_zdr: bool,
    /// Do not auto-load AGENTS.md from the current directory.
    #[serde(default)]
    pub no_agents_md: bool,
    /// Approximate model context window in tokens.
    pub context_window: usize,
    /// Trigger automatic compaction once this fraction of context_window is reached.
    pub compact_threshold: f64,
    /// Absolute token threshold to trigger automatic compaction (None = disabled, uses compact_threshold fraction).
    #[serde(default)]
    pub compact_token_limit: Option<usize>,
    /// Keep the last N messages when compacting context.
    pub compact_keep_last: usize,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub mcp: Vec<crate::mcp::McpServerConfig>,
    #[serde(default)]
    pub embedding: crate::embedding::EmbeddingConfig,
    /// Thinking/reasoning level: "none", "low", "medium", "high". None = provider default.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Custom system prompt. When set, replaces the default hardcoded prompt.
    /// AGENTS.md is still appended if present.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional monthly spending budget/limit in USD (e.g. 50.0).
    #[serde(default)]
    pub monthly_budget: Option<f64>,
    /// Skills to preload at startup (comma-separated names from --skills flag).
    /// When set, only these skills are available; semantic/@ discovery is skipped.
    #[serde(skip)]
    pub preload_skills: Vec<String>,
    /// Notes mode configuration ([notes] section in rune.toml).
    #[serde(default)]
    pub notes: NotesConfig,
    #[serde(default)]
    pub agents: std::collections::HashMap<String, AgentProfile>,
    #[serde(rename = "loop", default)]
    pub loop_config: LoopConfig,
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
            skills_dir: "~/skills".to_string(),
            log_level: "error".to_string(),
            max_steps: Some(50),
            token_budget: None,
            timeout_secs: Some(30),
            base_url: None,
            trace: None,
            json_output: false,
            auto_approve: false,
            openrouter_zdr: false,
            no_agents_md: false,
            context_window: 128000,
            compact_threshold: 0.85,
            compact_token_limit: None,
            compact_keep_last: 6,
            policy: PolicyConfig::default(),
            mcp: Vec::new(),
            embedding: crate::embedding::EmbeddingConfig::default(),
            thinking: None,
            system_prompt: None,
            monthly_budget: None,
            preload_skills: Vec::new(),
            notes: NotesConfig::default(),
            agents: std::collections::HashMap::new(),
            loop_config: LoopConfig::default(),
            cli_prompt: None,
        }
    }
}

/// Partial config for layered merging.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct PartialConfig {
    pub(crate) model: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) skills_dir: Option<String>,
    pub(crate) log_level: Option<String>,
    pub(crate) max_steps: Option<u32>,
    pub(crate) token_budget: Option<u32>,
    pub(crate) timeout_secs: Option<u64>,
    pub(crate) base_url: Option<String>,
    pub(crate) trace: Option<String>,
    pub(crate) context_window: Option<usize>,
    pub(crate) compact_threshold: Option<f64>,
    pub(crate) compact_token_limit: Option<usize>,
    pub(crate) compact_keep_last: Option<usize>,
    pub(crate) policy: Option<PolicyConfig>,
    pub(crate) mcp: Option<Vec<crate::mcp::McpServerConfig>>,
    pub(crate) embedding: Option<crate::embedding::EmbeddingConfig>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) thinking: Option<String>,
    pub(crate) openrouter_zdr: Option<bool>,
    pub(crate) no_agents_md: Option<bool>,
    pub(crate) monthly_budget: Option<f64>,
    pub(crate) notes: Option<NotesConfig>,
    #[serde(default)]
    pub(crate) agents: Option<std::collections::HashMap<String, AgentProfile>>,
    #[serde(rename = "loop")]
    pub(crate) loop_config: Option<LoopConfig>,
}
