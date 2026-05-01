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
    LlmRequest { messages_count: usize, model: String },
    LlmResponse { tokens_used: u32, has_tool_calls: bool },
    ToolCall { name: String, arguments_preview: String },
    ToolResult { name: String, is_error: bool, content_preview: String },
    PreCommand { command: String, exit_code: i32, duration_ms: u64 },
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

        Self { trace, output_dir, enabled }
    }

    /// 記錄一個步驟
    pub fn record(&mut self, kind: StepKind) {
        let step_num = (self.trace.steps.len() as u32) + 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let step = TraceStep { step_num, timestamp, kind };
        self.trace.steps.push(step);
    }

    /// 完成 trace 並寫入檔案
    pub fn finish(&mut self, exit_code: i32) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.trace.ended_at = Some(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0));
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
        w.record(StepKind::PreCommand { command: "echo hi".to_string(), exit_code: 0, duration_ms: 10 });
        w.record(StepKind::ToolCall { name: "t1".to_string(), arguments_preview: "{\"a\":1}".to_string() });
        w.finish(2).expect("finish should succeed");

        let path = output_dir.join(format!("{}.json", run_id));
        assert!(path.exists(), "trace file should exist");

        let data = fs::read_to_string(&path).expect("read trace");
        let v: serde_json::Value = serde_json::from_str(&data).expect("parse json");
        assert_eq!(v.get("run_id").and_then(|x| x.as_str()), Some("test-run-12345"));
        assert_eq!(v.get("model").and_then(|x| x.as_str()), Some("m1"));
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(2));
        assert!(v.get("steps").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0) >= 2);

        // cleanup
        let _ = fs::remove_dir_all(&dir);
    }
}
