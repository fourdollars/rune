//! Scheduled cron jobs worker, interval parsing, and disk-based log management for Rune Notes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::StopReason;
use crate::serve::api::{
    broadcast_file_list, broadcast_to_room, build_effective_note_system_prompt, SseMsg,
};
use crate::serve::db::NoteCronJobRecord;
use crate::serve::ServerState;

/// Single execution log entry stored in `~/.rune/notes/{note}/cron/{job_id}.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteCronJobLogEntry {
    pub timestamp: String,
    pub trigger: String, // "schedule" | "manual"
    pub status: String,  // "success" | "silent_ok" | "error" | "timeout"
    pub duration_ms: u64,
    pub files_modified: Vec<String>,
    pub output_snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

/// Returns the path to the cron directory for a note: `<base_dir>/notes/<note_id>/cron`.
pub fn note_cron_dir(base_dir: &Path, note_id: &str) -> PathBuf {
    base_dir.join("notes").join(note_id).join("cron")
}

/// Appends a JSON line execution log to `<cron_dir>/<job_id>.jsonl`.
pub async fn append_job_log(
    cron_dir: &Path,
    job_id: &str,
    entry: &NoteCronJobLogEntry,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(cron_dir).await?;
    let log_file = cron_dir.join(format!("{}.jsonl", job_id));
    let mut json = serde_json::to_string(entry)?;
    json.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(json.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// Reads the most recent execution logs from `<cron_dir>/<job_id>.jsonl` (newest first).
pub async fn read_job_logs(
    cron_dir: &Path,
    job_id: &str,
    limit: usize,
) -> anyhow::Result<Vec<NoteCronJobLogEntry>> {
    let log_file = cron_dir.join(format!("{}.jsonl", job_id));
    if !log_file.exists() {
        return Ok(Vec::new());
    }
    let content = tokio::fs::read_to_string(&log_file).await?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            if let Ok(entry) = serde_json::from_str::<NoteCronJobLogEntry>(trimmed) {
                entries.push(entry);
            }
        }
    }
    entries.reverse();
    if entries.len() > limit {
        entries.truncate(limit);
    }
    Ok(entries)
}

/// Deletes the log file for a given job.
pub async fn delete_job_logs(cron_dir: &Path, job_id: &str) -> anyhow::Result<()> {
    let log_file = cron_dir.join(format!("{}.jsonl", job_id));
    if log_file.exists() {
        let _ = tokio::fs::remove_file(&log_file).await;
    }
    Ok(())
}

/// Parses interval string (e.g., "15m", "30m", "1h", "1d", "30s", "60") into seconds.
pub fn parse_interval_seconds(val: &str) -> Option<u64> {
    let s = val.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(sec) = s.parse::<u64>() {
        return Some(sec);
    }
    let (num_str, unit) = s.split_at(s.len().saturating_sub(1));
    let num = num_str.parse::<u64>().ok()?;
    match unit {
        "s" | "S" => Some(num),
        "m" | "M" => Some(num * 60),
        "h" | "H" => Some(num * 3600),
        "d" | "D" => Some(num * 86400),
        _ => None,
    }
}

/// Matches a single cron field pattern against an integer value.
pub fn matches_cron_field(field_pattern: &str, value: u32, min_val: u32, max_val: u32) -> bool {
    let pattern = field_pattern.trim();
    if pattern == "*" {
        return true;
    }
    for part in pattern.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((range, step_str)) = part.split_once('/') {
            let step: u32 = match step_str.parse() {
                Ok(st) if st > 0 => st,
                _ => return false,
            };
            let (start, end) = if range == "*" {
                (min_val, max_val)
            } else if let Some((start_str, end_str)) = range.split_once('-') {
                let s = start_str.parse::<u32>().unwrap_or(min_val);
                let e = end_str.parse::<u32>().unwrap_or(max_val);
                (s, e)
            } else {
                let s = range.parse::<u32>().unwrap_or(min_val);
                (s, max_val)
            };
            if value >= start && value <= end && (value - start) % step == 0 {
                return true;
            }
        } else if let Some((start_str, end_str)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start_str.parse::<u32>(), end_str.parse::<u32>()) {
                if value >= s && value <= e {
                    return true;
                }
            }
        } else if let Ok(exact) = part.parse::<u32>() {
            if exact == value {
                return true;
            }
        }
    }
    false
}

/// UTC date/time components for cron evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub day_of_week: u32, // 0 = Sunday, 1 = Monday, ..., 6 = Saturday
}

