//! MCP server connection management.
//!
//! Wraps an `rmcp` client session, handling tool discovery and providing
//! access to the peer for tool call forwarding. A background monitor task
//! awaits the service lifecycle and emits a disconnect event when the
//! underlying service stops running.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::stream::BoxStream;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo, Implementation};
use rmcp::service::{Peer, QuitReason, RoleClient, RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::Value;
use sse_stream::{Error as SseError, Sse};
use swink_agent::{
    AgentEvent, CredentialError, CredentialResolver, CredentialType, ResolvedCredential,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::{McpServerConfig, McpTransport, SseBearerAuth};
use crate::error::McpError;

/// Status of an MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectionStatus {
    /// Connected and ready to serve tool calls.
    Connected,
    /// Disconnected; tool calls will fail immediately.
    Disconnected,
}

struct McpConnectionState {
    status: McpConnectionStatus,
    peer: Option<Peer<RoleClient>>,
    monitor: Option<JoinHandle<()>>,
}

#[derive(Debug, thiserror::Error)]
enum SseBearerResolutionError {
    #[error("credential resolution timed out for {key}")]
    Timeout { key: String },
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error("credential type mismatch for {key}: expected {expected:?}, got {actual:?}")]
    TypeMismatch {
        key: String,
        expected: CredentialType,
        actual: CredentialType,
    },
}

