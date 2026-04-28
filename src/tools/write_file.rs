//! Built-in tool for writing content to a file.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::path::resolve_writable_path;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};
use crate::types::ContentBlock;

/// Built-in tool that writes content to a file, creating parent directories as
/// needed.
pub struct WriteFileTool {
    schema: Value,
    execution_root: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::SessionState;

    fn result_text(result: &AgentToolResult) -> &str {
        match result.content.first() {
            Some(ContentBlock::Text { text }) => text.as_str(),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn write_file_rejects_relative_path_outside_execution_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        tokio::fs::create_dir(&root).await.unwrap();
        let outside = temp.path().join("outside.txt");

        let result = WriteFileTool::new()
            .with_execution_root(&root)
            .execute(
                "call-1",
                json!({ "path": "../outside.txt", "content": "outside" }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::new())),
                None,
            )
            .await;

        assert!(result.is_error);
        assert!(
            result_text(&result).contains("escapes execution root"),
            "unexpected result: {}",
            result_text(&result)
        );
        assert!(
            !tokio::fs::try_exists(&outside).await.unwrap(),
            "write escaped execution root"
        );
    }
}

impl WriteFileTool {
    /// Create a new `WriteFileTool`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: validated_schema_for::<Params>(),
            execution_root: None,
        }
    }

    /// Set the working directory used to resolve relative file paths.
    #[must_use]
    pub fn with_execution_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.execution_root = Some(root.into());
        self
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

    fn execution_root(&self) -> Option<&Path> {
        self.execution_root.as_deref()
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

            let path =
                match resolve_writable_path(&parsed.path, self.execution_root.as_deref()).await {
                    Ok(path) => path,
                    Err(error) => return AgentToolResult::error(error),
                };

            // Read existing content for diff (empty string if file doesn't exist)
            let old_content = tokio::fs::read_to_string(&path).await.unwrap_or_default();

            if let Some(parent) = path.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                return AgentToolResult::error(format!(
                    "failed to create parent directories for {}: {e}",
                    parsed.path
                ));
            }

            let bytes_written = parsed.content.len();
            match tokio::fs::write(&path, &parsed.content).await {
                Ok(()) => AgentToolResult {
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "Successfully wrote {bytes_written} bytes to {}",
                            path.display()
                        ),
                    }],
                    details: serde_json::json!({
                        "path": path,
                        "bytes_written": bytes_written,
                        "is_new_file": old_content.is_empty(),
                        "old_content": old_content,
                        "new_content": parsed.content,
                    }),
                    is_error: false,
                    transfer_signal: None,
                },
                Err(e) => {
                    AgentToolResult::error(format!("failed to write file {}: {e}", path.display()))
                }
            }
        })
    }
}