/// Converts Unix epoch seconds to UTC components.
pub fn unix_secs_to_utc(secs: i64) -> UtcDateTime {
    let days_since_epoch = (secs / 86400) as i32;
    let sec_in_day = (secs % 86400).rem_euclid(86400) as u32;

    let hour = sec_in_day / 3600;
    let minute = (sec_in_day % 3600) / 60;
    let second = sec_in_day % 60;

    // 1970-01-01 was Thursday (day 4)
    let day_of_week = ((days_since_epoch + 4).rem_euclid(7)) as u32;

    // Gregorian calculation from epoch days
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i32) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    UtcDateTime {
        year,
        month: m,
        day: d,
        hour,
        minute,
        second,
        day_of_week,
    }
}

/// Formats UTC date/time components as ISO-8601 string: `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_utc_iso(dt: UtcDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}

/// Parses ISO-8601 timestamp string roughly into approximate Unix seconds.
pub fn parse_iso_to_secs(iso_str: &str) -> Option<i64> {
    let s = iso_str.trim().trim_end_matches('Z');
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();
    if date_parts.len() != 3 || time_parts.len() < 2 {
        return None;
    }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let second: u32 = if time_parts.len() >= 3 {
        time_parts[2].parse().unwrap_or(0)
    } else {
        0
    };

    // Approximate days from year/month/day
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146097 + (doe as i32) - 719468) as i64;
    let secs = days * 86400 + (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    Some(secs)
}

/// Evaluates whether a cron job is currently due to run.
pub fn is_job_due(job: &NoteCronJobRecord, now_secs: i64) -> bool {
    if !job.enabled {
        return false;
    }

    let last_run_secs = job.last_run_at.as_deref().and_then(parse_iso_to_secs);

    if job.schedule_type.eq_ignore_ascii_case("interval") {
        let interval = match parse_interval_seconds(&job.schedule_value) {
            Some(sec) if sec > 0 => sec,
            _ => return false,
        };
        match last_run_secs {
            None => true,
            Some(last) => now_secs >= last + (interval as i64),
        }
    } else if job.schedule_type.eq_ignore_ascii_case("cron") {
        let fields: Vec<&str> = job.schedule_value.split_whitespace().collect();
        if fields.len() != 5 {
            return false;
        }
        let now_dt = unix_secs_to_utc(now_secs);

        // Check if already executed in the current minute
        if let Some(last) = last_run_secs {
            let last_dt = unix_secs_to_utc(last);
            if last_dt.year == now_dt.year
                && last_dt.month == now_dt.month
                && last_dt.day == now_dt.day
                && last_dt.hour == now_dt.hour
                && last_dt.minute == now_dt.minute
            {
                return false;
            }
        }

        let (min_pat, hour_pat, dom_pat, month_pat, dow_pat) =
            (fields[0], fields[1], fields[2], fields[3], fields[4]);

        let min_match = matches_cron_field(min_pat, now_dt.minute, 0, 59);
        let hour_match = matches_cron_field(hour_pat, now_dt.hour, 0, 23);
        let dom_match = matches_cron_field(dom_pat, now_dt.day, 1, 31);
        let month_match = matches_cron_field(month_pat, now_dt.month, 1, 12);
        let dow_match = matches_cron_field(dow_pat, now_dt.day_of_week, 0, 6)
            || (now_dt.day_of_week == 0 && matches_cron_field(dow_pat, 7, 0, 7));

        min_match && hour_match && dom_match && month_match && dow_match
    } else {
        false
    }
}

/// Checks if an agent answer should be considered silent (HEARTBEAT_OK / NO_ACTION).
pub fn is_heartbeat_silent(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let upper = trimmed.to_ascii_uppercase();
    upper == "HEARTBEAT_OK"
        || upper == "NO_ACTION"
        || upper.ends_with("HEARTBEAT_OK")
        || upper.ends_with("NO_ACTION")
        || upper.starts_with("HEARTBEAT_OK")
        || upper.starts_with("NO_ACTION")
        || upper == "\"HEARTBEAT_OK\""
}

