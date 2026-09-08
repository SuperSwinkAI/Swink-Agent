//! Tests for `tool_middleware`.
#![cfg(test)]

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
                crate::tool::ToolMetadata::with_namespace("middleware-tests").with_version("1.0.0"),
            )
        }

        fn auth_config(&self) -> Option<crate::credential::AuthConfig> {
            Some(crate::credential::AuthConfig {
                credential_key: "weather-api".to_string(),
                auth_scheme: crate::credential::AuthScheme::ApiKeyHeader("X-Api-Key".to_string()),
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
        Some(crate::tool::ToolMetadata::with_namespace("middleware-tests").with_version("1.0.0"),)
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
