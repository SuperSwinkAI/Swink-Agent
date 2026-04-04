//! Built-in tool for writing content to a file.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};
use crate::types::ContentBlock;

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
            schema: validated_schema_for::<Params>(),
        }
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Absolute path to write.
    path: String,
    /// Content to write to the file.
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

            let path = std::path::Path::new(&parsed.path);

            // Read existing content for diff (empty string if file doesn't exist)
            let old_content = tokio::fs::read_to_string(path).await.unwrap_or_default();

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
                Ok(()) => AgentToolResult {
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "Successfully wrote {bytes_written} bytes to {}",
                            parsed.path
                        ),
                    }],
                    details: serde_json::json!({
                        "path": parsed.path,
                        "bytes_written": bytes_written,
                        "is_new_file": old_content.is_empty(),
                        "old_content": old_content,
                        "new_content": parsed.content,
                    }),
                    is_error: false,
                    transfer_signal: None,
                },
                Err(e) => {
                    AgentToolResult::error(format!("failed to write file {}: {e}", parsed.path))
                }
            }
        })
    }
}
