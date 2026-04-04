//! MCP server connection management.
//!
//! Wraps an `rmcp` client session, handling tool discovery and providing
//! access to the peer for tool call forwarding. A background monitor task
//! awaits the service lifecycle and emits a disconnect event when the
//! transport closes unexpectedly.

use std::sync::{Arc, Mutex, PoisonError};

use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo, Implementation};
use rmcp::service::{Peer, QuitReason, RoleClient, RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::Value;
use swink_agent::AgentEvent;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::{McpServerConfig, McpTransport};
use crate::error::McpError;

/// Status of an MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectionStatus {
    /// Connected and ready to serve tool calls.
    Connected,
    /// Disconnected — tool calls will fail immediately.
    Disconnected,
}

/// A connection to a single MCP server.
///
/// Holds a cloned `Peer<RoleClient>` for making tool calls and a background
/// monitor task that awaits the service lifecycle. When the remote transport
/// closes the monitor transitions `status` to `Disconnected` and sends an
/// `AgentEvent::McpServerDisconnected` on the optional event channel.
///
/// Created via [`McpConnection::connect`] or [`McpConnection::from_service`].
pub struct McpConnection {
    /// The server configuration used to establish this connection.
    pub config: McpServerConfig,
    /// Discovered tools from the server (raw rmcp tool definitions).
    pub discovered_tools: Vec<rmcp::model::Tool>,
    /// Shared connection status — written by the monitor task, read by callers.
    status: Arc<Mutex<McpConnectionStatus>>,
    /// Cloned peer handle for forwarding tool calls.
    peer: Option<Peer<RoleClient>>,
    /// Background lifecycle-monitor task. Holds the `RunningService` and
    /// resolves when the transport closes.
    monitor: Option<JoinHandle<()>>,
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
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Create a disconnected connection placeholder.
    ///
    /// Useful for tests that only inspect tool metadata without establishing
    /// a real connection. Calls to `call_tool()` will fail immediately.
    pub fn disconnected(config: McpServerConfig) -> Self {
        Self {
            config,
            discovered_tools: Vec::new(),
            status: Arc::new(Mutex::new(McpConnectionStatus::Disconnected)),
            peer: None,
            monitor: None,
        }
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

        let status = Arc::new(Mutex::new(McpConnectionStatus::Connected));
        let monitor = spawn_monitor(service, Arc::clone(&status), config.name.clone(), event_tx);

        Ok(Self {
            config,
            discovered_tools,
            status,
            peer: Some(peer),
            monitor: Some(monitor),
        })
    }

    /// Connect to an MCP server using the configured transport.
    ///
    /// Supports stdio and SSE (HTTP) transports. Spawns a background lifecycle
    /// monitor that sends `AgentEvent::McpServerDisconnected` on `event_tx`
    /// when the transport closes unexpectedly.
    pub async fn connect(
        config: McpServerConfig,
        event_tx: Option<UnboundedSender<AgentEvent>>,
    ) -> Result<Self, McpError> {
        let service = match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                Self::connect_stdio(command, args, env, &config.name).await?
            }
            McpTransport::Sse { url, bearer_token } => {
                Self::connect_sse(url, bearer_token.as_deref(), &config.name).await?
            }
        };

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

        let status = Arc::new(Mutex::new(McpConnectionStatus::Connected));
        let monitor = spawn_monitor(service, Arc::clone(&status), config.name.clone(), event_tx);

        Ok(Self {
            config,
            discovered_tools,
            status,
            peer: Some(peer),
            monitor: Some(monitor),
        })
    }

    /// Connect to a stdio-based MCP server subprocess.
    async fn connect_stdio(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        server_name: &str,
    ) -> Result<RunningService<RoleClient, ClientInfo>, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args);
        for (key, value) in env {
            cmd.env(key, value);
        }

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
        server_name: &str,
    ) -> Result<RunningService<RoleClient, ClientInfo>, McpError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        if let Some(token) = bearer_token {
            config = config.auth_header(format!("Bearer {token}"));
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

    /// Call a tool on the connected MCP server.
    ///
    /// Returns an error if the connection is disconnected.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        if self.status() == McpConnectionStatus::Disconnected {
            return Err(McpError::ToolCallFailed {
                server: self.config.name.clone(),
                tool: tool_name.to_string(),
                reason: "server is disconnected".to_string(),
            });
        }

        let peer = self.peer.as_ref().ok_or_else(|| McpError::ToolCallFailed {
            server: self.config.name.clone(),
            tool: tool_name.to_string(),
            reason: "no active session".to_string(),
        })?;

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
    /// on drop. For SSE servers, the HTTP connection is closed.
    pub async fn shutdown(mut self) {
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
            let _ = monitor.await;
        }
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) =
            McpConnectionStatus::Disconnected;
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
/// When the transport closes with `QuitReason::Closed` (remote disconnect or
/// crash), the shared `status` is updated to `Disconnected` and
/// `McpServerDisconnected` is sent on `event_tx`. Voluntary cancellations
/// (`QuitReason::Cancelled`) and join errors are silently ignored since they
/// are initiated by the caller via `shutdown()`.
fn spawn_monitor(
    service: RunningService<RoleClient, ClientInfo>,
    status: Arc<Mutex<McpConnectionStatus>>,
    server_name: String,
    event_tx: Option<UnboundedSender<AgentEvent>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Ok(QuitReason::Closed | QuitReason::JoinError(_)) = service.waiting().await {
            *status.lock().unwrap_or_else(PoisonError::into_inner) =
                McpConnectionStatus::Disconnected;
            if let Some(ref tx) = event_tx {
                let _ = tx.send(crate::event::server_disconnected(
                    &server_name,
                    "transport closed",
                ));
            }
        }
        // Cancelled by shutdown() or other future variants — no event needed.
    })
}
