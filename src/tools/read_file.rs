//! Built-in tool for reading file contents.

use std::future::Future;
use std::pin::Pin;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::MAX_OUTPUT_BYTES;
use crate::schema::schema_for;
use crate::tool::{AgentTool, AgentToolResult, validate_schema};

/// Built-in tool that reads a file and returns its contents.
pub struct ReadFileTool {
    schema: Value,
}

impl ReadFileTool {
    /// Create a new `ReadFileTool`.
    #[must_use]
    pub fn new() -> Self {
        let schema = schema_for::<Params>();
        debug_assert!(validate_schema(&schema).is_ok());
        Self { schema }
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

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            let parsed: Params = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            match tokio::fs::read_to_string(&parsed.path).await {
                Ok(mut content) => {
                    if content.len() > MAX_OUTPUT_BYTES {
                        content.truncate(MAX_OUTPUT_BYTES);
                        content.push_str("\n[truncated]");
                    }
                    AgentToolResult::text(content)
                }
                Err(e) => {
                    AgentToolResult::error(format!("failed to read file {}: {e}", parsed.path))
                }
            }
        })
    }
}
