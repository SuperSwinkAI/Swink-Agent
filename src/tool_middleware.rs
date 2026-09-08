//! Middleware wrapper for [`AgentTool`] that intercepts `execute()` while
//! delegating all metadata methods to the inner tool.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(feature = "builtin-tools")]
//! # {
//! use std::sync::Arc;
//! use swink_agent::{AgentTool, AgentToolResult, BashTool, ToolMiddleware};
//!
//! let tool = Arc::new(BashTool::new());
//! let logged = ToolMiddleware::new(tool, |inner, id, params, cancel, on_update, state, credential| {
//!     Box::pin(async move {
//!         println!("before");
//!         let result = inner.execute(&id, params, cancel, on_update, state, credential).await;
//!         println!("after");
//!         result
//!     })
//! });
//!
//! assert_eq!(logged.name(), "bash");
//! # }
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult, ToolFuture};

// ─── Type alias for the middleware closure ──────────────────────────────────

type MiddlewareFn = Arc<
    dyn Fn(
            Arc<dyn AgentTool>,
            String,
            Value,
            CancellationToken,
            Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
            Option<crate::credential::ResolvedCredential>,
        ) -> ToolFuture<'static>
        + Send
        + Sync,
>;

// ─── ToolMiddleware ─────────────────────────────────────────────────────────

/// Intercepts [`execute()`](AgentTool::execute) on a wrapped [`AgentTool`].
///
/// All descriptor methods (`name`, `label`, `description`,
/// `parameters_schema`, `metadata`, `requires_approval`, `auth_config`)
/// delegate to the inner tool.
pub struct ToolMiddleware {
    inner: Arc<dyn AgentTool>,
    middleware_fn: MiddlewareFn,
}

impl ToolMiddleware {
    /// Create a new middleware wrapping `inner`.
    ///
    /// The closure receives `(inner_tool, tool_call_id, params, cancel, on_update, state, credential)`
    /// and can call through to the inner tool's `execute()` at any point.
    pub fn new<F>(inner: Arc<dyn AgentTool>, f: F) -> Self
    where
        F: Fn(
                Arc<dyn AgentTool>,
                String,
                Value,
                CancellationToken,
                Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
                Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'static>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner,
            middleware_fn: Arc::new(f),
        }
    }

    /// Create a middleware that enforces a timeout on tool execution.
    ///
    /// If the inner tool does not complete within `timeout`, an error result
    /// is returned.
    pub fn with_timeout(inner: Arc<dyn AgentTool>, timeout: Duration) -> Self {
        Self::new(
            inner,
            move |tool, id, params, cancel, on_update, state, credential| {
                Box::pin(async move {
                    tokio::select! {
                        result = tool.execute(&id, params, cancel.clone(), on_update, state, credential) => result,
                        () = tokio::time::sleep(timeout) => {
                            cancel.cancel();
                            AgentToolResult::error(format!(
                                "tool timed out after {}ms",
                                timeout.as_millis()
                            ))
                        }
                    }
                })
            },
        )
    }

    /// Create a middleware that calls a logging callback before and after
    /// tool execution.
    ///
    /// The callback receives `(tool_name, tool_call_id, is_start)` where
    /// `is_start` is `true` before execution and `false` after.
    pub fn with_logging<F>(inner: Arc<dyn AgentTool>, callback: F) -> Self
    where
        F: Fn(&str, &str, bool) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        Self::new(
            inner,
            move |tool, id, params, cancel, on_update, state, credential| {
                let cb = callback.clone();
                let name = tool.name().to_owned();
                Box::pin(async move {
                    cb(&name, &id, true);
                    let result = tool
                        .execute(&id, params, cancel, on_update, state, credential)
                        .await;
                    cb(&name, &id, false);
                    result
                })
            },
        )
    }
}

impl AgentTool for ToolMiddleware {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> &Value {
        self.inner.parameters_schema()
    }

    fn metadata(&self) -> Option<crate::tool::ToolMetadata> {
        self.inner.metadata()
    }

    fn execution_root(&self) -> Option<&Path> {
        self.inner.execution_root()
    }

    fn requires_approval(&self) -> bool {
        self.inner.requires_approval()
    }

    fn approval_context(&self, params: &Value) -> Option<Value> {
        self.inner.approval_context(params)
    }

    fn auth_config(&self) -> Option<crate::credential::AuthConfig> {
        self.inner.auth_config()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let inner = self.inner.clone();
        let id = tool_call_id.to_owned();
        let fut = (self.middleware_fn)(
            inner,
            id,
            params,
            cancellation_token,
            on_update,
            state,
            credential,
        );
        Box::pin(fut)
    }
}

impl std::fmt::Debug for ToolMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolMiddleware")
            .field("inner_name", &self.inner.name())
            .finish_non_exhaustive()
    }
}

// ─── Compile-time Send + Sync assertion ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolMiddleware>();
};

#[cfg(test)]
#[path = "tool_middleware_tests.rs"]
mod tests;
