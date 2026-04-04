//! Built-in tool for listing session artifacts.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::artifact::ArtifactStore;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};

/// Built-in tool that lists all artifacts in the current session.
pub struct ListArtifactsTool<S: ArtifactStore + 'static> {
    store: Arc<S>,
    schema: Value,
}

impl<S: ArtifactStore + 'static> ListArtifactsTool<S> {
    /// Create a new `ListArtifactsTool` backed by the given store.
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
struct Params {}

#[allow(clippy::unnecessary_literal_bound)]
impl<S: ArtifactStore + 'static> AgentTool for ListArtifactsTool<S> {
    fn name(&self) -> &str {
        "list_artifacts"
    }

    fn label(&self) -> &str {
        "List Artifacts"
    }

    fn description(&self) -> &str {
        "List all artifacts saved in the current session."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
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

            match self.store.list(&session_id).await {
                Ok(metas) => {
                    if metas.is_empty() {
                        AgentToolResult::text("No artifacts in this session.")
                    } else {
                        let mut lines = vec!["Artifacts:".to_string()];
                        for m in &metas {
                            lines.push(format!(
                                "- {} (v{}, {})",
                                m.name, m.latest_version, m.content_type
                            ));
                        }
                        AgentToolResult::text(lines.join("\n"))
                    }
                }
                Err(e) => AgentToolResult::error(format!("{e}")),
            }
        })
    }
}
