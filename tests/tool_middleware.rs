#![cfg(feature = "testkit")]
//! Integration test: run a ToolMiddleware-wrapped tool through the agent loop.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use common::{
    MockStreamFn, MockTool, default_convert, default_model, text_only_events, tool_call_events,
    user_msg,
};
use serde_json::{Value, json};

use swink_agent::{
    Agent, AgentOptions, AgentTool, AgentToolResult, AuthConfig, AuthScheme, CredentialFuture,
    CredentialResolver, CredentialType, DefaultRetryStrategy, ResolvedCredential, ToolFuture,
    ToolMetadata, ToolMiddleware,
};
use tokio_util::sync::CancellationToken;

fn auth_tool_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| json!({"type": "object"}))
}

#[derive(Default)]
struct StaticCredentialResolver;

impl CredentialResolver for StaticCredentialResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        let key = key.to_string();
        Box::pin(async move {
            if key == "weather-api" {
                Ok(ResolvedCredential::ApiKey("test-secret".to_string()))
            } else {
                Err(swink_agent::CredentialError::NotFound { key })
            }
        })
    }
}

struct AuthCapturingTool {
    seen_credentials: Arc<Mutex<Vec<Option<ResolvedCredential>>>>,
}

impl AuthCapturingTool {
    fn new() -> Self {
        Self {
            seen_credentials: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen_credentials(&self) -> Arc<Mutex<Vec<Option<ResolvedCredential>>>> {
        Arc::clone(&self.seen_credentials)
    }
}

impl AgentTool for AuthCapturingTool {
    fn name(&self) -> &str {
        "secure_echo"
    }

    fn label(&self) -> &str {
        "Secure Echo"
    }

    fn description(&self) -> &str {
        "Captures resolved credentials."
    }

    fn parameters_schema(&self) -> &Value {
        auth_tool_schema()
    }

    fn metadata(&self) -> Option<ToolMetadata> {
        Some(ToolMetadata::with_namespace("middleware-tests").with_version("1.0.0"))
    }

    fn auth_config(&self) -> Option<AuthConfig> {
        Some(AuthConfig {
            credential_key: "weather-api".to_string(),
            auth_scheme: AuthScheme::ApiKeyHeader("X-Api-Key".to_string()),
            credential_type: CredentialType::ApiKey,
        })
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        credential: Option<ResolvedCredential>,
    ) -> ToolFuture<'_> {
        self.seen_credentials.lock().unwrap().push(credential);
        Box::pin(async { AgentToolResult::text("secure result") })
    }
}

#[tokio::test]
async fn middleware_runs_in_agent_loop() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let inner = Arc::new(MockTool::new("echo"));
    let wrapped = ToolMiddleware::new(
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

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "echo", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![Arc::new(wrapped)])
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    assert!(!result.messages.is_empty());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "middleware should have been called once"
    );
}

#[tokio::test]
async fn middleware_preserves_metadata_and_auth_config_in_agent_loop() {
    let inner = Arc::new(AuthCapturingTool::new());
    let seen_credentials = inner.seen_credentials();
    let wrapped = ToolMiddleware::new(
        inner.clone(),
        |tool, id, params, cancel, on_update, state, credential| {
            Box::pin(async move {
                tool.execute(&id, params, cancel, on_update, state, credential)
                    .await
            })
        },
    );

    assert_eq!(
        wrapped.metadata(),
        Some(ToolMetadata::with_namespace("middleware-tests").with_version("1.0.0"))
    );

    let auth_config = wrapped
        .auth_config()
        .expect("wrapped tool should expose auth config");
    assert_eq!(auth_config.credential_key, "weather-api");
    assert!(matches!(
        auth_config.auth_scheme,
        AuthScheme::ApiKeyHeader(ref header) if header == "X-Api-Key"
    ));
    assert_eq!(auth_config.credential_type, CredentialType::ApiKey);

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "secure_echo", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![Arc::new(wrapped)])
            .with_credential_resolver(Arc::new(StaticCredentialResolver))
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    assert!(!result.messages.is_empty());
    let seen_credentials = seen_credentials.lock().unwrap();
    assert_eq!(seen_credentials.len(), 1);
    assert!(matches!(
        seen_credentials.first(),
        Some(Some(ResolvedCredential::ApiKey(secret))) if secret == "test-secret"
    ));
}