/// Executes a single scheduled job against a notebook.
/// Executes a single scheduled job against a notebook.
pub async fn execute_cron_job(
    state: &ServerState,
    job: &NoteCronJobRecord,
    trigger: &str,
) -> NoteCronJobLogEntry {
    let now_dt = unix_secs_to_utc(crate::serve::db::now_secs());
    let now_ts = format_utc_iso(now_dt);
    let note_id = job.note_id.clone();

    // Concurrency check: prevent concurrent duplicate executions of the same job
    {
        let mut running = state.running_cron_jobs.write().await;
        if !running.insert(job.id.clone()) {
            warn!(
                "Cron job [{}] is already running, skipping execution",
                job.id
            );
            return NoteCronJobLogEntry {
                timestamp: now_ts,
                trigger: trigger.to_string(),
                status: "error".to_string(),
                duration_ms: 0,
                files_modified: vec![],
                output_snippet: String::new(),
                error: Some("Job is already running".to_string()),
                model: job.model.clone(),
                thinking: job.thinking.clone(),
                steps: None,
                tokens_in: None,
                tokens_out: None,
                tokens_used: None,
                tools: vec![],
            };
        }
    }

    let room = state.get_or_create_room(&note_id).await;

    // Broadcast running status to room via SSE
    broadcast_to_room(
        &room,
        &SseMsg::CronJobStatus {
            note_id: note_id.clone(),
            job_id: job.id.clone(),
            is_running: true,
        },
    );

    let res = execute_cron_job_inner(state, job, trigger, &room, &now_ts).await;

    // Release concurrency lock
    {
        let mut running = state.running_cron_jobs.write().await;
        running.remove(&job.id);
    }

    // Broadcast done status to room via SSE
    broadcast_to_room(
        &room,
        &SseMsg::CronJobStatus {
            note_id: note_id.clone(),
            job_id: job.id.clone(),
            is_running: false,
        },
    );

    res
}