#[derive(Debug, thiserror::Error)]
enum ResolverBackedSseHttpClientError {
    #[error(transparent)]
    Credential(#[from] SseBearerResolutionError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
struct ResolverBackedSseHttpClient {
    inner: reqwest::Client,
    bearer_auth: SseBearerAuth,
    credential_resolver: Arc<dyn CredentialResolver>,
}

impl ResolverBackedSseHttpClient {
    fn new(bearer_auth: SseBearerAuth, credential_resolver: Arc<dyn CredentialResolver>) -> Self {
        Self {
            inner: reqwest::Client::default(),
            bearer_auth,
            credential_resolver,
        }
    }

    async fn resolve_bearer_token(&self) -> Result<String, ResolverBackedSseHttpClientError> {
        resolve_sse_bearer_secret(&self.bearer_auth, self.credential_resolver.as_ref())
            .await
            .map_err(ResolverBackedSseHttpClientError::from)
    }
}

impl StreamableHttpClient for ResolverBackedSseHttpClient {
    type Error = ResolverBackedSseHttpClientError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: rmcp::model::ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let bearer_token = self
            .resolve_bearer_token()
            .await
            .map_err(StreamableHttpError::Client)?;
        <reqwest::Client as StreamableHttpClient>::post_message(
            &self.inner,
            uri,
            message,
            session_id,
            Some(bearer_token),
            custom_headers,
        )
        .await
        .map_err(map_reqwest_streamable_http_error)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let bearer_token = self
            .resolve_bearer_token()
            .await
            .map_err(StreamableHttpError::Client)?;
        <reqwest::Client as StreamableHttpClient>::delete_session(
            &self.inner,
            uri,
            session_id,
            Some(bearer_token),
            custom_headers,
        )
        .await
        .map_err(map_reqwest_streamable_http_error)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let bearer_token = self
            .resolve_bearer_token()
            .await
            .map_err(StreamableHttpError::Client)?;
        <reqwest::Client as StreamableHttpClient>::get_stream(
            &self.inner,
            uri,
            session_id,
            last_event_id,
            Some(bearer_token),
            custom_headers,
        )
        .await
        .map_err(map_reqwest_streamable_http_error)
    }
}

/// A connection to a single MCP server.
///
/// Holds a cloned `Peer<RoleClient>` for making tool calls and a background
/// monitor task that awaits the service lifecycle. When the remote transport
/// closes, the monitor transitions the shared state to `Disconnected` and
/// sends an `AgentEvent::McpServerDisconnected` on the optional event channel.
///
/// Created via [`McpConnection::connect`] or [`McpConnection::from_service`].
pub struct McpConnection {
    /// The server configuration used to establish this connection.
    pub config: McpServerConfig,
    /// Discovered tools from the server (raw rmcp tool definitions).
    pub discovered_tools: Vec<rmcp::model::Tool>,
    /// Shared connection state used by callers, shutdown, and the monitor task.
    state: Arc<Mutex<McpConnectionState>>,
    /// Optional event channel for emitting MCP lifecycle events such as
    /// `McpToolCallStarted` / `McpToolCallCompleted`. Connect, discovery, and
    /// disconnect events are emitted through this sender during
    /// [`connect`](Self::connect) / [`from_service`](Self::from_service) and
    /// the background monitor task respectively.
    event_tx: Option<UnboundedSender<AgentEvent>>,
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnection")
            .field("config", &self.config)
            .field("discovered_tools", &self.discovered_tools.len())
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl McpConnection {
    /// Returns the current connection status.
    pub fn status(&self) -> McpConnectionStatus {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status
    }

    /// Create a disconnected connection placeholder.
    ///
    /// Useful for tests that only inspect tool metadata without establishing
    /// a real connection. Calls to `call_tool()` will fail immediately.
    pub fn disconnected(config: McpServerConfig) -> Self {
        Self {
            config,
            discovered_tools: Vec::new(),
            state: Arc::new(Mutex::new(McpConnectionState {
                status: McpConnectionStatus::Disconnected,
                peer: None,
                monitor: None,
            })),
            event_tx: None,
        }
    }

    /// Shared reference to the optional event sender.
    ///
    /// Used by tool wrappers to emit `McpToolCallStarted` /
    /// `McpToolCallCompleted` around forwarded calls.
    pub(crate) const fn event_tx(&self) -> Option<&UnboundedSender<AgentEvent>> {
        self.event_tx.as_ref()
    }

    /// Create a connection from a pre-established rmcp service.
    ///
    /// Performs tool discovery on the already-connected service and spawns the
    /// background lifecycle monitor. Useful for testing with in-process mock
    /// servers or when the transport is managed externally.
    pub async fn from_service(
        config: McpServerConfig,
        service: RunningService<RoleClient, ClientInfo>,
        event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Result<Self, McpError> {
        let peer = service.peer().clone();

        // Handshake already completed before we were given the service.
        emit_event(event_tx.as_ref(), || {
            crate::event::server_connected(&config.name)
        });

        let discovered_tools =
            peer.list_all_tools()
                .await
                .map_err(|e| McpError::ConnectionFailed {
                    server: config.name.clone(),
                    reason: format!("tool discovery failed: {e}"),
                })?;

        info!(
            server = %config.name,
            tool_count = discovered_tools.len(),
            "MCP server connected via provided service, tools discovered"
        );

        emit_event(event_tx.as_ref(), || {
            crate::event::tools_discovered(&config.name, discovered_tools.len())
        });

        let state = Arc::new(Mutex::new(McpConnectionState {
            status: McpConnectionStatus::Connected,
            peer: Some(peer),
            monitor: None,
        }));
        let monitor = spawn_monitor(
            service,
            Arc::clone(&state),
            config.name.clone(),
            event_tx.clone(),
        );
        state.lock().unwrap_or_else(PoisonError::into_inner).monitor = Some(monitor);

        Ok(Self {
            config,
            discovered_tools,
            state,
            event_tx,
        })
    }

    /// Connect to an MCP server using the configured transport.
    ///
    /// Supports stdio and SSE (HTTP) transports. Spawns a background lifecycle
    /// monitor that sends `AgentEvent::McpServerDisconnected` on `event_tx`
    /// when the underlying service terminates.
    ///
    /// For SSE, transient stream drops and stale-session recovery are handled
    /// by rmcp's streamable HTTP transport. This wrapper only transitions to
    /// [`McpConnectionStatus::Disconnected`] once rmcp has given up and the
    /// service itself exits.
    pub async fn connect(
        config: McpServerConfig,
        event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Result<Self, McpError> {
        Self::connect_with_resolver(config, None, event_tx).await
    }

    /// Connect to an MCP server using the configured transport and optional
    /// credential resolver for SSE bearer auth.
    pub async fn connect_with_resolver(
        config: McpServerConfig,
        credential_resolver: Option<Arc<dyn CredentialResolver>>,
        event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Result<Self, McpError> {
        let service = match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                Self::connect_stdio(command, args, env, &config.name).await?
            }
            McpTransport::Sse {
                url,
                bearer_token,
                bearer_auth,
                headers,
            } => match bearer_auth.as_ref() {
                Some(bearer_auth) => {
                    let credential_resolver =
                        credential_resolver.clone().ok_or_else(|| McpError::ConnectionFailed {
                            server: config.name.clone(),
                            reason: format!(
                                "SSE bearer auth for credential `{}` requires a credential resolver",
                                bearer_auth.credential_key
                            ),
                        })?;
                    Self::connect_sse_with_resolver(
                        url,
                        bearer_auth,
                        credential_resolver,
                        headers,
                        &config.name,
                    )
                    .await?
                }
                None => {
                    Self::connect_sse(url, bearer_token.as_deref(), headers, &config.name).await?
                }
            },
        };

        // Handshake succeeded, transport is live.
        emit_event(event_tx.as_ref(), || {
            crate::event::server_connected(&config.name)
        });

        let peer = service.peer().clone();

        // Discover tools from the server.
        let discovered_tools =
            peer.list_all_tools()
                .await
                .map_err(|e| McpError::ConnectionFailed {
                    server: config.name.clone(),
                    reason: format!("tool discovery failed: {e}"),
                })?;

        info!(
            server = %config.name,
            tool_count = discovered_tools.len(),
            "MCP server connected, tools discovered"
        );

        emit_event(event_tx.as_ref(), || {
            crate::event::tools_discovered(&config.name, discovered_tools.len())
        });

        let state = Arc::new(Mutex::new(McpConnectionState {
            status: McpConnectionStatus::Connected,
            peer: Some(peer),
            monitor: None,
        }));
        let monitor = spawn_monitor(
            service,
            Arc::clone(&state),
            config.name.clone(),
            event_tx.clone(),
        );
        state.lock().unwrap_or_else(PoisonError::into_inner).monitor = Some(monitor);

        Ok(Self {
            config,
            discovered_tools,
            state,
            event_tx,
        })
    }

    /// Connect to a stdio-based MCP server subprocess.
    async fn connect_stdio(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        server_name: &str,
    ) -> Result<RunningService<RoleClient, ClientInfo>, McpError> {
        let cmd = build_stdio_command(command, args, env);

        let transport = TokioChildProcess::new(cmd).map_err(|e| McpError::SpawnFailed {
            server: server_name.to_string(),
            source: e,
        })?;

        let client_info = client_info();

        let service =
            client_info
                .serve(transport)
                .await
                .map_err(|e| McpError::ConnectionFailed {
                    server: server_name.to_string(),
                    reason: format!("connection handshake failed: {e}"),
                })?;

        Ok(service)
    }

    /// Connect to a remote MCP server via HTTP streaming transport.
    async fn connect_sse(
        url: &str,
        bearer_token: Option<&str>,
        headers: &HashMap<String, String>,
        server_name: &str,
    ) -> Result<RunningService<RoleClient, ClientInfo>, McpError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        if let Some(token) = bearer_token {
            config = config.auth_header(token.to_owned());
        }
        if !headers.is_empty() {
            config = config.custom_headers(parse_custom_headers(headers, server_name)?);
        }

        let transport = StreamableHttpClientTransport::from_config(config);

        client_info()
            .serve(transport)
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                reason: format!("HTTP streaming handshake failed: {e}"),
            })
    }

