//! Built-in tool for executing shell commands.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::MAX_OUTPUT_BYTES;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Built-in tool that executes a shell command.
///
/// On Unix-like targets the command is passed to `sh -c`. On Windows the
/// command is passed to `cmd /C`, matching the platform's native shell.
///
/// # Security
///
/// This tool executes arbitrary shell commands via the platform shell. It
/// should only be used with trusted input. It is NOT suitable for production
/// agents exposed to untrusted users.
pub struct BashTool {
    schema: Value,
}

impl BashTool {
    /// Create a new `BashTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: validated_schema_for::<Params>(),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Shell command to execute.
    command: String,
    /// Timeout in milliseconds (default 30000).
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

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            let parsed: Params = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            let timeout = Duration::from_millis(parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

            let mut child = match shell_command(&parsed.command)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    return AgentToolResult::error(format!("failed to spawn command: {e}"));
                }
            };

            // Spawn concurrent readers for stdout/stderr to prevent deadlocks
            // when OS pipe buffers fill up on large output.
            let stdout_task = tokio::spawn(read_stream(child.stdout.take()));
            let stderr_task = tokio::spawn(read_stream(child.stderr.take()));

            tokio::select! {
                result = child.wait() => {
                    match result {
                        Ok(status) => {
                            let stdout = stdout_task.await.unwrap_or_default();
                            let stderr = stderr_task.await.unwrap_or_default();
                            format_output(status.code(), &stdout, &stderr)
                        }
                        Err(e) => AgentToolResult::error(format!("failed to execute command: {e}")),
                    }
                }
                () = cancellation_token.cancelled() => {
                    let _ = child.kill().await;
                    stdout_task.abort();
                    stderr_task.abort();
                    AgentToolResult::error("cancelled")
                }
                () = tokio::time::sleep(timeout) => {
                    let _ = child.kill().await;
                    stdout_task.abort();
                    stderr_task.abort();
                    AgentToolResult::error(format!(
                        "failed to complete command: timed out after {}ms",
                        timeout.as_millis()
                    ))
                }
            }
        })
    }
}

/// Build a platform-appropriate shell `Command` that executes `command`.
///
/// Unix: `sh -c <command>`. Windows: `cmd /C <command>`.
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin + Send + 'static>(pipe: Option<R>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    if let Some(mut p) = pipe {
        let mut buf = Vec::new();
        let _ = p.read_to_end(&mut buf).await;
        buf
    } else {
        Vec::new()
    }
}

fn format_output(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> AgentToolResult {
    let code_str = exit_code.map_or_else(|| "unknown".to_owned(), |c| c.to_string());

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