async fn execute_cron_job_inner(
    state: &ServerState,
    job: &NoteCronJobRecord,
    trigger: &str,
    room: &Arc<crate::serve::NoteRoom>,
    now_ts: &str,
) -> NoteCronJobLogEntry {
    let start = std::time::Instant::now();
    let note_id = job.note_id.clone();

    info!(
        "Executing cron job [{}] for note [{}] (trigger: {})",
        job.name, note_id, trigger
    );

    // Resolve active model: Job override > Note override > Global default
    let active_model = if let Some(ref m) = job.model {
        m.clone()
    } else {
        state.effective_model(&note_id).await
    };

    let mut agent_cfg = state.config.clone();
    agent_cfg.model = active_model.clone();
    // Resolve active thinking: Job override > Note override > Global default
    let effective_thinking_level = if let Some(ref t) = job.thinking {
        Some(t.clone())
    } else {
        state.effective_thinking(&note_id).await
    };
    agent_cfg.thinking = effective_thinking_level.clone();

    let provider = match crate::serve::api::build_provider(&agent_cfg) {
        Ok(p) => p,
        Err(e) => {
            error!("Cron job [{}] provider init failed: {}", job.name, e);
            let log_entry = NoteCronJobLogEntry {
                timestamp: now_ts.to_string(),
                trigger: trigger.to_string(),
                status: "error".to_string(),
                duration_ms: 0,
                files_modified: vec![],
                output_snippet: String::new(),
                error: Some(format!("Provider error: {}", e)),
                model: Some(active_model),
                thinking: effective_thinking_level,
                steps: None,
                tokens_in: None,
                tokens_out: None,
                tokens_used: None,
                tools: vec![],
            };
            let _ = state
                .chat_db
                .update_cron_job_status_async(
                    job.id.clone(),
                    now_ts.to_string(),
                    "error".to_string(),
                )
                .await;
            let cron_dir = note_cron_dir(&state.data_dir, &note_id);
            let _ = append_job_log(&cron_dir, &job.id, &log_entry).await;
            return log_entry;
        }
    };

    let embedding = crate::serve::api::build_embedding(&agent_cfg).await;

    let mut agent = crate::agent::Agent::new(agent_cfg, provider, false, embedding);
    agent.set_serve_mode(true);
    agent.set_agent_skills(state.config.notes.agent_skills);
    agent.user_name = Some(format!("cron:{}", job.name));

    agent.markdown_dir = Some(state.note_markdown_dir(&note_id));
    agent.chat_db = Some(state.chat_db.clone());
    agent.chat_note_id = Some(note_id.clone());

    let files_modified = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let files_modified_cb = files_modified.clone();
    let state_for_fl = state.clone();
    let note_id_for_fl = note_id.clone();

    agent.file_list_callback = Some(Arc::new(move || {
        let s = state_for_fl.clone();
        let n = note_id_for_fl.clone();
        tokio::spawn(async move {
            broadcast_file_list(&s, &n).await;
        });
    }));

    let state_for_fc = state.clone();
    let note_id_for_fc = note_id.clone();
    agent.file_content_callback = Some(Arc::new(move |filename: String, content: String| {
        {
            let mut fm = files_modified_cb.lock().unwrap();
            if !fm.contains(&filename) {
                fm.push(filename.clone());
            }
        }
        let s = state_for_fc.clone();
        let n = note_id_for_fc.clone();
        tokio::spawn(async move {
            let room = s.get_or_create_room(&n).await;
            let fc = SseMsg::FileContent {
                note_id: n,
                filename,
                content,
            };
            broadcast_to_room(&room, &fc);
        });
    }));

    // Build system prompt (loads 7 persona files if enabled)
    let (system_prompt, _) = build_effective_note_system_prompt(state, &note_id).await;
    agent.set_system_prompt(&system_prompt);

    // Execute agent with the prompt specified by the cron job
    let prompt = &job.prompt;

    // Timeout: customizable (default 60s, clamped 5s - 3600s)
    let timeout_secs = job.timeout_secs.unwrap_or(60).clamp(5, 3600);
    let run_res = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        agent.run(prompt),
    )
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;
    let modified_list = {
        let fm = files_modified.lock().unwrap();
        fm.clone()
    };

    let (status, output_snippet, err_opt) = match run_res {
        Ok(StopReason::FinalAnswer(answer)) => {
            let is_silent =
                job.silent_if_no_action && modified_list.is_empty() && is_heartbeat_silent(&answer);

            if is_silent {
                ("silent_ok", answer, None)
            } else {
                // Broadcast system notification to room (not recorded in chat conversation history)
                let sys_content = if modified_list.is_empty() {
                    format!("⚡ Cron [{}]: {}", job.name, answer)
                } else {
                    format!(
                        "⚡ Cron [{}] (modified: {}): {}",
                        job.name,
                        modified_list.join(", "),
                        answer
                    )
                };
                let sys_msg = SseMsg::System {
                    content: sys_content,
                };
                broadcast_to_room(room, &sys_msg);

                ("success", answer, None)
            }
        }
        Ok(StopReason::Error(err)) => {
            error!("Cron job [{}] error: {}", job.name, err);
            ("error", String::new(), Some(err))
        }
        Ok(other) => {
            let repr = format!("{:?}", other);
            ("error", repr.clone(), Some(repr))
        }
        Err(_) => {
            warn!("Cron job [{}] timed out after {}s", job.name, timeout_secs);
            (
                "timeout",
                String::new(),
                Some(format!("Execution timed out after {}s", timeout_secs)),
            )
        }
    };

    let tools_used = agent.tool_call_names().to_vec();
    let log_entry = NoteCronJobLogEntry {
        timestamp: now_ts.to_string(),
        trigger: trigger.to_string(),
        status: status.to_string(),
        duration_ms,
        files_modified: modified_list,
        output_snippet: if output_snippet.len() > 4000 {
            output_snippet[..4000].to_string()
        } else {
            output_snippet
        },
        error: err_opt,
        model: Some(active_model),
        thinking: effective_thinking_level,
        steps: Some(agent.step_count()),
        tokens_in: Some(agent.tokens_in()),
        tokens_out: Some(agent.tokens_out()),
        tokens_used: Some(agent.tokens_used()),
        tools: tools_used,
    };

    // Update DB status
    let _ = state
        .chat_db
        .update_cron_job_status_async(job.id.clone(), now_ts.to_string(), status.to_string())
        .await;

    // Append to file log under ~/.rune/notes/{note}/cron/{job_id}.jsonl
    let cron_dir = note_cron_dir(&state.data_dir, &note_id);
    let _ = append_job_log(&cron_dir, &job.id, &log_entry).await;

    log_entry
}

