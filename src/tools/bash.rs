//! Built-in tool for executing shell commands.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult};

/// Maximum combined stdout + stderr size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Built-in tool that executes a shell command via `sh -c`.
pub struct BashTool {
    schema: Value,
}

impl BashTool {
    /// Create a new `BashTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (default 30000)"
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct Params {
    command: String,
    timeout_ms: Option<u64>,
}

#[allow(clippy::unnecessary_literal_bound)]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn label(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            let parsed: Params = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("Invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("Cancelled");
            }

            let timeout = Duration::from_millis(
                parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
            );

            let mut child = match Command::new("sh")
                .arg("-c")
                .arg(&parsed.command)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    return AgentToolResult::error(format!("Failed to spawn command: {e}"));
                }
            };

            // Take stdout/stderr handles before waiting so we can read them
            // after the process exits without moving `child`.
            let mut stdout_handle = child.stdout.take();
            let mut stderr_handle = child.stderr.take();

            tokio::select! {
                result = child.wait() => {
                    match result {
                        Ok(status) => {
                            let stdout = read_stream(&mut stdout_handle).await;
                            let stderr = read_stream(&mut stderr_handle).await;
                            format_output(status.code(), &stdout, &stderr)
                        }
                        Err(e) => AgentToolResult::error(format!("Command execution failed: {e}")),
                    }
                }
                () = cancellation_token.cancelled() => {
                    let _ = child.kill().await;
                    AgentToolResult::error("Cancelled")
                }
                () = tokio::time::sleep(timeout) => {
                    let _ = child.kill().await;
                    AgentToolResult::error(format!(
                        "Command timed out after {}ms",
                        timeout.as_millis()
                    ))
                }
            }
        })
    }
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    if let Some(p) = pipe {
        let mut buf = Vec::new();
        let _ = p.read_to_end(&mut buf).await;
        buf
    } else {
        Vec::new()
    }
}

fn format_output(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> AgentToolResult {
    let code_str = exit_code
        .map_or_else(|| "unknown".to_owned(), |c| c.to_string());

    let mut stdout_text = String::from_utf8_lossy(stdout).into_owned();
    let mut stderr_text = String::from_utf8_lossy(stderr).into_owned();

    let combined_len = stdout_text.len() + stderr_text.len();
    if combined_len > MAX_OUTPUT_BYTES {
        // Truncate proportionally, favouring stdout.
        let stdout_budget = MAX_OUTPUT_BYTES * stdout_text.len() / combined_len.max(1);
        let stderr_budget = MAX_OUTPUT_BYTES.saturating_sub(stdout_budget);

        if stdout_text.len() > stdout_budget {
            stdout_text.truncate(stdout_budget);
            stdout_text.push_str("\n[truncated]");
        }
        if stderr_text.len() > stderr_budget {
            stderr_text.truncate(stderr_budget);
            stderr_text.push_str("\n[truncated]");
        }
    }

    let mut text = format!("Exit code: {code_str}");

    if !stdout_text.is_empty() {
        use std::fmt::Write;
        let _ = write!(text, "\n\nStdout:\n{stdout_text}");
    }
    if !stderr_text.is_empty() {
        use std::fmt::Write;
        let _ = write!(text, "\n\nStderr:\n{stderr_text}");
    }

    AgentToolResult::text(text)
}