    /// Connect to a remote MCP server via HTTP streaming transport, resolving
    /// bearer auth on every HTTP request so rmcp reconnect/reinit paths can
    /// pick up rotated credentials.
    async fn connect_sse_with_resolver(
        url: &str,
        bearer_auth: &SseBearerAuth,
        credential_resolver: Arc<dyn CredentialResolver>,
        headers: &HashMap<String, String>,
        server_name: &str,
    ) -> Result<RunningService<RoleClient, ClientInfo>, McpError> {
        resolve_sse_bearer_secret(bearer_auth, credential_resolver.as_ref())
            .await
            .map_err(|error| sse_bearer_resolution_error(error, server_name))?;

        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        if !headers.is_empty() {
            config = config.custom_headers(parse_custom_headers(headers, server_name)?);
        }

        let transport = StreamableHttpClientTransport::with_client(
            ResolverBackedSseHttpClient::new(bearer_auth.clone(), credential_resolver),
            config,
        );

        client_info()
            .serve(transport)
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                reason: format!("HTTP streaming handshake failed: {e}"),
            })
    }

    /// Call a tool on the connected MCP server.
    ///
    /// Returns an error if the connection is disconnected.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        let peer = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.status == McpConnectionStatus::Disconnected {
                return Err(McpError::ToolCallFailed {
                    server: self.config.name.clone(),
                    tool: tool_name.to_string(),
                    reason: "server is disconnected".to_string(),
                });
            }

            state.peer.clone().ok_or_else(|| McpError::ToolCallFailed {
                server: self.config.name.clone(),
                tool: tool_name.to_string(),
                reason: "no active session".to_string(),
            })?
        };

        let json_args = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => {
                warn!(
                    server = %self.config.name,
                    tool = %tool_name,
                    "tool arguments are not a JSON object, wrapping"
                );
                let mut map = serde_json::Map::new();
                map.insert("value".to_string(), arguments);
                Some(map)
            }
        };

        let mut params = CallToolRequestParams::new(tool_name.to_string());
        params.arguments = json_args;

        peer.call_tool(params)
            .await
            .map_err(|e| McpError::ToolCallFailed {
                server: self.config.name.clone(),
                tool: tool_name.to_string(),
                reason: e.to_string(),
            })
    }

    /// Shut down the connection gracefully.
    ///
    /// Aborts the monitor task (which drops the underlying `RunningService`).
    /// For stdio servers, rmcp's `ChildWithCleanup` terminates the subprocess
    /// on drop. For SSE servers, the HTTP connection is closed. Explicit
    /// shutdown also emits `McpServerDisconnected` because the monitor only
    /// reports transport-driven exits.
    pub async fn shutdown(&self) {
        let (monitor, should_emit_disconnect) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let was_live = state.status == McpConnectionStatus::Connected
                || state.peer.is_some()
                || state.monitor.is_some();
            state.status = McpConnectionStatus::Disconnected;
            state.peer = None;
            (state.monitor.take(), was_live)
        };

        if let Some(monitor) = monitor {
            monitor.abort();
            let _ = monitor.await;
        }

        if should_emit_disconnect {
            emit_event(self.event_tx.as_ref(), || {
                crate::event::server_disconnected(&self.config.name, "shutdown")
            });
        }
    }
}

