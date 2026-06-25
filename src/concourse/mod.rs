use crate::agent::{Agent, StopReason};
use crate::config::{PolicyConfig, RuneConfig};
use crate::provider::{CopilotProvider, OpenAiProvider, ProviderRegistry};
use crate::sandbox::{SandboxConfig, SandboxExecutor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Deserialize null or missing as empty Vec<String>.
fn null_as_empty_vec<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(d).map(|o| o.unwrap_or_default())
}

/// Source configuration for the Rune resource type.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceSource {
    /// API key for LLM provider.
    pub api_key: Option<String>,
    /// LLM model name.
    pub model: Option<String>,
    /// Base URL for LLM provider.
    pub base_url: Option<String>,
    /// Prompt to execute for check/get.
    pub prompt: Option<String>,
    /// Pre-commands to run before the AI loop.
    #[serde(default)]
    pub pre_commands: Vec<String>,
    /// Sandbox configuration (network/filesystem/syscalls/resources).
    pub sandbox: Option<Value>,
    /// Policy fields (can be at source level instead of nested in sandbox).
    #[serde(default)]
    pub policy: Option<SourcePolicySpec>,
}

/// Top-level policy spec for resource source.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SourcePolicySpec {
    /// Policy mode: "allowlist" (default for Concourse), "confirm", or "unrestricted"
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub allowed_syscalls: Vec<String>,
}