/// Spawns the background cron worker loop if `config.notes.cron_jobs` is enabled.
pub fn start_cron_scheduler(state: ServerState, cancel_token: CancellationToken) {
    if !state.config.notes.cron_jobs {
        info!("Cron jobs worker is disabled (config.notes.cron_jobs = false)");
        return;
    }

    info!("Starting Rune Notes background Cron Scheduler worker");

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Cron scheduler worker shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let now_secs = crate::serve::db::now_secs();
                    if let Ok(jobs) = state.chat_db.list_all_enabled_cron_jobs_async().await {
                        for job in jobs {
                            if is_job_due(&job, now_secs) {
                                let running = state.running_cron_jobs.read().await;
                                if running.contains(&job.id) {
                                    continue;
                                }
                                drop(running);
                                let st = state.clone();
                                let j = job.clone();
                                tokio::spawn(async move {
                                    execute_cron_job(&st, &j, "schedule").await;
                                });
                            }
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_interval_seconds() {
        assert_eq!(parse_interval_seconds("15s"), Some(15));
        assert_eq!(parse_interval_seconds("30m"), Some(1800));
        assert_eq!(parse_interval_seconds("1h"), Some(3600));
        assert_eq!(parse_interval_seconds("1d"), Some(86400));
        assert_eq!(parse_interval_seconds("120"), Some(120));
        assert_eq!(parse_interval_seconds("invalid"), None);
    }

    #[test]
    fn test_matches_cron_field() {
        assert!(matches_cron_field("*", 15, 0, 59));
        assert!(matches_cron_field("*/15", 30, 0, 59));
        assert!(!matches_cron_field("*/15", 32, 0, 59));
        assert!(matches_cron_field("1,15,30", 15, 0, 59));
        assert!(!matches_cron_field("1,15,30", 16, 0, 59));
        assert!(matches_cron_field("1-5", 3, 0, 59));
        assert!(!matches_cron_field("1-5", 6, 0, 59));
    }

    #[test]
    fn test_unix_secs_to_utc_and_iso() {
        // 2026-09-21 14:30:00 UTC = 1790001000 approx
        let dt = unix_secs_to_utc(0);
        assert_eq!(dt.year, 1970);
        assert_eq!(dt.month, 1);
        assert_eq!(dt.day, 1);
        assert_eq!(dt.hour, 0);
        assert_eq!(dt.minute, 0);
        assert_eq!(dt.second, 0);
        assert_eq!(dt.day_of_week, 4); // Thursday

        let iso = format_utc_iso(dt);
        assert_eq!(iso, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn test_is_heartbeat_silent() {
        assert!(is_heartbeat_silent("HEARTBEAT_OK"));
        assert!(is_heartbeat_silent("heartbeat_ok"));
        assert!(is_heartbeat_silent("NO_ACTION"));
        assert!(is_heartbeat_silent("Everything is fine. HEARTBEAT_OK"));
        assert!(!is_heartbeat_silent("Found an issue in TODO.md!"));
    }

    #[tokio::test]
    async fn test_cron_file_logs() {
        let tmp = TempDir::new().unwrap();
        let cron_dir = note_cron_dir(tmp.path(), "my-note");

        let entry1 = NoteCronJobLogEntry {
            timestamp: "2026-09-21T14:00:00Z".to_string(),
            trigger: "schedule".to_string(),
            status: "silent_ok".to_string(),
            duration_ms: 120,
            files_modified: vec![],
            output_snippet: "HEARTBEAT_OK".to_string(),
            error: None,
            model: Some("openrouter/auto".to_string()),
            thinking: Some("low".to_string()),
            steps: Some(1),
            tokens_in: Some(50),
            tokens_out: Some(10),
            tokens_used: Some(60),
            tools: vec![],
        };

        let entry2 = NoteCronJobLogEntry {
            timestamp: "2026-09-21T14:30:00Z".to_string(),
            trigger: "schedule".to_string(),
            status: "success".to_string(),
            duration_ms: 450,
            files_modified: vec!["TODO.md".to_string()],
            output_snippet: "Updated TODO".to_string(),
            error: None,
            model: Some("openrouter/auto".to_string()),
            thinking: Some("low".to_string()),
            steps: Some(2),
            tokens_in: Some(120),
            tokens_out: Some(80),
            tokens_used: Some(200),
            tools: vec!["write_markdown".to_string()],
        };

        append_job_log(&cron_dir, "job_1", &entry1).await.unwrap();
        append_job_log(&cron_dir, "job_1", &entry2).await.unwrap();

        let logs = read_job_logs(&cron_dir, "job_1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        // Newest first
        assert_eq!(logs[0].status, "success");
        assert_eq!(logs[1].status, "silent_ok");

        delete_job_logs(&cron_dir, "job_1").await.unwrap();
        let logs_after = read_job_logs(&cron_dir, "job_1", 10).await.unwrap();
        assert_eq!(logs_after.len(), 0);
    }
}
