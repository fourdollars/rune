pub mod loader;
pub mod persist;
pub mod types;
pub mod util;

#[cfg(test)]
mod tests;

// Re-export core types and functions to maintain 100% backward compatibility
pub use loader::{apply_mount_flags, load, load_without_clap, load_without_clap_path};
pub use persist::{
    persist_command, persist_domain, persist_path_ro, persist_path_rw, persist_policy_array,
    persist_policy_array_at,
};
pub use types::{
    AgentProfile, GitHubOAuthConfig, LocalConfig, LoopConfig, NotesConfig, OAuthProviderConfig,
    PolicyConfig, RuneConfig,
};
pub use util::{data_dir, expand_tilde, safe_truncate};
