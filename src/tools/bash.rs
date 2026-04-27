//! Built-in tool for executing shell commands.

use std::path::{Path, PathBuf};
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{MAX_OUTPUT_BYTES, truncate_utf8_to_boundary};
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
    execution_root: Option<PathBuf>,
}

impl BashTool {
    /// Create a new `BashTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: validated_schema_for::<Params>(),
            execution_root: None,
        }
    }

    /// Set the working directory used when spawning shell commands.
    #[must_use]
    pub fn with_execution_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.execution_root = Some(root.into());
        self
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

    fn execution_root(&self) -> Option<&Path> {
        self.execution_root.as_deref()
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

            let mut command = shell_command(&parsed.command);
            if let Some(root) = self.execution_root.as_deref() {
                command.current_dir(root);
            }

            let mut child = match command
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
    if let Some(p) = pipe {
        let mut buf = Vec::with_capacity(MAX_OUTPUT_BYTES + 1);
        let _ = p
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut buf)
            .await;
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
            truncate_utf8_to_boundary(&mut stdout_text, stdout_budget);
        }
        if stderr_text.len() > stderr_budget {
            truncate_utf8_to_boundary(&mut stderr_text, stderr_budget);
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

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;
    use crate::types::ContentBlock;

    fn result_text(result: &AgentToolResult) -> &str {
        match result.content.first() {
            Some(ContentBlock::Text { text }) => text.as_str(),
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn format_output_truncates_multibyte_stdout_on_char_boundary() {
        let stdout = "€".repeat((MAX_OUTPUT_BYTES / "€".len()) + 1);

        let result = format_output(Some(0), stdout.as_bytes(), &[]);
        let text = result_text(&result);

        assert!(text.contains("[truncated]"), "expected marker in: {text}");
        assert!(text.is_char_boundary(text.len()));
    }

    #[tokio::test]
    async fn read_stream_stops_after_output_budget_sentinel() {
        let (reader, mut writer) = tokio::io::duplex(1024);
        let writer_task = tokio::spawn(async move {
            let bytes = vec![b'a'; MAX_OUTPUT_BYTES + 2];
            let _ = writer.write_all(&bytes).await;
        });

        let output = read_stream(Some(reader)).await;
        let _ = writer_task.await;

        assert_eq!(output.len(), MAX_OUTPUT_BYTES + 1);
    }
}
