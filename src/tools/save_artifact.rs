//! Built-in tool for saving versioned artifacts.

use std::collections::HashMap;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::artifact::{ArtifactData, ArtifactStore};
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};

/// Built-in tool that saves content as a versioned artifact.
pub struct SaveArtifactTool<S: ArtifactStore + 'static> {
    store: Arc<S>,
    schema: Value,
}

impl<S: ArtifactStore + 'static> SaveArtifactTool<S> {
    /// Create a new `SaveArtifactTool` backed by the given store.
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            schema: validated_schema_for::<Params>(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Params {
    /// Artifact name (e.g., 'report.md', 'data/output.csv').
    name: String,
    /// Content to save.
    content: String,
    /// MIME type (defaults to 'text/plain').
    content_type: Option<String>,
}

#[allow(clippy::unnecessary_literal_bound)]
impl<S: ArtifactStore + 'static> AgentTool for SaveArtifactTool<S> {
    fn name(&self) -> &str {
        "save_artifact"
    }

    fn label(&self) -> &str {
        "Save Artifact"
    }

    fn description(&self) -> &str {
        "Save content as a versioned artifact in the current session."
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
                let guard = state.read().unwrap_or_else(|e| e.into_inner());
                match guard.get::<String>("session_id") {
                    Some(id) => id,
                    None => return AgentToolResult::error("no session_id in state"),
                }
            };

            let content_type = parsed
                .content_type
                .unwrap_or_else(|| "text/plain".to_string());

            let data = ArtifactData {
                content: parsed.content.into_bytes(),
                content_type,
                metadata: HashMap::new(),
            };

            match self.store.save(&session_id, &parsed.name, data).await {
                Ok(version) => {
                    AgentToolResult::text(format!(
                        "Saved '{}' version {}",
                        parsed.name, version.version
                    ))
                }
                Err(e) => AgentToolResult::error(format!("{e}")),
            }
        })
    }
}