fn build_stdio_command(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    cmd.env_clear();
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd
}

async fn resolve_sse_bearer_secret(
    bearer_auth: &SseBearerAuth,
    credential_resolver: &dyn CredentialResolver,
) -> Result<String, SseBearerResolutionError> {
    let resolve_future = credential_resolver.resolve(&bearer_auth.credential_key);
    let credential = tokio::time::timeout(Duration::from_secs(30), resolve_future)
        .await
        .map_err(|_| SseBearerResolutionError::Timeout {
            key: bearer_auth.credential_key.clone(),
        })??;

    let actual_type = resolved_credential_type(&credential);
    if actual_type != bearer_auth.credential_type {
        return Err(SseBearerResolutionError::TypeMismatch {
            key: bearer_auth.credential_key.clone(),
            expected: bearer_auth.credential_type,
            actual: actual_type,
        });
    }

    Ok(resolved_credential_secret(&credential).to_string())
}

fn sse_bearer_resolution_error(error: SseBearerResolutionError, server_name: &str) -> McpError {
    let reason = match error {
        SseBearerResolutionError::Timeout { key } => {
            format!("timed out resolving SSE credential `{key}`")
        }
        SseBearerResolutionError::Credential(error) => {
            format!("failed to resolve SSE credential: {error}")
        }
        SseBearerResolutionError::TypeMismatch {
            key,
            expected,
            actual,
        } => {
            format!(
                "SSE credential type mismatch for `{key}`: expected {expected:?}, got {actual:?}"
            )
        }
    };

    McpError::ConnectionFailed {
        server: server_name.to_string(),
        reason,
    }
}

