use serde_json::Value;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoopState {
    pub loop_id: String,
    pub goal: String,
    pub status: String,
    pub current_iteration: u32,
    pub max_iterations: u32,
    pub worktree_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct IterationRecord {
    pub iteration: u32,
    pub input_summary: String,
    pub tool_calls: Vec<String>,
    pub output_summary: String,
    pub tokens_used: Option<u32>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

/// Safely formats a SystemTime to RFC3339 UTC string without external crate dependencies.
pub fn format_rfc3339(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, min, sec) = secs_to_datetime(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

/// Generates the current timestamp in RFC3339 format.
pub fn now_rfc3339() -> String {
    format_rfc3339(std::time::SystemTime::now())
}

fn secs_to_datetime(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    const SECS_PER_DAY: u64 = 86400;
    let days = secs / SECS_PER_DAY;
    let rem_secs = secs % SECS_PER_DAY;

    let hour = (rem_secs / 3600) as u32;
    let min = ((rem_secs % 3600) / 60) as u32;
    let sec = (rem_secs % 60) as u32;

    let mut year = 1970;
    let mut days_left = days;

    loop {
        let leap = is_leap_year(year);
        let days_in_year = if leap { 366 } else { 365 };
        if days_left < days_in_year {
            break;
        }
        days_left -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    let mut day = 1;
    for (m, &days_in_month) in month_days.iter().enumerate() {
        if days_left < days_in_month {
            day = (days_left + 1) as u32;
            month = (m + 1) as u32;
            break;
        }
        days_left -= days_in_month;
    }

    (year, month, day, hour, min, sec)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub fn save_state(state: &LoopState, dir: &str) -> std::io::Result<()> {
    let path = std::path::Path::new(dir).join("state.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(path, content)
}

pub fn load_state(dir: &str) -> std::io::Result<LoopState> {
    let path = std::path::Path::new(dir).join("state.json");
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn log_audit(dir: &str, role: &str, action: &str, details: Value) -> std::io::Result<()> {
    let path = std::path::Path::new(dir).join("audit.jsonl");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let record = serde_json::json!({
        "timestamp": now_rfc3339(),
        "role": role,
        "action": action,
        "details": details
    });
    use std::io::Write;
    writeln!(file, "{}", record.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_and_load_state() {
        let dir = tempdir().unwrap();
        let dir_str = dir.path().to_str().unwrap();

        let state = LoopState {
            loop_id: "test-loop-123".to_string(),
            goal: "Test goal".to_string(),
            status: "running".to_string(),
            current_iteration: 1,
            max_iterations: 10,
            worktree_path: Some("/path/to/worktree".to_string()),
            created_at: "2026-06-19T12:00:00Z".to_string(),
            updated_at: "2026-06-19T12:05:00Z".to_string(),
        };

        save_state(&state, dir_str).unwrap();

        let state_path = dir.path().join("state.json");
        assert!(state_path.exists());

        let loaded_state = load_state(dir_str).unwrap();
        assert_eq!(loaded_state.loop_id, "test-loop-123");
        assert_eq!(loaded_state.goal, "Test goal");
        assert_eq!(loaded_state.status, "running");
        assert_eq!(loaded_state.current_iteration, 1);
        assert_eq!(loaded_state.max_iterations, 10);
        assert_eq!(
            loaded_state.worktree_path,
            Some("/path/to/worktree".to_string())
        );
        assert_eq!(loaded_state.created_at, "2026-06-19T12:00:00Z");
        assert_eq!(loaded_state.updated_at, "2026-06-19T12:05:00Z");
    }

    #[test]
    fn test_log_audit() {
        let dir = tempdir().unwrap();
        let dir_str = dir.path().to_str().unwrap();

        let details = serde_json::json!({
            "step": 1,
            "cmd": "cargo test"
        });

        log_audit(dir_str, "agent", "execute_command", details.clone()).unwrap();
        log_audit(
            dir_str,
            "system",
            "iteration_start",
            serde_json::Value::Null,
        )
        .unwrap();

        let audit_path = dir.path().join("audit.jsonl");
        assert!(audit_path.exists());

        let content = std::fs::read_to_string(audit_path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);

        let record1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record1["role"], "agent");
        assert_eq!(record1["action"], "execute_command");
        assert_eq!(record1["details"], details);
        assert!(record1["timestamp"].is_string());

        let record2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(record2["role"], "system");
        assert_eq!(record2["action"], "iteration_start");
        assert!(record2["details"].is_null());
        assert!(record2["timestamp"].is_string());
    }

    #[test]
    fn test_iteration_record_serialization() {
        let record = IterationRecord {
            iteration: 1,
            input_summary: "User asked to fix a bug".to_string(),
            tool_calls: vec!["view_file".to_string()],
            output_summary: "Found character boundary panic".to_string(),
            tokens_used: Some(1024),
            duration_ms: Some(1500),
            error: None,
        };
        let serialized = serde_json::to_string(&record).unwrap();
        let deserialized: IterationRecord = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, record);
    }

    #[test]
    fn test_format_rfc3339() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1771330925); // 2026-02-17T12:22:05Z
        assert_eq!(format_rfc3339(t), "2026-02-17T12:22:05Z");
    }
}
