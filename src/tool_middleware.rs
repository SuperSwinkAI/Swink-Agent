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
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    use super::*;
    use crate::FnTool;
    use crate::tool::AgentTool;

    fn dummy_tool() -> Arc<dyn AgentTool> {
        Arc::new(
            FnTool::new("dummy", "Dummy", "A dummy tool.")
                .with_requires_approval(true)
                .with_execute_simple(|_params, _cancel| async {
                    AgentToolResult::text("dummy result")
                }),
        )
    }

    #[test]
    fn metadata_and_auth_config_delegate_to_inner() {
        struct MetadataAuthTool;

        impl AgentTool for MetadataAuthTool {
            fn name(&self) -> &'static str {
                "auth_tool"
            }

            fn label(&self) -> &'static str {
                "Auth Tool"
            }

            fn description(&self) -> &'static str {
                "A tool with metadata and auth config."
            }

            fn parameters_schema(&self) -> &Value {
                &Value::Null
            }

            fn metadata(&self) -> Option<crate::tool::ToolMetadata> {
                Some(
                    crate::tool::ToolMetadata::with_namespace("middleware-tests")
                        .with_version("1.0.0"),
                )
            }

            fn auth_config(&self) -> Option<crate::credential::AuthConfig> {
                Some(crate::credential::AuthConfig {
                    credential_key: "weather-api".to_string(),
                    auth_scheme: crate::credential::AuthScheme::ApiKeyHeader(
                        "X-Api-Key".to_string(),
                    ),
                    credential_type: crate::credential::CredentialType::ApiKey,
                })
            }

            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _cancellation_token: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let inner: Arc<dyn AgentTool> = Arc::new(MetadataAuthTool);
        let mw = ToolMiddleware::new(
            inner,
            |tool, id, params, cancel, on_update, state, credential| {
                Box::pin(async move {
                    tool.execute(&id, params, cancel, on_update, state, credential)
                        .await
                })
            },
        );

        assert_eq!(mw.name(), "auth_tool");
        assert_eq!(mw.label(), "Auth Tool");
        assert_eq!(mw.description(), "A tool with metadata and auth config.");
        assert!(!mw.requires_approval());
        assert_eq!(
            mw.metadata(),
            Some(
                crate::tool::ToolMetadata::with_namespace("middleware-tests").with_version("1.0.0"),
            )
        );

        let auth_config = mw
            .auth_config()
            .expect("middleware should delegate auth config");
        assert_eq!(auth_config.credential_key, "weather-api");
        assert!(matches!(
            auth_config.auth_scheme,
            crate::credential::AuthScheme::ApiKeyHeader(ref header) if header == "X-Api-Key"
        ));
        assert_eq!(
            auth_config.credential_type,
            crate::credential::CredentialType::ApiKey
        );
    }

    fn test_state() -> std::sync::Arc<std::sync::RwLock<crate::SessionState>> {
        std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::new()))
    }

    #[tokio::test]
    async fn middleware_intercepts_execute() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let inner: Arc<dyn AgentTool> = dummy_tool();
        let mw = ToolMiddleware::new(
            inner,
            move |tool, id, params, cancel, on_update, state, credential| {
                let c = counter_clone.clone();
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    tool.execute(&id, params, cancel, on_update, state, credential)
                        .await
                })
            },
        );

        let result = mw
            .execute(
                "id",
                json!({}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn call_through_returns_inner_result() {
        let inner: Arc<dyn AgentTool> = dummy_tool();
        let mw = ToolMiddleware::new(
            inner,
            |tool, id, params, cancel, on_update, state, credential| {
                Box::pin(async move {
                    tool.execute(&id, params, cancel, on_update, state, credential)
                        .await
                })
            },
        );

        let result = mw
            .execute(
                "id",
                json!({}),
                CancellationToken::new(),
                None,
                test_state(),
                None,
            )
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
                _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async move {
                    cancel.cancelled().await;
                    AgentToolResult::error("cancelled")
                })
            }
        }

        let inner: Arc<dyn AgentTool> = Arc::new(SlowTool);
        let mw = ToolMiddleware::with_timeout(inner, Duration::from_millis(10));

        let result = mw
            .execute(
                "id",
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
    async fn logging_middleware_calls_callback() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();

        let inner: Arc<dyn AgentTool> = dummy_tool();
        let mw = ToolMiddleware::with_logging(inner, move |_name, _id, _is_start| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
        });

        mw.execute(
            "id",
            json!({}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;

        // Should be called twice — once before, once after.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