fn map_reqwest_streamable_http_error(
    error: StreamableHttpError<reqwest::Error>,
) -> StreamableHttpError<ResolverBackedSseHttpClientError> {
    match error {
        StreamableHttpError::Sse(error) => StreamableHttpError::Sse(error),
        StreamableHttpError::Io(error) => StreamableHttpError::Io(error),
        StreamableHttpError::Client(error) => {
            StreamableHttpError::Client(ResolverBackedSseHttpClientError::Http(error))
        }
        StreamableHttpError::UnexpectedEndOfStream => StreamableHttpError::UnexpectedEndOfStream,
        StreamableHttpError::UnexpectedServerResponse(error) => {
            StreamableHttpError::UnexpectedServerResponse(error)
        }
        StreamableHttpError::UnexpectedContentType(content_type) => {
            StreamableHttpError::UnexpectedContentType(content_type)
        }
        StreamableHttpError::ServerDoesNotSupportSse => {
            StreamableHttpError::ServerDoesNotSupportSse
        }
        StreamableHttpError::ServerDoesNotSupportDeleteSession => {
            StreamableHttpError::ServerDoesNotSupportDeleteSession
        }
        StreamableHttpError::TokioJoinError(error) => StreamableHttpError::TokioJoinError(error),
        StreamableHttpError::Deserialize(error) => StreamableHttpError::Deserialize(error),
        StreamableHttpError::TransportChannelClosed => StreamableHttpError::TransportChannelClosed,
        StreamableHttpError::AuthRequired(error) => StreamableHttpError::AuthRequired(error),
        StreamableHttpError::InsufficientScope(error) => {
            StreamableHttpError::InsufficientScope(error)
        }
        StreamableHttpError::ReservedHeaderConflict(header) => {
            StreamableHttpError::ReservedHeaderConflict(header)
        }
        StreamableHttpError::SessionExpired => StreamableHttpError::SessionExpired,
        other => StreamableHttpError::UnexpectedServerResponse(
            format!("unexpected streamable HTTP error: {other:?}").into(),
        ),
    }
}

const fn resolved_credential_type(credential: &ResolvedCredential) -> CredentialType {
    match credential {
        ResolvedCredential::ApiKey(_) => CredentialType::ApiKey,
        ResolvedCredential::Bearer(_) => CredentialType::Bearer,
        ResolvedCredential::OAuth2AccessToken(_) => CredentialType::OAuth2,
    }
}

fn resolved_credential_secret(credential: &ResolvedCredential) -> &str {
    match credential {
        ResolvedCredential::ApiKey(secret)
        | ResolvedCredential::Bearer(secret)
        | ResolvedCredential::OAuth2AccessToken(secret) => secret,
    }
}

fn parse_custom_headers(
    headers: &HashMap<String, String>,
    server_name: &str,
) -> Result<HashMap<HeaderName, HeaderValue>, McpError> {
    headers
        .iter()
        .map(|(name, value)| {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                McpError::ConnectionFailed {
                    server: server_name.to_string(),
                    reason: format!("invalid SSE header name `{name}`: {error}"),
                }
            })?;
            let header_value =
                HeaderValue::from_str(value).map_err(|error| McpError::ConnectionFailed {
                    server: server_name.to_string(),
                    reason: format!("invalid SSE header value for `{name}`: {error}"),
                })?;
            Ok((header_name, header_value))
        })
        .collect()
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.status = McpConnectionStatus::Disconnected;
        state.peer = None;
        if let Some(monitor) = state.monitor.take() {
            monitor.abort();
        }
    }
}