/// Parameters for put step.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceParams {
    /// Prompt for the put step (AI agent execution).
    pub prompt: Option<String>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Append to the default system prompt.
    pub append_system_prompt: Option<String>,
    /// Pre-commands to run before the AI loop.
    #[serde(default)]
    pub pre_commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CheckRequest {
    pub source: ResourceSource,
    pub version: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InRequest {
    pub source: ResourceSource,
    pub version: Option<Value>,
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct OutRequest {
    pub source: ResourceSource,
    pub params: Option<ResourceParams>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct CheckResponse(pub Vec<Value>);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InResponse {
    pub version: Value,
    pub metadata: Vec<MetadataItem>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct OutResponse {
    pub version: Value,
    pub metadata: Vec<MetadataItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetadataItem {
    pub name: String,
    pub value: String,
}

pub enum ConcourseMode {
    Check,
    In,
    Out,
}

fn read_to_string_from<R: Read>(mut reader: R) -> io::Result<String> {
    let mut s = String::new();
    reader.read_to_string(&mut s)?;
    Ok(s)
}

/// Compute sha256 of a string and return `sha256:<hex>` format.
fn sha256_ref(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SandboxNetworkSpec {
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    allowed_domains: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SandboxFilesystemSpec {
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    read_write_paths: Vec<String>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    read_only_paths: Vec<String>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    denied_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SandboxSyscallsSpec {
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    allow: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SandboxResourcesSpec {
    timeout_secs: Option<u64>,
    max_memory_mb: Option<u64>,
    max_pids: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SandboxSpec {
    #[serde(default)]
    network: SandboxNetworkSpec,
    #[serde(default)]
    filesystem: SandboxFilesystemSpec,
    #[serde(default)]
    syscalls: SandboxSyscallsSpec,
    #[serde(default)]
    resources: SandboxResourcesSpec,
    #[serde(default)]
    allowed_commands: Vec<String>,
}

fn extend_unique(dst: &mut Vec<String>, src: Vec<String>) {
    let mut seen: HashSet<String> = dst.iter().cloned().collect();
    for item in src {
        if seen.insert(item.clone()) {
            dst.push(item);
        }
    }
}

fn sandbox_spec(source: &ResourceSource) -> Option<SandboxSpec> {
    source
        .sandbox
        .as_ref()
        .and_then(|v| serde_json::from_value::<SandboxSpec>(v.clone()).ok())
}

/// Merge Concourse sandbox config into Rune policy config.
fn build_policy_from_source(source: &ResourceSource) -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    // Merge from source.policy (top-level)
    if let Some(ref p) = source.policy {
        extend_unique(&mut policy.allowed_commands, p.allowed_commands.clone());
        extend_unique(&mut policy.allowed_domains, p.allowed_domains.clone());
        extend_unique(&mut policy.allowed_syscalls, p.allowed_syscalls.clone());
    }
    if let Some(spec) = sandbox_spec(source) {
        extend_unique(&mut policy.allowed_domains, spec.network.allowed_domains);
        extend_unique(&mut policy.allowed_commands, spec.allowed_commands);
        extend_unique(
            &mut policy.allowed_paths_rw,
            spec.filesystem.read_write_paths,
        );
        extend_unique(
            &mut policy.allowed_paths_ro,
            spec.filesystem.read_only_paths,
        );
        extend_unique(&mut policy.denied_paths, spec.filesystem.denied_paths);
        extend_unique(&mut policy.allowed_syscalls, spec.syscalls.allow);
        if let Some(max_memory_mb) = spec.resources.max_memory_mb {
            policy.max_memory_mb = max_memory_mb;
        }
        if let Some(max_pids) = spec.resources.max_pids {
            policy.max_pids = max_pids;
        }
    }
    policy
}

fn build_sandbox_config(source: &ResourceSource) -> SandboxConfig {
    let policy_mode = source
        .policy
        .as_ref()
        .and_then(|p| p.mode.clone())
        .unwrap_or_default();

    // Unrestricted mode: no sandbox restrictions (timeout only)
    if policy_mode == "unrestricted" {
        return SandboxConfig {
            allowed_domains: vec!["*".to_string()],
            read_write_paths: vec![PathBuf::from("/")],
            read_only_paths: vec![],
            denied_paths: vec![],
            traverse_paths: Vec::new(),
            timeout_secs: sandbox_spec(source)
                .and_then(|s| s.resources.timeout_secs)
                .unwrap_or(30),
            uid: 0,
            gid: 0,
            memory_limit: 0,
            cpu_limit_secs: 0,
            max_pids: 0,
            allowed_syscalls: vec!["*".to_string()],
            tmp_size_mb: 0,
        };
    }

    let policy = build_policy_from_source(source);
    let timeout_secs = sandbox_spec(source)
        .and_then(|s| s.resources.timeout_secs)
        .unwrap_or(30);

    SandboxConfig {
        traverse_paths: Vec::new(),
        allowed_domains: policy.allowed_domains,
        read_write_paths: policy.allowed_paths_rw.iter().map(PathBuf::from).collect(),
        read_only_paths: policy.allowed_paths_ro.iter().map(PathBuf::from).collect(),
        denied_paths: policy.denied_paths.iter().map(PathBuf::from).collect(),
        timeout_secs,
        allowed_syscalls: policy.allowed_syscalls.clone(),
        uid: 0,
        gid: 0,
        memory_limit: policy.max_memory_mb.saturating_mul(1024 * 1024),
        cpu_limit_secs: 0,
        max_pids: policy.max_pids,
        tmp_size_mb: policy.max_tmp_mb,
    }
}

fn build_runtime_config(source: &ResourceSource) -> RuneConfig {
    let mut cfg = RuneConfig::default();
    cfg.model = source.model.clone().unwrap_or(cfg.model);
    cfg.api_key = source.api_key.clone();
    cfg.base_url = source.base_url.clone();
    cfg.policy = build_policy_from_source(source);
    // Concourse defaults to allowlist mode
    let policy_mode = source.policy.as_ref().and_then(|p| p.mode.clone());
    if let Some(mode) = policy_mode {
        cfg.policy.mode = mode;
    } else if cfg.policy.mode == "confirm" {
        cfg.policy.mode = "allowlist".to_string();
    }
    cfg.json_output = false;
    cfg.trace = None;
    cfg.auto_approve = true;
    cfg
}

fn build_provider(cfg: &RuneConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    if let Some(ref key) = cfg.api_key {
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
            } else {
                "openai"
            }
        });

        match provider_name {
            "github-copilot" | "copilot" => {
                registry.register(Box::new(CopilotProvider::new(key.clone())));
            }
            "gemini" | "google" => {
                registry.register(Box::new(crate::provider::GeminiProvider::new(
                    key.clone(),
                    Some(cfg.model.clone()),
                    cfg.base_url.clone(),
                )));
            }
            _ => {
                registry.register(Box::new(OpenAiProvider::new(
                    provider_name.to_string(),
                    key.clone(),
                    cfg.base_url.clone(),
                    cfg.openrouter_zdr,
                )));
            }
        }
    }

    registry
}

fn default_system_prompt() -> String {
    "You are Rune, a high-performance AI agent running inside a Concourse CI resource type. Use tools when needed. Be concise and accurate."
        .to_string()
}

async fn execute_sandboxed_pre_commands(
    source: &ResourceSource,
    commands: &[String],
) -> anyhow::Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let executor = SandboxExecutor::new(build_sandbox_config(source));
    for cmd in commands {
        eprintln!("rune: pre-command (sandboxed): {}", cmd);
        let result = executor.run_shell_command(cmd, None, None).await?;
        eprintln!(
            "rune: sandbox layers: {:?}, degraded: {}",
            result.active_layers, result.degraded
        );
        if !result.stdout.trim().is_empty() {
            eprintln!("rune: pre-cmd stdout: {}", result.stdout.trim());
        }
        if !result.stderr.trim().is_empty() {
            eprintln!("rune: pre-cmd stderr: {}", result.stderr.trim());
        }
        if result.exit_code != 0 {
            anyhow::bail!(
                "pre-command failed: '{}' exit code {}\nstdout: {}\nstderr: {}",
                cmd,
                result.exit_code,
                result.stdout,
                result.stderr
            );
        }
    }

    Ok(())
}

async fn run_agent_prompt(
    source: &ResourceSource,
    prompt: &str,
    system_prompt: Option<&str>,
    append_system_prompt: Option<&str>,
    pre_commands: &[String],
) -> anyhow::Result<String> {
    execute_sandboxed_pre_commands(source, pre_commands).await?;

    let cfg = build_runtime_config(source);
    let provider = build_provider(&cfg);
    if provider.is_empty() {
        anyhow::bail!("source.api_key is required");
    }

    let mut agent = Agent::new(cfg, provider, false, None);

    let mut sys = system_prompt
        .map(|s| s.to_string())
        .unwrap_or_else(default_system_prompt);
    if let Some(append) = append_system_prompt {
        if !append.trim().is_empty() {
            sys.push('\n');
            sys.push_str(append);
        }
    }
    agent.set_system_prompt(&sys);

    match agent.run(prompt).await {
        StopReason::FinalAnswer(answer) => Ok(answer),
        StopReason::Error(e) => anyhow::bail!("{}", e),
        other => anyhow::bail!("agent stopped before final answer: {:?}", other),
    }
}

/// Handle `check` mode.
///
/// When `source.prompt` is provided, the prompt is executed through the full
/// Rune agent pipeline (sandbox + tools + provider). The returned final answer
/// is hashed and used as the resource version.
pub async fn handle_check<R: Read>(reader: R) -> anyhow::Result<CheckResponse> {
    let s = read_to_string_from(reader)?;
    let req: CheckRequest =
        serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("invalid check JSON: {}", e))?;

    let prompt = match req.source.prompt.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Ok(CheckResponse(vec![serde_json::json!({"ref": "latest"})])),
    };

    eprintln!("rune check: sandboxed agent execution...");
    eprintln!("rune check: prompt={}", prompt);
    let response =
        run_agent_prompt(&req.source, prompt, None, None, &req.source.pre_commands).await?;
    let new_ref = sha256_ref(&response);
    eprintln!("rune check: ref={}", &new_ref[..20]);

    let new_version = serde_json::json!({"ref": new_ref});
    if let Some(prev) = req.version {
        if prev == new_version {
            Ok(CheckResponse(vec![prev]))
        } else {
            Ok(CheckResponse(vec![prev, new_version]))
        }
    } else {
        Ok(CheckResponse(vec![new_version]))
    }
}

/// Handle `in` (get) mode.
///
/// Re-executes the prompt through the full Rune agent pipeline and writes the
/// result to `<dest_dir>/payload.json` and `<dest_dir>/response.txt`.
pub async fn handle_in<R: Read>(reader: R, dest_dir: &str) -> anyhow::Result<InResponse> {
    let s = read_to_string_from(reader)?;
    let req: InRequest =
        serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("invalid in JSON: {}", e))?;

    let prompt = match req.source.prompt.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            let dest = Path::new(dest_dir);
            std::fs::create_dir_all(dest).map_err(|e| {
                anyhow::anyhow!("failed to create dest dir '{}': {}", dest.display(), e)
            })?;
            std::fs::write(dest.join("payload.json"), "{}")
                .map_err(|e| anyhow::anyhow!("failed to write payload.json: {}", e))?;

            let version = req
                .version
                .unwrap_or_else(|| serde_json::json!({"ref": "latest"}));

            return Ok(InResponse {
                version,
                metadata: vec![MetadataItem {
                    name: "status".into(),
                    value: "no prompt configured".into(),
                }],
            });
        }
    };

    eprintln!("rune in: sandboxed agent execution...");
    eprintln!("rune in: prompt={}", prompt);
    let response =
        run_agent_prompt(&req.source, prompt, None, None, &req.source.pre_commands).await?;
    let content_ref = sha256_ref(&response);

    let dest = Path::new(dest_dir);
    std::fs::create_dir_all(dest)
        .map_err(|e| anyhow::anyhow!("failed to create dest dir '{}': {}", dest.display(), e))?;

    let payload = serde_json::json!({
        "prompt": prompt,
        "response": response,
        "ref": content_ref,
        "model": req.source.model.as_deref().unwrap_or("gpt-4o-mini"),
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    std::fs::write(
        dest.join("payload.json"),
        serde_json::to_string_pretty(&payload)?,
    )
    .map_err(|e| anyhow::anyhow!("failed to write payload.json: {}", e))?;
    std::fs::write(dest.join("response.txt"), &response)
        .map_err(|e| anyhow::anyhow!("failed to write response.txt: {}", e))?;

    let version = req
        .version
        .unwrap_or_else(|| serde_json::json!({"ref": content_ref}));

    let metadata = vec![
        MetadataItem {
            name: "ref".into(),
            value: content_ref[..20].to_string(),
        },
        MetadataItem {
            name: "response_length".into(),
            value: response.len().to_string(),
        },
        MetadataItem {
            name: "model".into(),
            value: req.source.model.unwrap_or_else(|| "gpt-4o-mini".into()),
        },
    ];

    Ok(InResponse { version, metadata })
}

/// Handle `out` (put) mode.
///
/// Executes `params.prompt` through the full Rune agent pipeline and returns a
/// sha256-based version derived from the final answer.
pub async fn handle_out<R: Read>(reader: R) -> anyhow::Result<OutResponse> {
    let s = read_to_string_from(reader)?;
    let req: OutRequest =
        serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("invalid out JSON: {}", e))?;

    let params = req.params.unwrap_or(ResourceParams {
        prompt: None,
        system_prompt: None,
        append_system_prompt: None,
        pre_commands: Vec::new(),
    });

    let prompt = params
        .prompt
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("params.prompt is required for put step"))?;

    eprintln!("rune out: sandboxed agent execution...");
    eprintln!("rune out: prompt={}", prompt);
    let response = run_agent_prompt(
        &req.source,
        prompt,
        params.system_prompt.as_deref(),
        params.append_system_prompt.as_deref(),
        &params.pre_commands,
    )
    .await?;
    let content_ref = sha256_ref(&response);

    eprintln!("--- AI Response ---");
    eprintln!("{}", response);
    eprintln!("--- End Response ---");

    let version = serde_json::json!({"ref": content_ref});
    let metadata = vec![
        MetadataItem {
            name: "ref".into(),
            value: content_ref[..20].to_string(),
        },
        MetadataItem {
            name: "response_length".into(),
            value: response.len().to_string(),
        },
        MetadataItem {
            name: "prompt".into(),
            value: if prompt.len() > 80 {
                format!("{}...", &prompt[..77])
            } else {
                prompt.to_string()
            },
        },
    ];

    Ok(OutResponse { version, metadata })
}

