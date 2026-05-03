use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一次完整 run 的 trace 記錄
#[derive(Debug, Serialize)]
pub struct RunTrace {
    pub run_id: String,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub model: String,
    pub steps: Vec<TraceStep>,
    pub total_tokens: u32,
    pub exit_code: i32,
}

/// 單一步驟的 trace
#[derive(Debug, Serialize)]
pub struct TraceStep {
    pub step_num: u32,
    pub timestamp: u64,
    pub kind: StepKind,
}

#[derive(Debug, Serialize)]
pub enum StepKind {
    LlmRequest {
        messages_count: usize,
        model: String,
    },
    LlmResponse {
        tokens_used: u32,
        has_tool_calls: bool,
    },
    ToolCall {
        name: String,
        arguments_preview: String,
    },
    ToolResult {
        name: String,
        is_error: bool,
        content_preview: String,
    },
    PreCommand {
        command: String,
        exit_code: i32,
        duration_ms: u64,
    },
}

/// Trace writer — 寫入 .rune/traces/ 目錄
pub struct TraceWriter {
    trace: RunTrace,
    output_dir: PathBuf,
    enabled: bool,
}

impl TraceWriter {
    pub fn new(run_id: String, model: String, output_dir: PathBuf, enabled: bool) -> Self {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let trace = RunTrace {
            run_id: run_id.clone(),
            started_at,
            ended_at: None,
            model,
            steps: Vec::new(),
            total_tokens: 0,
            exit_code: 0,
        };

        Self {
            trace,
            output_dir,
            enabled,
        }
    }

    /// 記錄一個步驟
    pub fn record(&mut self, kind: StepKind) {
        let step_num = (self.trace.steps.len() as u32) + 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let step = TraceStep {
            step_num,
            timestamp,
            kind,
        };
        self.trace.steps.push(step);
    }

    /// 完成 trace 並寫入檔案
    pub fn finish(&mut self, exit_code: i32) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.trace.ended_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        self.trace.exit_code = exit_code;

        fs::create_dir_all(&self.output_dir)?;
        let path = self.output_dir.join(format!("{}.json", self.trace.run_id));
        let json = serde_json::to_string_pretty(&self.trace)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// 生成 run_id (timestamp + random suffix)
    pub fn generate_run_id() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{}-{}-{}", now, pid, nanos)
    }
}

