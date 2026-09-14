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
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::SessionState;
use crate::credential::{AuthConfig, ResolvedCredential};
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
            Arc<RwLock<SessionState>>,
            Option<ResolvedCredential>,
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
    execution_root: Option<PathBuf>,
    execute_fn: ExecuteFn,
    approval_context_fn: Option<ApprovalContextFn>,
    auth_config: Option<AuthConfig>,
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
            execution_root: None,
            execute_fn: Arc::new(|_, _, _, _, _, _| {
                Box::pin(async { AgentToolResult::error("not implemented") })
            }),
            approval_context_fn: None,
            auth_config: None,
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

    /// Set the working directory used to resolve this tool's relative paths.
    #[must_use]
    pub fn with_execution_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.execution_root = Some(root.into());
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
        self.execute_fn = Arc::new(move |id, params, cancel, on_update, _state, _credential| {
            Box::pin(f(id, params, cancel, on_update))
        });
        self
    }

    /// Set the execution function using the complete [`AgentTool::execute`]
    /// context.
    ///
    /// The closure receives `(tool_call_id, params, cancellation_token,
    /// on_update, state, credential)`. `credential` is `Some` only when an
    /// [`AuthConfig`] was set via [`Self::with_auth_config`] and the framework
    /// resolved it.
    ///
    /// # Example
    ///
    /// ```
    /// use swink_agent::{
    ///     AgentTool, AgentToolResult, AuthConfig, AuthScheme, CredentialType, FnTool,
    ///     ResolvedCredential,
    /// };
    ///
    /// let tool = FnTool::new("call_api", "Call API", "Call an authenticated API.")
    ///     .with_auth_config(AuthConfig::new(
    ///         "my-api",
    ///         AuthScheme::BearerHeader,
    ///         CredentialType::Bearer,
    ///     ))
    ///     .with_execute_context(|_id, _params, _cancel, _on_update, state, credential| async move {
    ///         let calls: u64 = state.read().unwrap().get("calls").unwrap_or(0);
    ///         state.write().unwrap().set("calls", calls + 1).unwrap();
    ///         match credential {
    ///             Some(ResolvedCredential::Bearer(_token)) => AgentToolResult::text("ok"),
    ///             _ => AgentToolResult::error("missing bearer token"),
    ///         }
    ///     });
    ///
    /// assert!(tool.auth_config().is_some());
    /// ```
    #[must_use]
    pub fn with_execute_context<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(
                String,
                Value,
                CancellationToken,
                Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                Arc<RwLock<SessionState>>,
                Option<ResolvedCredential>,
            ) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static,
    {
        self.execute_fn = Arc::new(move |id, params, cancel, on_update, state, credential| {
            Box::pin(f(id, params, cancel, on_update, state, credential))
        });
        self
    }

    /// Declare the credential this tool needs resolved before execution.
    ///
    /// Read the resolved credential with [`Self::with_execute_context`].
    #[must_use]
    pub fn with_auth_config(mut self, config: AuthConfig) -> Self {
        self.auth_config = Some(config);
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
        self.execute_fn = Arc::new(
            move |_id, params, cancel, _on_update, _state, _credential| Box::pin(f(params, cancel)),
        );
        self
    }

    /// Set the execution function using an explicit untyped async signature.
    ///
    /// This is equivalent to [`Self::with_execute_simple`] and exists as a
    /// discoverability alias for callers looking for an untyped async builder.
    #[must_use]
    pub fn with_execute_async<F, Fut>(self, f: F) -> Self
    where
        F: Fn(Value, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static,
    {
        self.with_execute_simple(f)
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
        self.execute_fn = Arc::new(
            move |_id, params, cancel, _on_update, _state, _credential| {
                let parsed: T = match serde_json::from_value(params) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        return Box::pin(async move {
                            AgentToolResult::error(format!("invalid parameters: {err}"))
                        });
                    }
                };
                Box::pin(f(parsed, cancel))
            },
        );
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

    fn execution_root(&self) -> Option<&Path> {
        self.execution_root.as_deref()
    }

    fn approval_context(&self, params: &Value) -> Option<Value> {
        self.approval_context_fn.as_ref().and_then(|f| f(params))
    }

    fn auth_config(&self) -> Option<AuthConfig> {
        self.auth_config.clone()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<RwLock<SessionState>>,
        credential: Option<ResolvedCredential>,
    ) -> ToolFuture<'_> {
        (self.execute_fn)(
            tool_call_id.to_owned(),
            params,
            cancellation_token,
            on_update,
            state,
            credential,
        )
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
#[path = "fn_tool_tests.rs"]
mod tests;
