use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::Path;

/// Source configuration for the Rune resource type.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceSource {
    /// API key for LLM provider.
    pub api_key: Option<String>,
    /// LLM model name.
    pub model: Option<String>,
    /// Base URL for LLM provider.
    pub base_url: Option<String>,
    /// The prompt to execute for check/get (content detection).
    pub prompt: Option<String>,
    /// Pre-commands to run before AI loop.
    #[serde(default)]
    pub pre_commands: Vec<String>,
    /// Sandbox configuration.
    pub sandbox: Option<Value>,
}

/// Parameters for put step.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceParams {
    /// Prompt for the put step (AI agent execution).
    pub prompt: Option<String>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Append to default system prompt.
    pub append_system_prompt: Option<String>,
    /// Pre-commands to run before AI loop.
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

/// Compute sha256 of a string and return "sha256:<hex>" format.
fn sha256_ref(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

/// Call the LLM via curl (same approach as provider/mod.rs).
/// Returns the assistant's response text.
/// Refresh GitHub Copilot session token from a ghu_/ghp_ OAuth token.
/// Returns (session_token, endpoint_base_url).
fn refresh_copilot_token(oauth_token: &str) -> anyhow::Result<(String, String)> {
    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "-H",
            &format!("Authorization: token {}", oauth_token),
            "-H",
            "editor-version: vscode/1.96.0",
            "https://api.github.com/copilot_internal/v2/token",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("copilot token refresh failed: {}", stderr);
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("failed to parse copilot token response: {}", e))?;

    let token = v
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no token in copilot response: {}",
                &body[..body.len().min(200)]
            )
        })?
        .to_string();

    let endpoint = v
        .get("endpoints")
        .and_then(|e| e.get("api"))
        .and_then(|a| a.as_str())
        .unwrap_or("https://api.githubcopilot.com")
        .to_string();

    Ok((token, endpoint))
}

fn call_llm_sync(source: &ResourceSource, prompt: &str) -> anyhow::Result<String> {
    let raw_key = source
        .api_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("source.api_key is required"))?;

    // Detect GitHub Copilot tokens and refresh
    let (api_key, base_url) = if raw_key.starts_with("ghu_") || raw_key.starts_with("ghp_") {
        eprintln!("rune: detected GitHub Copilot token, refreshing...");
        let (session_token, endpoint) = refresh_copilot_token(raw_key)?;
        eprintln!("rune: copilot endpoint: {}", endpoint);
        (
            session_token,
            format!("{}/chat/completions", endpoint.trim_end_matches('/')),
        )
    } else {
        let base = source
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        (
            raw_key.to_string(),
            format!("{}/chat/completions", base.trim_end_matches('/')),
        )
    };

    let model = source.model.as_deref().unwrap_or("gpt-4o-mini");

    let url = base_url;

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "You are a helpful assistant. Respond concisely and accurately."},
            {"role": "user", "content": prompt}
        ]
    });

    // Write payload to temp file to avoid shell escaping issues
    let tmp_path = format!("/tmp/rune_concourse_{}.json", std::process::id());
    std::fs::write(&tmp_path, payload.to_string())?;

    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            &url,
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {}", api_key),
            "-H",
            "editor-version: vscode/1.96.0",
            "--max-time",
            "120",
            "-d",
            &format!("@{}", tmp_path),
        ])
        .output()?;

    // Clean up temp file
    let _ = std::fs::remove_file(&tmp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("curl failed: {}", stderr);
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let resp: Value = serde_json::from_str(&body).map_err(|e| {
        anyhow::anyhow!(
            "failed to parse LLM response: {} body={}",
            e,
            &body[..body.len().min(200)]
        )
    })?;

    // Extract assistant message content
    let content = resp
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            // Check for error response
            if let Some(err) = resp.get("error") {
                anyhow::anyhow!("LLM API error: {}", err)
            } else {
                anyhow::anyhow!(
                    "unexpected LLM response format: {}",
                    &body[..body.len().min(500)]
                )
            }
        })?;

    Ok(content.to_string())
}

/// Handle `check` mode:
/// - If source.prompt is set: call LLM, compute sha256 of response → version
/// - If no prompt: return synthetic version
/// - If previous version matches: return same (no change detected)
pub fn handle_check<R: Read>(reader: R) -> anyhow::Result<CheckResponse> {
    let s = read_to_string_from(reader)?;
    let req: CheckRequest =
        serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("invalid check JSON: {}", e))?;

    let prompt = match req.source.prompt.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => {
            // No prompt configured: return synthetic version
            return Ok(CheckResponse(vec![serde_json::json!({"ref": "latest"})]));
        }
    };

    // Execute the prompt
    eprintln!("rune check: executing prompt...");
    let response = call_llm_sync(&req.source, prompt)?;
    let new_ref = sha256_ref(&response);
    eprintln!("rune check: ref={}", &new_ref[..20]);

    let new_version = serde_json::json!({"ref": new_ref});

    if let Some(prev) = req.version {
        if prev == new_version {
            // No change
            Ok(CheckResponse(vec![prev]))
        } else {
            // New content detected
            Ok(CheckResponse(vec![prev, new_version]))
        }
    } else {
        // First check
        Ok(CheckResponse(vec![new_version]))
    }
}