/// Send an event on the optional channel, ignoring closed-receiver errors.
///
/// The emitter is lazy so we never allocate event payloads when no channel
/// is wired.
pub fn emit_event(
    event_tx: Option<&UnboundedSender<AgentEvent>>,
    build: impl FnOnce() -> AgentEvent,
) {
    if let Some(tx) = event_tx {
        let _ = tx.send(build());
    }
}

/// Build the `ClientInfo` used for MCP handshakes.
fn client_info() -> ClientInfo {
    let mut info = ClientInfo::default();
    info.client_info = Implementation::new("swink-agent-mcp", env!("CARGO_PKG_VERSION"));
    info
}

/// Spawn a background task that awaits the service lifecycle.
///
/// When the underlying service exits with `QuitReason::Closed` or a join error,
/// the shared state is updated to `Disconnected` and
/// `McpServerDisconnected` is sent on `event_tx`. Voluntary cancellations
/// (`QuitReason::Cancelled`) and join errors are silently ignored since they
/// are initiated by the caller via `shutdown()`.
fn spawn_monitor(
    service: RunningService<RoleClient, ClientInfo>,
    state: Arc<Mutex<McpConnectionState>>,
    server_name: String,
    event_tx: Option<UnboundedSender<AgentEvent>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Ok(QuitReason::Closed | QuitReason::JoinError(_)) = service.waiting().await {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            state.status = McpConnectionStatus::Disconnected;
            state.peer = None;
            state.monitor = None;
            drop(state);

            if let Some(ref tx) = event_tx {
                let _ = tx.send(crate::event::server_disconnected(
                    &server_name,
                    "transport closed",
                ));
            }
        }
        // Cancelled by shutdown() or other future variants; no event needed.
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::process::Stdio;

    use super::build_stdio_command;

    #[tokio::test]
    async fn stdio_command_clears_inherited_environment_before_applying_configured_values() {
        let inherited_key = std::env::vars()
            .map(|(key, _)| key)
            .find(|key| key != "COMSPEC" && !key.starts_with('='))
            .expect("current process should expose at least one inherited env var");
        let configured_key = "SWINK_MCP_STDIO_ONLY";
        let configured_value = "configured-value";
        let env = HashMap::from([(configured_key.to_string(), configured_value.to_string())]);

        let (command, args) = env_probe_command();
        let mut cmd = build_stdio_command(&command, &args, &env);
        cmd.stdout(Stdio::piped());

        let output = cmd.output().await.expect("spawn env probe");
        assert!(
            output.status.success(),
            "env probe should exit successfully: {output:?}"
        );

        let stdout = String::from_utf8(output.stdout).expect("env probe output should be UTF-8");
        let configured_line = format!("{configured_key}={configured_value}");
        let inherited_prefix = format!("{inherited_key}=");

        assert!(
            stdout.lines().any(|line| line == configured_line),
            "configured env var should be present in child env: {stdout}"
        );
        assert!(
            !stdout
                .lines()
                .any(|line| line.starts_with(&inherited_prefix)),
            "inherited env var should be absent after env_clear(): {stdout}"
        );
    }

    #[cfg(windows)]
    fn env_probe_command() -> (String, Vec<String>) {
        let command =
            std::env::var("COMSPEC").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".into());
        (command, vec!["/d".into(), "/c".into(), "set".into()])
    }

    #[cfg(not(windows))]
    fn env_probe_command() -> (String, Vec<String>) {
        use std::path::Path;

        let command = if Path::new("/usr/bin/env").exists() {
            "/usr/bin/env".to_string()
        } else {
            "/bin/env".to_string()
        };
        (command, Vec::new())
    }
}
