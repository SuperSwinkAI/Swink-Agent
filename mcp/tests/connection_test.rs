//! Connection tests for MCP integration (T010, T013).

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use swink_agent::{
    ContentBlock, CredentialFuture, CredentialResolver, CredentialType, ResolvedCredential,
};
use tokio::sync::RwLock;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use swink_agent_mcp::{McpConnection, McpServerConfig, McpTransport, SseBearerAuth};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedHeaders {
    authorization: Option<String>,
    api_key: Option<String>,
    trace_id: Option<String>,
}

#[derive(Clone)]
struct HeaderCaptureState {
    sender: Arc<Mutex<Option<oneshot::Sender<CapturedHeaders>>>>,
}

struct StaticCredentialResolver {
    expected_key: String,
    credential: ResolvedCredential,
}

impl StaticCredentialResolver {
    fn new(expected_key: impl Into<String>, credential: ResolvedCredential) -> Self {
        Self {
            expected_key: expected_key.into(),
            credential,
        }
    }
}

impl CredentialResolver for StaticCredentialResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        let expected_key = self.expected_key.clone();
        let actual_key = key.to_string();
        let credential = self.credential.clone();
        Box::pin(async move {
            assert_eq!(actual_key, expected_key);
            Ok(credential)
        })
    }
}

struct RotatingCredentialResolver {
    expected_key: String,
    current_token: Arc<RwLock<String>>,
    calls: Arc<AtomicUsize>,
}

impl RotatingCredentialResolver {
    fn new(expected_key: impl Into<String>, current_token: Arc<RwLock<String>>) -> Self {
        Self {
            expected_key: expected_key.into(),
            current_token,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CredentialResolver for RotatingCredentialResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        let expected_key = self.expected_key.clone();
        let actual_key = key.to_string();
        let current_token = Arc::clone(&self.current_token);
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            assert_eq!(actual_key, expected_key);
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(ResolvedCredential::Bearer(
                current_token.read().await.clone(),
            ))
        })
    }
}

#[derive(Clone)]
struct AuthGateState {
    current_token: Arc<RwLock<String>>,
    seen_authorization: Arc<Mutex<Vec<String>>>,
}

