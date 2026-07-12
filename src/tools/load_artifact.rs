//! Built-in tool for loading versioned artifacts.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::artifact::ArtifactStore;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};

/// Built-in tool that loads a previously saved artifact.
pub struct LoadArtifactTool {
    store: Arc<dyn ArtifactStore>,
    schema: Value,
}

impl LoadArtifactTool {
    /// Create a new `LoadArtifactTool` backed by the given store.
    #[must_use]
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self {
        Self {
            store,
            schema: validated_schema_for::<Params>(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Artifact name to load.
    name: String,
    /// Specific version to load (latest if omitted).
    version: Option<u32>,
}

impl AgentTool for LoadArtifactTool {
    fn name(&self) -> &str {
        "load_artifact"
    }

    fn label(&self) -> &str {
        "Load Artifact"
    }

    fn description(&self) -> &str {
        "Load a previously saved artifact from the current session."
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
        state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
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

            let session_id = {
                let guard = state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match guard.get::<String>("session_id") {
                    Some(id) => id,
                    None => return AgentToolResult::error("no session_id in state"),
                }
            };

            let result = if let Some(ver) = parsed.version {
                self.store
                    .load_version(&session_id, &parsed.name, ver)
                    .await
            } else {
                self.store.load(&session_id, &parsed.name).await
            };

            match result {
                Ok(Some((data, version))) => {
                    if data.content_type.starts_with("text/") {
                        match String::from_utf8(data.content) {
                            Ok(text) => AgentToolResult::text(text),
                            Err(_) => AgentToolResult::text(format!(
                                "[binary: {} bytes, type: {}]",
                                version.size, data.content_type
                            )),
                        }
                    } else {
                        AgentToolResult::text(format!(
                            "[binary: {} bytes, type: {}]",
                            version.size, data.content_type
                        ))
                    }
                }
                Ok(None) => AgentToolResult::error(format!("artifact '{}' not found", parsed.name)),
                Err(e) => AgentToolResult::error(format!("{e}")),
            }
        })
    }
}
