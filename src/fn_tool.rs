//! Closure-based tool builder that implements [`AgentTool`] without requiring
//! a custom struct or trait implementation.
//!
//! # Example
//!
//! ```
//! use schemars::JsonSchema;
//! use serde::Deserialize;
//! use swink_agent::{AgentToolResult, FnTool};
//!
//! #[derive(Deserialize, JsonSchema)]
//! struct Params { city: String }
//!
//! let tool = FnTool::new("get_weather", "Weather", "Get weather for a city.")
//!     .with_execute_typed(|params: Params, _cancel| async move {
//!         AgentToolResult::text(format!("72F in {}", params.city))
//!     });
//!
//! assert_eq!(swink_agent::AgentTool::name(&tool), "get_weather");
//! ```

use std::future::Future;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{
    AgentTool, AgentToolResult, ToolFuture, debug_validated_schema, permissive_object_schema,
    validated_schema_for,
};

// ─── Type aliases for stored closures ───────────────────────────────────────

type ExecuteFn = Arc<
    dyn Fn(
            String,
            Value,
            CancellationToken,
            Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> ToolFuture<'static>
        + Send
        + Sync,
>;

type ApprovalContextFn = Arc<dyn Fn(&Value) -> Option<Value> + Send + Sync>;

// ─── FnTool ─────────────────────────────────────────────────────────────────

/// A tool built entirely from closures and configuration, implementing
/// [`AgentTool`] without requiring a custom struct.
///
/// Use the builder methods to configure the tool's schema, approval
/// requirements, and execution logic.
pub struct FnTool {
    name: String,
    label: String,
    description: String,
    schema: Value,
    requires_approval: bool,
    execute_fn: ExecuteFn,
    approval_context_fn: Option<ApprovalContextFn>,
}

impl FnTool {
    /// Create a new `FnTool` with the given name, label, and description.
    ///
    /// The default schema accepts any object and the default execute returns
    /// an error indicating the tool is not implemented.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            schema: permissive_object_schema(),
            requires_approval: false,
            execute_fn: Arc::new(|_, _, _, _| {
                Box::pin(async { AgentToolResult::error("not implemented") })
            }),
            approval_context_fn: None,
        }
    }

    /// Set the parameters schema from a type implementing
    /// [`JsonSchema`](schemars::JsonSchema).
    #[must_use]
    pub fn with_schema_for<T: schemars::JsonSchema>(mut self) -> Self {
        self.schema = validated_schema_for::<T>();
        self
    }

    /// Set the parameters schema from a raw JSON value.
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = debug_validated_schema(schema);
        self
    }

    /// Set whether this tool requires user approval before execution.
    #[must_use]
    pub const fn with_requires_approval(mut self, requires: bool) -> Self {
        self.requires_approval = requires;
        self
    }

    /// Set the execution function using the full signature.
    ///
    /// The closure receives `(tool_call_id, params, cancellation_token, on_update)`.
    #[must_use]
    pub fn with_execute<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(
                String,
                Value,
                CancellationToken,
                Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            ) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static,
    {
        self.execute_fn = Arc::new(move |id, params, cancel, on_update| {
            Box::pin(f(id, params, cancel, on_update))
        });
        self
    }

    /// Set the execution function using a simplified signature.
    ///
    /// The closure receives only `(params, cancellation_token)`, ignoring the
    /// tool call ID and update callback.
    #[must_use]
    pub fn with_execute_simple<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Value, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static,
    {
        self.execute_fn =
            Arc::new(move |_id, params, cancel, _on_update| Box::pin(f(params, cancel)));
        self
    }

    /// Set the execution function using a typed parameter struct.
    ///
    /// This derives the schema from `T` and deserializes validated params into
    /// `T` before calling the closure. On deserialization failure, execution
    /// returns `AgentToolResult::error("invalid parameters: ...")`.
    #[must_use]
    pub fn with_execute_typed<T, F, Fut>(mut self, f: F) -> Self
    where
        T: DeserializeOwned + schemars::JsonSchema + Send + 'static,
        F: Fn(T, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static,
    {
        self.schema = validated_schema_for::<T>();
        self.execute_fn = Arc::new(move |_id, params, cancel, _on_update| {
            let parsed: T = match serde_json::from_value(params) {
                Ok(parsed) => parsed,
                Err(err) => {
                    return Box::pin(async move {
                        AgentToolResult::error(format!("invalid parameters: {err}"))
                    });
                }
            };
            Box::pin(f(parsed, cancel))
        });
        self
    }

    /// Set a closure that provides rich context for the approval UI.
    ///
    /// When the tool requires approval, this closure is called to produce
    /// context that is attached to the [`ToolApprovalRequest`](crate::ToolApprovalRequest).
    #[must_use]
    pub fn with_approval_context<F>(mut self, f: F) -> Self
    where
        F: Fn(&Value) -> Option<Value> + Send + Sync + 'static,
    {
        self.approval_context_fn = Some(Arc::new(f));
        self
    }
}

