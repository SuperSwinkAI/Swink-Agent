//! Built-in tool for writing content to a file.

use std::future::Future;
use std::pin::Pin;

use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult};

/// Built-in tool that writes content to a file, creating parent directories as
/// needed.
pub struct WriteFileTool {
    schema: Value,
}

impl WriteFileTool {
    /// Create a new `WriteFileTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct Params {
    path: String,
    content: String,
}

#[allow(clippy::unnecessary_literal_bound)]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn label(&self) -> &str {
        "Write File"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories if needed."
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
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            let path = std::path::Path::new(&parsed.path);

            if let Some(parent) = path.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                return AgentToolResult::error(format!(
                    "failed to create parent directories for {}: {e}",
                    parsed.path
                ));
            }

            let bytes_written = parsed.content.len();
            match tokio::fs::write(path, &parsed.content).await {
                Ok(()) => AgentToolResult::text(format!(
                    "Successfully wrote {bytes_written} bytes to {}",
                    parsed.path
                )),
                Err(e) => AgentToolResult::error(format!(
                    "failed to write file {}: {e}",
                    parsed.path
                )),
            }
        })
    }
}