/// Handle `in` (get) mode:
/// - Re-execute the prompt from source
/// - Write result to <dest_dir>/payload.json
/// - Return version and metadata
pub fn handle_in<R: Read>(reader: R, dest_dir: &str) -> anyhow::Result<InResponse> {
    let s = read_to_string_from(reader)?;
    let req: InRequest =
        serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("invalid in JSON: {}", e))?;

    let prompt = match req.source.prompt.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => {
            // No prompt: write empty payload
            let dest = Path::new(dest_dir);
            std::fs::create_dir_all(dest)?;
            std::fs::write(dest.join("payload.json"), "{}")?;

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

    // Execute the prompt
    eprintln!("rune in: executing prompt...");
    let response = call_llm_sync(&req.source, prompt)?;
    let content_ref = sha256_ref(&response);

    // Write payload to destination
    let dest = Path::new(dest_dir);
    std::fs::create_dir_all(dest)?;

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
    )?;

    // Also write raw response as response.txt for convenience
    std::fs::write(dest.join("response.txt"), &response)?;

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

/// Handle `out` (put) mode:
/// - Execute params.prompt via the AI agent
/// - Return version with sha256 of output
pub fn handle_out<R: Read>(reader: R) -> anyhow::Result<OutResponse> {
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

    // Execute the prompt
    eprintln!("rune out: executing prompt...");
    let response = call_llm_sync(&req.source, prompt)?;
    let content_ref = sha256_ref(&response);

    // Print the response to stderr (visible in Concourse build log)
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

/// Main entry point for Concourse mode. Reads stdin and writes JSON to stdout.
/// Logs go to stderr (Concourse convention).
pub fn run(mode: ConcourseMode) {
    match mode {
        ConcourseMode::Check => match handle_check(io::stdin()) {
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
            // Concourse passes destination dir as first CLI argument
            let dest_dir = std::env::args()
                .nth(1)
                .unwrap_or_else(|| "/tmp/rune-in".into());
            match handle_in(io::stdin(), &dest_dir) {
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
        ConcourseMode::Out => match handle_out(io::stdin()) {
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
        assert_eq!(r.len(), 7 + 64); // "sha256:" + 64 hex chars
                                     // Known value
        assert_eq!(
            r,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_ref_deterministic() {
        assert_eq!(sha256_ref("test"), sha256_ref("test"));
        assert_ne!(sha256_ref("a"), sha256_ref("b"));
    }

    #[test]
    fn test_check_no_prompt_returns_synthetic() {
        let input = json!({"source": {"api_key": "test"}}).to_string();
        let resp = handle_check(input.as_bytes()).expect("handle_check");
        assert_eq!(resp.0.len(), 1);
        assert_eq!(resp.0[0], json!({"ref": "latest"}));
    }

    #[test]
    fn test_check_with_version_no_prompt() {
        let input = json!({
            "source": {"api_key": "test"},
            "version": {"ref": "sha256:abc123"}
        })
        .to_string();
        let resp = handle_check(input.as_bytes()).expect("handle_check");
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

        let resp = handle_in(input.as_bytes(), dir.to_str().unwrap()).expect("handle_in");
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

        let result = handle_out(input.as_bytes());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("prompt is required"));
    }

    #[test]
    fn test_resource_source_deserialization() {
        let input = json!({
            "source": {
                "api_key": "sk-test",
                "model": "gpt-4o",
                "base_url": "https://custom.api/v1",
                "prompt": "Get latest news",
                "pre_commands": ["export FOO=bar"]
            }
        });

        let req: CheckRequest = serde_json::from_value(input).unwrap();
        assert_eq!(req.source.api_key.as_deref(), Some("sk-test"));
        assert_eq!(req.source.model.as_deref(), Some("gpt-4o"));
        assert_eq!(req.source.prompt.as_deref(), Some("Get latest news"));
        assert_eq!(req.source.pre_commands, vec!["export FOO=bar"]);
    }

    #[test]
    fn test_metadata_serialization() {
        let meta = vec![
            MetadataItem {
                name: "ref".into(),
                value: "sha256:abc".into(),
            },
            MetadataItem {
                name: "length".into(),
                value: "42".into(),
            },
        ];
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("sha256:abc"));
        assert!(json.contains("42"));
    }
}