async fn require_bearer_auth(
    State(state): State<AuthGateState>,
    request: Request,
    next: Next,
) -> Response {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if let Some(ref header) = authorization {
        state
            .seen_authorization
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(header.clone());
    }

    let expected = format!("Bearer {}", state.current_token.read().await.as_str());
    if authorization.as_deref() != Some(expected.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    next.run(request).await
}

async fn current_session_id(session_manager: &Arc<LocalSessionManager>) -> String {
    let sessions = session_manager.sessions.read().await;
    sessions
        .keys()
        .next()
        .map(std::string::ToString::to_string)
        .expect("session should exist")
}

async fn capture_headers(
    State(state): State<HeaderCaptureState>,
    headers: HeaderMap,
) -> StatusCode {
    let captured = CapturedHeaders {
        authorization: headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        api_key: headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        trace_id: headers
            .get("x-trace-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    };

    let sender = state
        .sender
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(sender) = sender {
        let _ = sender.send(captured);
    }

    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(windows)]
fn sleep_command(seconds: u64) -> (String, Vec<String>) {
    let command = std::env::var("SystemRoot").map_or_else(
        |_| "C:\\Windows\\System32\\choice.exe".into(),
        |root| format!("{root}\\System32\\choice.exe"),
    );
    (
        command,
        vec![
            "/T".into(),
            seconds.to_string(),
            "/D".into(),
            "Y".into(),
            "/N".into(),
        ],
    )
}

#[cfg(not(windows))]
fn sleep_command(seconds: u64) -> (String, Vec<String>) {
    ("sh".into(), vec!["-c".into(), format!("sleep {seconds}")])
}

/// T010: Connect to mock stdio MCP server, verify connection succeeds
/// and tools are discovered.
///
/// We cannot use a real stdio subprocess in unit tests without an external
/// binary, so we test via the in-process duplex transport helper to verify
/// tool discovery works, and test the `McpConnection` API for error cases.
#[tokio::test]
async fn connect_discovers_tools_via_in_process_server() {
    let config = common::MockServerConfig::new(vec![
        common::MockToolDef::simple("search_files", "found: main.rs"),
        common::MockToolDef::simple("read_file", "contents of file"),
    ]);

    let client = common::spawn_mock_server_with_client(&config).await;

    // Verify tool discovery works via the rmcp peer API.
    let tools = client.peer().list_all_tools().await.unwrap();
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        tool_names,
        vec!["read_file".to_string(), "search_files".to_string()],
        "tool discovery should reflect the configured mock tool set"
    );
}

/// T013: Attempt connection to non-existent server, verify graceful error
/// with `McpError::SpawnFailed`.
#[tokio::test]
async fn connect_to_nonexistent_server_returns_spawn_failed() {
    let config = McpServerConfig {
        name: "nonexistent".into(),
        transport: McpTransport::Stdio {
            command: "/tmp/definitely-not-a-real-mcp-server-binary-xyz".into(),
            args: vec![],
            env: std::collections::HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(
        result.is_err(),
        "should fail to connect to nonexistent server"
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("nonexistent"),
        "error should mention server name, got: {err_msg}"
    );
}

#[tokio::test]
async fn connect_timeout_returns_connection_failed() {
    let (command, args) = sleep_command(5);
    let config = McpServerConfig {
        name: "sleepy-stdio".into(),
        transport: McpTransport::Stdio {
            command,
            args,
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: Some(50),
        discovery_timeout_ms: None,
    };

    let start = Instant::now();
    let result = McpConnection::connect(config, None).await;
    let elapsed = start.elapsed();

    let error = result.expect_err("sleeping server should time out");
    let message = error.to_string();
    assert!(
        message.contains("timed out"),
        "timeout error should mention timeout, got: {message}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "connect timeout should fail quickly, elapsed: {elapsed:?}"
    );
}

/// T035: Verify `from_service` connects and discovers tools (exercises the
/// same code path as SSE/HTTP transport, minus the network layer).
///
/// The original test used `rmcp::transport::sse_server::SseServer` which was
/// removed in rmcp 1.x. The in-process duplex transport exercises identical
/// tool-discovery logic.
#[tokio::test]
async fn from_service_discovers_tools() {
    let conn = common::spawn_mock_connection("http-test-server", None, vec![]).await;

    assert_eq!(
        conn.status(),
        swink_agent_mcp::McpConnectionStatus::Connected
    );
    assert!(
        !conn.discovered_tools.is_empty(),
        "should discover tools from mock server"
    );
    let names: Vec<_> = conn
        .discovered_tools
        .iter()
        .map(|t| t.name.as_ref())
        .collect();
    assert!(
        names.contains(&"echo"),
        "should discover echo tool, got: {names:?}"
    );
}

/// T036: Verify `connect` to a non-existent HTTP URL returns a connection error
/// (exercises the HTTP streaming code path with an unreachable endpoint).
#[tokio::test]
async fn connect_sse_to_unreachable_url_returns_error() {
    let config = McpServerConfig {
        name: "sse-unreachable".into(),
        transport: McpTransport::Sse {
            url: "http://127.0.0.1:1/mcp".into(),
            bearer_token: Some("test-bearer-token-123".into()),
            bearer_auth: None,
            headers: HashMap::new(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(result.is_err(), "connecting to unreachable URL should fail");
}

#[tokio::test]
async fn connect_sse_sends_bearer_and_custom_headers() {
    let (sender, receiver) = oneshot::channel();
    let state = HeaderCaptureState {
        sender: Arc::new(Mutex::new(Some(sender))),
    };

    let app = Router::new()
        .route("/mcp", post(capture_headers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("listener address");

    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = McpServerConfig {
        name: "sse-header-test".into(),
        transport: McpTransport::Sse {
            url: format!("http://{address}/mcp"),
            bearer_token: Some("test-bearer-token-123".into()),
            bearer_auth: None,
            headers: HashMap::from([
                ("x-api-key".into(), "custom-key-456".into()),
                ("x-trace-id".into(), "trace-789".into()),
            ]),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(
        result.is_err(),
        "the mock HTTP server should reject the handshake"
    );

    let captured = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("timed out waiting for header capture")
        .expect("header capture channel closed");

    assert_eq!(
        captured.authorization.as_deref(),
        Some("Bearer test-bearer-token-123")
    );
    assert_eq!(captured.api_key.as_deref(), Some("custom-key-456"));
    assert_eq!(captured.trace_id.as_deref(), Some("trace-789"));

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn connect_sse_uses_resolved_bearer_auth_over_static_token() {
    let (sender, receiver) = oneshot::channel();
    let state = HeaderCaptureState {
        sender: Arc::new(Mutex::new(Some(sender))),
    };

    let app = Router::new()
        .route("/mcp", post(capture_headers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("listener address");

    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = McpServerConfig {
        name: "sse-resolver-auth".into(),
        transport: McpTransport::Sse {
            url: format!("http://{address}/mcp"),
            bearer_token: Some("static-token".into()),
            bearer_auth: Some(SseBearerAuth {
                credential_key: "mcp-sse-token".into(),
                credential_type: CredentialType::ApiKey,
            }),
            headers: HashMap::new(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let resolver = Arc::new(StaticCredentialResolver::new(
        "mcp-sse-token",
        ResolvedCredential::ApiKey("resolved-token-789".into()),
    ));

    let result = McpConnection::connect_with_resolver(config, Some(resolver), None).await;
    assert!(
        result.is_err(),
        "the mock HTTP server should reject the handshake"
    );

    let captured = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("timed out waiting for header capture")
        .expect("header capture channel closed");

    assert_eq!(
        captured.authorization.as_deref(),
        Some("Bearer resolved-token-789")
    );

    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn connect_sse_with_resolver_auth_requires_resolver() {
    let config = McpServerConfig {
        name: "sse-missing-resolver".into(),
        transport: McpTransport::Sse {
            url: "http://127.0.0.1:1/mcp".into(),
            bearer_token: None,
            bearer_auth: Some(SseBearerAuth {
                credential_key: "mcp-sse-token".into(),
                credential_type: CredentialType::Bearer,
            }),
            headers: HashMap::new(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(result.is_err(), "missing resolver should fail fast");
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("mcp-sse-token"),
        "error should mention the missing credential key, got: {error}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sse_resolver_auth_refreshes_during_session_recovery() {
    let session_manager = Arc::new(LocalSessionManager::default());
    let shutdown = CancellationToken::new();
    let current_token = Arc::new(RwLock::new("initial-token".to_string()));
    let auth_gate = AuthGateState {
        current_token: Arc::clone(&current_token),
        seen_authorization: Arc::new(Mutex::new(Vec::new())),
    };

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let server = common::MockMcpServer::from_config(&mock_cfg);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::clone(&session_manager),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(shutdown.child_token()),
    );

    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(auth_gate.clone(), require_bearer_auth));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("listener address");

    let server_task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
        }
    });

    let resolver = Arc::new(RotatingCredentialResolver::new(
        "mcp-sse-token",
        Arc::clone(&current_token),
    ));
    let config = McpServerConfig {
        name: "sse-refresh-auth".into(),
        transport: McpTransport::Sse {
            url: format!("http://{address}/mcp"),
            bearer_token: Some("stale-static-token".into()),
            bearer_auth: Some(SseBearerAuth {
                credential_key: "mcp-sse-token".into(),
                credential_type: CredentialType::Bearer,
            }),
            headers: HashMap::new(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };

    let conn = McpConnection::connect_with_resolver(config, Some(resolver.clone()), None)
        .await
        .expect("SSE connection should succeed");

    let original_session_id = current_session_id(&session_manager).await;
    *current_token.write().await = "rotated-token".to_string();
    session_manager.sessions.write().await.clear();

    let result = conn
        .call_tool("echo", serde_json::json!({ "text": "recovered" }))
        .await
        .expect("tool call should recover with the refreshed bearer token");
    let agent_result = swink_agent_mcp::convert::call_result_to_agent_result(&result);
    let text = ContentBlock::extract_text(&agent_result.content);
    assert!(
        text.contains("recovered"),
        "recovered call should still reach the server, got: {text}"
    );

    let replacement_session_id = current_session_id(&session_manager).await;
    assert_ne!(
        replacement_session_id, original_session_id,
        "session recovery should create a new server session"
    );

    let seen_authorization = auth_gate
        .seen_authorization
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        seen_authorization
            .iter()
            .any(|header| header == "Bearer initial-token"),
        "initial handshake should use the resolver token, got: {seen_authorization:?}"
    );
    assert!(
        seen_authorization
            .iter()
            .any(|header| header == "Bearer rotated-token"),
        "session recovery should use the rotated resolver token, got: {seen_authorization:?}"
    );
    assert!(
        !seen_authorization
            .iter()
            .any(|header| header == "Bearer stale-static-token"),
        "static bearer fallback should not override resolver auth, got: {seen_authorization:?}"
    );
    assert!(
        resolver.call_count() >= 2,
        "resolver should be consulted again for session recovery"
    );

    conn.shutdown().await;
    shutdown.cancel();
    let _ = server_task.await;
}