/// Main entry point for Concourse mode.
/// Reads stdin and writes JSON to stdout. Logs go to stderr.
pub async fn run(mode: ConcourseMode) {
    match mode {
        ConcourseMode::Check => match handle_check(io::stdin()).await {
            Ok(resp) => match serde_json::to_string(&resp.0) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("Failed to serialize CheckResponse: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error running check: {}", e);
                std::process::exit(1);
            }
        },
        ConcourseMode::In => {
            let dest_dir = std::env::args()
                .nth(1)
                .unwrap_or_else(|| "/tmp/rune-in".into());
            match handle_in(io::stdin(), &dest_dir).await {
                Ok(resp) => match serde_json::to_string(&resp) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("Failed to serialize InResponse: {}", e);
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("Error running in: {}", e);
                    std::process::exit(1);
                }
            }
        }
        ConcourseMode::Out => match handle_out(io::stdin()).await {
            Ok(resp) => match serde_json::to_string(&resp) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("Failed to serialize OutResponse: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error running out: {}", e);
                std::process::exit(1);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sha256_ref() {
        let r = sha256_ref("hello world");
        assert!(r.starts_with("sha256:"));
        assert_eq!(r.len(), 7 + 64);
        assert_eq!(
            r,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_check_no_prompt_returns_synthetic() {
        let input = json!({"source": {"api_key": "test"}}).to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt
            .block_on(handle_check(input.as_bytes()))
            .expect("handle_check");
        assert_eq!(resp.0.len(), 1);
        assert_eq!(resp.0[0], json!({"ref": "latest"}));
    }

    #[test]
    fn test_in_no_prompt_writes_empty_payload() {
        let dir = std::env::temp_dir().join(format!("rune-in-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let input = json!({
            "source": {"api_key": "test"},
            "version": {"ref": "latest"}
        })
        .to_string();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt
            .block_on(handle_in(input.as_bytes(), dir.to_str().unwrap()))
            .expect("handle_in");
        assert_eq!(resp.version, json!({"ref": "latest"}));

        let payload_path = dir.join("payload.json");
        assert!(payload_path.exists());
        let content = std::fs::read_to_string(&payload_path).unwrap();
        assert_eq!(content, "{}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_out_requires_prompt() {
        let input = json!({
            "source": {"api_key": "test"},
            "params": {}
        })
        .to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(handle_out(input.as_bytes()));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("prompt is required"));
    }

    #[test]
    fn test_sandbox_spec_parsing() {
        let source = ResourceSource {
            api_key: Some("x".into()),
            model: Some("m".into()),
            base_url: None,
            prompt: None,
            pre_commands: vec![],
            policy: None,
            sandbox: Some(json!({
                "network": {"allowed_domains": ["github.com"]},
                "filesystem": {
                    "read_write_paths": ["/workspace"],
                    "read_only_paths": ["/usr"],
                    "denied_paths": ["/root"]
                },
                "syscalls": {"allow": ["ptrace"]},
                "resources": {"timeout_secs": 10, "max_memory_mb": 256, "max_pids": 32}
            })),
        };

        let policy = build_policy_from_source(&source);
        assert!(policy.allowed_domains.contains(&"github.com".to_string()));
        assert!(policy.allowed_paths_rw.contains(&"/workspace".to_string()));
        assert!(policy.allowed_paths_ro.contains(&"/usr".to_string()));
        assert!(policy.denied_paths.contains(&"/root".to_string()));
        assert!(policy.allowed_syscalls.contains(&"ptrace".to_string()));
        assert_eq!(policy.max_memory_mb, 256);
        assert_eq!(policy.max_pids, 32);

        let sb = build_sandbox_config(&source);
        assert_eq!(sb.timeout_secs, 10);
        assert_eq!(sb.memory_limit, 256 * 1024 * 1024);
        assert_eq!(sb.max_pids, 32);

        // Default runtime config should use allowlist mode (not confirm)
        let cfg = build_runtime_config(&source);
        assert_eq!(cfg.policy.mode, "allowlist");
    }

    #[test]
    fn test_policy_mode_default_allowlist() {
        let source = ResourceSource {
            api_key: Some("x".into()),
            model: None,
            base_url: None,
            prompt: None,
            pre_commands: vec![],
            policy: None,
            sandbox: None,
        };
        let cfg = build_runtime_config(&source);
        assert_eq!(
            cfg.policy.mode, "allowlist",
            "Concourse should default to allowlist"
        );
    }

    #[test]
    fn test_policy_mode_unrestricted_from_source_policy() {
        let source = ResourceSource {
            api_key: Some("x".into()),
            model: None,
            base_url: None,
            prompt: None,
            pre_commands: vec![],
            policy: Some(SourcePolicySpec {
                mode: Some("unrestricted".into()),
                ..Default::default()
            }),
            sandbox: None,
        };
        let cfg = build_runtime_config(&source);
        assert_eq!(
            cfg.policy.mode, "unrestricted",
            "source.policy.mode should set unrestricted"
        );
    }

    #[test]
    fn test_policy_mode_confirm_from_source_policy() {
        let source = ResourceSource {
            api_key: Some("x".into()),
            model: None,
            base_url: None,
            prompt: None,
            pre_commands: vec![],
            policy: Some(SourcePolicySpec {
                mode: Some("confirm".into()),
                ..Default::default()
            }),
            sandbox: None,
        };
        let cfg = build_runtime_config(&source);
        assert_eq!(
            cfg.policy.mode, "confirm",
            "source.policy.mode should set confirm"
        );
    }

    #[test]
    fn test_null_as_empty_vec_deserialize_null() {
        #[derive(Deserialize)]
        struct Test {
            #[serde(default, deserialize_with = "super::null_as_empty_vec")]
            items: Vec<String>,
        }
        let json = r#"{"items": null}"#;
        let t: Test = serde_json::from_str(json).unwrap();
        assert!(t.items.is_empty());
    }

    #[test]
    fn test_null_as_empty_vec_deserialize_missing() {
        #[derive(Deserialize)]
        struct Test {
            #[serde(default, deserialize_with = "super::null_as_empty_vec")]
            items: Vec<String>,
        }
        let json = r#"{}"#;
        let t: Test = serde_json::from_str(json).unwrap();
        assert!(t.items.is_empty());
    }

    #[test]
    fn test_null_as_empty_vec_deserialize_values() {
        #[derive(Deserialize)]
        struct Test {
            #[serde(default, deserialize_with = "super::null_as_empty_vec")]
            items: Vec<String>,
        }
        let json = r#"{"items": ["a", "b"]}"#;
        let t: Test = serde_json::from_str(json).unwrap();
        assert_eq!(t.items, vec!["a", "b"]);
    }

    #[test]
    fn test_policy_mode_from_source_policy() {
        let source = ResourceSource {
            api_key: Some("x".into()),
            model: None,
            base_url: None,
            prompt: None,
            pre_commands: vec![],
            policy: Some(SourcePolicySpec {
                mode: Some("unrestricted".into()),
                ..Default::default()
            }),
            sandbox: None,
        };
        let cfg = build_runtime_config(&source);
        assert_eq!(
            cfg.policy.mode, "unrestricted",
            "source.policy.mode should set the policy mode"
        );

        let sb = build_sandbox_config(&source);
        // Unrestricted mode should give wildcard domains
        assert!(sb.allowed_domains.contains(&"*".to_string()));
    }

    #[test]
    fn test_sandbox_spec_with_null_domains() {
        // Simulate Concourse YAML where allowed_domains is null
        let json = r#"{
            "network": {"allowed_domains": null},
            "filesystem": {},
            "syscalls": {},
            "resources": {}
        }"#;
        let spec: super::SandboxSpec = serde_json::from_str(json).unwrap();
        assert!(spec.network.allowed_domains.is_empty());
    }
}
