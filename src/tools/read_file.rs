//! Built-in tool for reading file contents.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{MAX_OUTPUT_BYTES, path::resolve_existing_path, truncate_utf8_to_boundary};
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};

/// Built-in tool that reads a file and returns its contents.
pub struct ReadFileTool {
    schema: Value,
    execution_root: Option<PathBuf>,
}

impl ReadFileTool {
    /// Create a new `ReadFileTool`.
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

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Absolute path to the file to read.
    path: String,
}

#[allow(clippy::unnecessary_literal_bound)]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn label(&self) -> &str {
        "Read File"
    }

    fn description(&self) -> &str {
        "Read a file and return its contents."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
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

            let path =
                match resolve_existing_path(&parsed.path, self.execution_root.as_deref()).await {
                    Ok(path) => path,
                    Err(error) => return AgentToolResult::error(error),
                };

            match read_limited_utf8_file(&path).await {
                Ok((mut content, truncated)) => {
                    if truncated {
                        truncate_utf8_to_boundary(&mut content, MAX_OUTPUT_BYTES);
                    }
                    AgentToolResult::text(content)
                }
                Err(e) => {
                    AgentToolResult::error(format!("failed to read file {}: {e}", path.display()))
                }
            }
        })
    }
}

async fn read_limited_utf8_file(path: &Path) -> std::io::Result<(String, bool)> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::with_capacity(MAX_OUTPUT_BYTES + 1);
    let mut reader = file.take((MAX_OUTPUT_BYTES + 1) as u64);
    reader.read_to_end(&mut bytes).await?;

    let truncated = bytes.len() > MAX_OUTPUT_BYTES;
    match String::from_utf8(bytes) {
        Ok(content) => Ok((content, truncated)),
        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let bytes = error.into_bytes();
            let content = String::from_utf8(bytes[..valid_up_to].to_vec())
                .expect("valid UTF-8 prefix should decode");
            Ok((content, truncated))
        }
        Err(error) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.utf8_error(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::SessionState;
    use crate::types::ContentBlock;

    fn result_text(result: &AgentToolResult) -> &str {
        match result.content.first() {
            Some(ContentBlock::Text { text }) => text.as_str(),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn read_file_truncates_multibyte_content_on_char_boundary() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let content = "€".repeat((MAX_OUTPUT_BYTES / "€".len()) + 1);
        tokio::fs::write(temp.path(), content).await.unwrap();

        let result = ReadFileTool::new()
            .execute(
                "call-1",
                json!({ "path": temp.path().to_str().unwrap() }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::new())),
                None,
            )
            .await;

        let text = result_text(&result);
        assert!(!result.is_error);
        assert!(text.contains("[truncated]"), "expected marker in: {text}");
        assert!(text.is_char_boundary(text.len()));
    }

    #[tokio::test]
    async fn read_file_resolves_relative_path_against_execution_root() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(temp.path().join("relative.txt"), "rooted")
            .await
            .unwrap();

        let result = ReadFileTool::new()
            .with_execution_root(temp.path())
            .execute(
                "call-1",
                json!({ "path": "relative.txt" }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::new())),
                None,
            )
            .await;

        assert!(!result.is_error);
        assert_eq!(result_text(&result), "rooted");
    }

    #[tokio::test]
    async fn read_file_rejects_relative_path_outside_execution_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(temp.path().join("outside.txt"), "outside")
            .await
            .unwrap();

        let result = ReadFileTool::new()
            .with_execution_root(&root)
            .execute(
                "call-1",
                json!({ "path": "../outside.txt" }),
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
    }
}
