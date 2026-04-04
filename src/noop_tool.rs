//! Placeholder tool for session history compatibility.
//!
//! When a session references a tool that is no longer registered,
//! [`NoopTool`] is auto-injected to prevent deserialization failures.
//! It returns an error result explaining the tool is no longer available.
//!
//! # Example
//!
//! ```
//! use swink_agent::{AgentTool, NoopTool};
//!
//! let tool = NoopTool::new("old_tool");
//! assert_eq!(tool.name(), "old_tool");
//! assert!(!tool.requires_approval());
//! ```

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult, ToolFuture, permissive_object_schema};

// ─── NoopTool ──────────────────────────────────────────────────────────────

/// A placeholder tool that returns an error message when invoked.
///
/// Used for session history compatibility when a tool referenced in a saved
/// session no longer exists in the agent's registry.
#[derive(Debug, Clone)]
pub struct NoopTool {
    name: String,
}

impl NoopTool {
    /// Create a new `NoopTool` with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl AgentTool for NoopTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &'static str {
        "This tool is no longer available."
    }

    fn parameters_schema(&self) -> &Value {
        // Accept any arguments (the tool won't execute them anyway).
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(permissive_object_schema);
        &SCHEMA
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let name = self.name.clone();
        Box::pin(async move {
            AgentToolResult::error(format!(
                "Tool '{name}' is no longer available. It may have been removed or renamed."
            ))
        })
    }
}

// ─── Compile-time Send + Sync assertion ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NoopTool>();
};

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::tool::AgentTool;

    fn test_state() -> Arc<std::sync::RwLock<crate::SessionState>> {
        Arc::new(std::sync::RwLock::new(crate::SessionState::new()))
    }

    #[test]
    fn noop_tool_name_matches() {
        let tool = NoopTool::new("old_tool");
        assert_eq!(tool.name(), "old_tool");
    }

    #[test]
    fn noop_tool_no_approval_required() {
        let tool = NoopTool::new("removed_tool");
        assert!(!tool.requires_approval());
    }

    #[tokio::test]
    async fn noop_tool_returns_error() {
        let tool = NoopTool::new("deleted_tool");
        let result = tool
            .execute(
                "call_1",
                json!({"any": "args"}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(result.is_error);
        let crate::types::ContentBlock::Text { text } = &result.content[0] else {
            panic!("expected text content");
        };
        assert!(text.contains("deleted_tool"));
        assert!(text.contains("no longer available"));
    }

    #[tokio::test]
    async fn noop_tool_ignores_arguments() {
        let tool = NoopTool::new("any");
        let result = tool
            .execute(
                "call_2",
                json!({"complex": {"nested": true}, "array": [1, 2, 3]}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(result.is_error);
    }
}