impl AgentTool for FnTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn approval_context(&self, params: &Value) -> Option<Value> {
        self.approval_context_fn.as_ref().and_then(|f| f(params))
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let fut = (self.execute_fn)(
            tool_call_id.to_owned(),
            params,
            cancellation_token,
            on_update,
        );
        Box::pin(fut)
    }
}

impl std::fmt::Debug for FnTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("requires_approval", &self.requires_approval)
            .finish_non_exhaustive()
    }
}

// ─── Compile-time Send + Sync assertion ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FnTool>();
};

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::ContentBlock;

    fn test_state() -> std::sync::Arc<std::sync::RwLock<crate::SessionState>> {
        std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::new()))
    }

    fn sample_tool() -> FnTool {
        FnTool::new("test", "Test", "A test tool.")
    }

    #[test]
    fn metadata_matches_constructor() {
        let tool = sample_tool();
        assert_eq!(tool.name(), "test");
        assert_eq!(tool.label(), "Test");
        assert_eq!(tool.description(), "A test tool.");
        assert!(!tool.requires_approval());
    }

    #[tokio::test]
    async fn default_execute_returns_error() {
        let tool = sample_tool();
        let result = tool
            .execute(
                "{}",
                json!({}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn simple_execute_receives_params() {
        let tool = FnTool::new("echo", "Echo", "Echo params.").with_execute_simple(
            |params, _cancel| async move {
                let msg = params["msg"].as_str().unwrap_or("none").to_owned();
                AgentToolResult::text(msg)
            },
        );

        let result = tool
            .execute(
                "id",
                json!({"msg": "hello"}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }

    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct TestParams {
        city: String,
    }

    #[test]
    fn with_schema_for_sets_schema() {
        let tool = sample_tool().with_schema_for::<TestParams>();
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("city"))
        );
    }

    #[test]
    fn approval_flag_is_configurable() {
        let tool = sample_tool().with_requires_approval(true);
        assert!(tool.requires_approval());
    }

    #[tokio::test]
    async fn full_execute_receives_all_args() {
        let tool =
            FnTool::new("full", "Full", "Full signature.").with_execute(
                |id, _params, _cancel, _on_update| async move {
                    AgentToolResult::text(format!("id={id}"))
                },
            );

        let result = tool
            .execute(
                "call_42",
                json!({}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(!result.is_error);
    }

    #[derive(Deserialize, JsonSchema)]
    struct TypedParams {
        city: String,
    }

    #[tokio::test]
    async fn typed_execute_deserializes_params_and_sets_schema() {
        let tool = FnTool::new("typed", "Typed", "Typed params.").with_execute_typed(
            |params: TypedParams, _cancel| async move { AgentToolResult::text(params.city) },
        );

        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("city"))
        );

        let result = tool
            .execute(
                "id",
                json!({"city": "Chicago"}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(ContentBlock::extract_text(&result.content), "Chicago");
    }

    #[tokio::test]
    async fn typed_execute_reports_deserialization_errors() {
        let tool = FnTool::new("typed", "Typed", "Typed params.").with_execute_typed(
            |params: TypedParams, _cancel| async move { AgentToolResult::text(params.city) },
        );

        let result = tool
            .execute(
                "id",
                json!({"city": 42}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(result.is_error);
        assert!(
            ContentBlock::extract_text(&result.content).contains("invalid parameters"),
            "expected invalid parameters error, got: {:?}",
            result.content
        );
    }
}
