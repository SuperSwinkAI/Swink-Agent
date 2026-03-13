//! Middleware wrapper for [`AgentTool`] that intercepts `execute()` while
//! delegating all metadata methods to the inner tool.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use swink_agent::{AgentTool, AgentToolResult, BashTool, ToolMiddleware};
//!
//! let tool = Arc::new(BashTool::new());
//! let logged = ToolMiddleware::new(tool, |inner, id, params, cancel, on_update| {
//!     Box::pin(async move {
//!         println!("before");
//!         let result = inner.execute(&id, params, cancel, on_update).await;
//!         println!("after");
//!         result
//!     })
//! });
//!
//! assert_eq!(logged.name(), "bash");
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tool::{AgentTool, AgentToolResult};

// ─── Type alias for the middleware closure ──────────────────────────────────

type MiddlewareFn = Arc<
    dyn Fn(
            Arc<dyn AgentTool>,
            String,
            Value,
            CancellationToken,
            Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send>>
        + Send
        + Sync,
>;

// ─── ToolMiddleware ─────────────────────────────────────────────────────────

/// Intercepts [`execute()`](AgentTool::execute) on a wrapped [`AgentTool`].
///
/// All metadata methods (`name`, `label`, `description`, `parameters_schema`,
/// `requires_approval`) delegate to the inner tool.
pub struct ToolMiddleware {
    inner: Arc<dyn AgentTool>,
    middleware_fn: MiddlewareFn,
}

impl ToolMiddleware {
    /// Create a new middleware wrapping `inner`.
    ///
    /// The closure receives `(inner_tool, tool_call_id, params, cancel, on_update)`
    /// and can call through to the inner tool's `execute()` at any point.
    pub fn new<F>(inner: Arc<dyn AgentTool>, f: F) -> Self
    where
        F: Fn(
                Arc<dyn AgentTool>,
                String,
                Value,
                CancellationToken,
                Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send>>
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
        Self::new(inner, move |tool, id, params, cancel, on_update| {
            Box::pin(async move {
                tokio::select! {
                    result = tool.execute(&id, params, cancel.clone(), on_update) => result,
                    () = tokio::time::sleep(timeout) => {
                        cancel.cancel();
                        AgentToolResult::error(format!(
                            "tool timed out after {}ms",
                            timeout.as_millis()
                        ))
                    }
                }
            })
        })
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
        Self::new(inner, move |tool, id, params, cancel, on_update| {
            let cb = callback.clone();
            let name = tool.name().to_owned();
            Box::pin(async move {
                cb(&name, &id, true);
                let result = tool.execute(&id, params, cancel, on_update).await;
                cb(&name, &id, false);
                result
            })
        })
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

    fn requires_approval(&self) -> bool {
        self.inner.requires_approval()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        let inner = self.inner.clone();
        let id = tool_call_id.to_owned();
        let fut = (self.middleware_fn)(inner, id, params, cancellation_token, on_update);
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
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    use super::*;
    use crate::tool::AgentTool;

    /// Minimal tool for testing middleware.
    struct DummyTool;

    impl AgentTool for DummyTool {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn label(&self) -> &'static str {
            "Dummy"
        }
        fn description(&self) -> &'static str {
            "A dummy tool."
        }
        fn parameters_schema(&self) -> &Value {
            &Value::Null
        }
        fn requires_approval(&self) -> bool {
            true
        }
        fn execute(
            &self,
            _tool_call_id: &str,
            _params: Value,
            _cancellation_token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async { AgentToolResult::text("dummy result") })
        }
    }

    #[test]
    fn metadata_delegates_to_inner() {
        let inner: Arc<dyn AgentTool> = Arc::new(DummyTool);
        let mw = ToolMiddleware::new(inner, |tool, id, params, cancel, on_update| {
            Box::pin(async move { tool.execute(&id, params, cancel, on_update).await })
        });

        assert_eq!(mw.name(), "dummy");
        assert_eq!(mw.label(), "Dummy");
        assert_eq!(mw.description(), "A dummy tool.");
        assert!(mw.requires_approval());
    }

    #[tokio::test]
    async fn middleware_intercepts_execute() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let inner: Arc<dyn AgentTool> = Arc::new(DummyTool);
        let mw = ToolMiddleware::new(inner, move |tool, id, params, cancel, on_update| {
            let c = counter_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                tool.execute(&id, params, cancel, on_update).await
            })
        });

        let result = mw
            .execute("id", json!({}), CancellationToken::new(), None)
            .await;
        assert!(!result.is_error);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn call_through_returns_inner_result() {
        let inner: Arc<dyn AgentTool> = Arc::new(DummyTool);
        let mw = ToolMiddleware::new(inner, |tool, id, params, cancel, on_update| {
            Box::pin(async move { tool.execute(&id, params, cancel, on_update).await })
        });

        let result = mw
            .execute("id", json!({}), CancellationToken::new(), None)
            .await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn timeout_middleware_returns_error_on_slow_tool() {
        /// A tool that sleeps forever.
        struct SlowTool;
        impl AgentTool for SlowTool {
            fn name(&self) -> &'static str {
                "slow"
            }
            fn label(&self) -> &'static str {
                "Slow"
            }
            fn description(&self) -> &'static str {
                "Sleeps."
            }
            fn parameters_schema(&self) -> &Value {
                &Value::Null
            }
            fn execute(
                &self,
                _id: &str,
                _params: Value,
                cancel: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
                Box::pin(async move {
                    cancel.cancelled().await;
                    AgentToolResult::error("cancelled")
                })
            }
        }

        let inner: Arc<dyn AgentTool> = Arc::new(SlowTool);
        let mw = ToolMiddleware::with_timeout(inner, Duration::from_millis(10));

        let result = mw
            .execute("id", json!({}), CancellationToken::new(), None)
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn logging_middleware_calls_callback() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();

        let inner: Arc<dyn AgentTool> = Arc::new(DummyTool);
        let mw = ToolMiddleware::with_logging(inner, move |_name, _id, _is_start| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
        });

        mw.execute("id", json!({}), CancellationToken::new(), None)
            .await;

        // Should be called twice — once before, once after.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
