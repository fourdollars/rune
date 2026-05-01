use anyhow::{bail, Result};
use std::time::Instant;
use tracing::{info, error};
use tokio::process::Command;
use std::process::Stdio;

/// 預處理指令的執行結果
#[derive(Debug)]
pub struct PreCommandResult {
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
}

/// 依序執行一組 pre-commands
/// 任何一個失敗就立即中止並回傳 Err
pub async fn execute_pre_commands(commands: &[String]) -> Result<Vec<PreCommandResult>> {
    let mut results = Vec::new();
    for cmd in commands {
        info!(command = %cmd, "executing pre-command");
        let start = Instant::now();

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                error!(command = %cmd, error = %e, "failed to spawn pre-command");
                e
            })?;

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let result = PreCommandResult {
            command: cmd.clone(),
            exit_code,
            duration_ms,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
        };

        if exit_code != 0 {
            error!(command = %cmd, exit_code, "pre-command failed");
            bail!("pre-command failed: '{}' exit code {}\nstdout: {}\nstderr: {}", cmd, exit_code, stdout, stderr);
        }

        info!(command = %cmd, exit_code = result.exit_code, duration_ms = result.duration_ms, "pre-command completed");
        results.push(result);
    }
    Ok(results)
}