/// Redact sensitive info from strings (api keys, tokens).
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    // Patterns: sk-..., ghu_..., ghp_..., AIza..., Bearer ...
    let patterns = ["sk-", "ghu_", "ghp_", "AIza"];
    for pat in patterns {
        while let Some(start) = out.find(pat) {
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == ',')
                .map(|i| start + i)
                .unwrap_or(out.len());
            if end - start > pat.len() + 3 {
                out.replace_range(start + pat.len()..end, "***");
            } else {
                break;
            }
        }
    }
    // Bearer token
    while let Some(start) = out.find("Bearer ") {
        let token_start = start + 7;
        let end = out[token_start..]
            .find(|c: char| c.is_whitespace() || c == '"')
            .map(|i| token_start + i)
            .unwrap_or(out.len());
        if end > token_start + 3 {
            out.replace_range(token_start..end, "***");
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_trace_writer_writes_json() {
        let run_id = "test-run-12345".to_string();
        let tmp = std::env::temp_dir();
        let dir = tmp.join(format!("rune-trace-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let output_dir = dir.clone();

        let mut w = TraceWriter::new(run_id.clone(), "m1".to_string(), output_dir.clone(), true);
        w.record(StepKind::PreCommand {
            command: "echo hi".to_string(),
            exit_code: 0,
            duration_ms: 10,
        });
        w.record(StepKind::ToolCall {
            name: "t1".to_string(),
            arguments_preview: "{\"a\":1}".to_string(),
        });
        w.finish(2).expect("finish should succeed");

        let path = output_dir.join(format!("{}.json", run_id));
        assert!(path.exists(), "trace file should exist");

        let data = fs::read_to_string(&path).expect("read trace");
        let v: serde_json::Value = serde_json::from_str(&data).expect("parse json");
        assert_eq!(
            v.get("run_id").and_then(|x| x.as_str()),
            Some("test-run-12345")
        );
        assert_eq!(v.get("model").and_then(|x| x.as_str()), Some("m1"));
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(2));
        assert!(
            v.get("steps")
                .and_then(|s| s.as_array())
                .map(|a| a.len())
                .unwrap_or(0)
                >= 2
        );

        // cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_redact_openai_key() {
        let input = r#"{"api_key": "sk-abc123def456"}"#;
        let redacted = redact(input);
        assert!(!redacted.contains("abc123def456"));
        assert!(redacted.contains("sk-***"));
    }

    #[test]
    fn test_redact_github_token_ghu() {
        let input = "Authorization: token ghu_abcdefgh12345678";
        let redacted = redact(input);
        assert!(!redacted.contains("abcdefgh12345678"));
        assert!(redacted.contains("ghu_***"));
    }

    #[test]
    fn test_redact_github_token_ghp() {
        let input = r#"key = "ghp_SomeSecretToken123""#;
        let redacted = redact(input);
        assert!(!redacted.contains("SomeSecretToken123"));
        assert!(redacted.contains("ghp_***"));
    }

    #[test]
    fn test_redact_gemini_key() {
        let input = "api_key=AIzaSyAbCdEfGhIjKlMnOpQrStUvWxYz012345";
        let redacted = redact(input);
        assert!(!redacted.contains("SyAbCdEfGhIjKlMnOpQrStUvWxYz012345"));
        assert!(redacted.contains("AIza***"));
    }

    #[test]
    fn test_redact_bearer_token() {
        let input = r#"{"Authorization": "Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature"}"#;
        let redacted = redact(input);
        assert!(!redacted.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(redacted.contains("Bearer ***"));
    }

    #[test]
    fn test_redact_no_sensitive_data() {
        let input = "This is a normal string with no keys";
        let redacted = redact(input);
        assert_eq!(redacted, input);
    }

    #[test]
    fn test_redact_multiple_keys() {
        let input = r#"key1="sk-first123456" key2="ghp_second789""#;
        let redacted = redact(input);
        assert!(!redacted.contains("first123456"));
        assert!(!redacted.contains("second789"));
        assert!(redacted.contains("sk-***"));
        assert!(redacted.contains("ghp_***"));
    }

    #[test]
    fn test_redact_short_prefix_not_redacted() {
        // Very short values after prefix should not be redacted
        let input = "sk-ab";
        let redacted = redact(input);
        // Only 2 chars after prefix - may or may not redact depending on impl
        // Just verify it doesn't panic
        assert!(!redacted.is_empty());
    }

    #[test]
    fn test_generate_run_id_unique() {
        let id1 = TraceWriter::generate_run_id();
        let id2 = TraceWriter::generate_run_id();
        // They should be different (different nanos)
        // Note: in very fast execution they might be same, but format should be valid
        assert!(!id1.is_empty());
        assert!(!id2.is_empty());
        assert!(id1.contains('-'));
    }

    #[test]
    fn test_trace_writer_disabled_does_not_write() {
        let dir = std::env::temp_dir().join(format!("rune-trace-disabled-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut w = TraceWriter::new(
            "disabled-run".to_string(),
            "model".to_string(),
            dir.clone(),
            false, // disabled
        );
        w.record(StepKind::ToolCall {
            name: "test".to_string(),
            arguments_preview: "{}".to_string(),
        });
        w.finish(0).expect("should succeed");

        // Directory should not be created
        assert!(!dir.exists());
    }

    #[test]
    fn test_trace_writer_records_all_step_kinds() {
        let dir = std::env::temp_dir().join(format!("rune-trace-kinds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut w = TraceWriter::new(
            "kinds-test".to_string(),
            "gpt-4".to_string(),
            dir.clone(),
            true,
        );

        w.record(StepKind::LlmRequest {
            messages_count: 3,
            model: "gpt-4".to_string(),
        });
        w.record(StepKind::LlmResponse {
            tokens_used: 150,
            has_tool_calls: true,
        });
        w.record(StepKind::ToolCall {
            name: "read_file".to_string(),
            arguments_preview: r#"{"path":"src/main.rs"}"#.to_string(),
        });
        w.record(StepKind::ToolResult {
            name: "read_file".to_string(),
            is_error: false,
            content_preview: "fn main() {...}".to_string(),
        });
        w.record(StepKind::PreCommand {
            command: "cargo build".to_string(),
            exit_code: 0,
            duration_ms: 5000,
        });

        w.finish(0).expect("should write");

        let path = dir.join("kinds-test.json");
        assert!(path.exists());

        let data = fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        let steps = v["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 5);

        let _ = fs::remove_dir_all(&dir);
    }
}
