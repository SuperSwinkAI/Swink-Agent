//! MCP server connection management.
//!
//! Wraps an `rmcp` client session, handling tool discovery and providing
//! access to the peer for tool call forwarding.

use rmcp::model::{CallToolRequestParam, CallToolResult, ClientInfo, Implementation};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;
use std::borrow::Cow;
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
/// Holds the rmcp `RunningService`, discovered tool definitions, and connection
/// status. Created via [`McpConnection::connect`].
pub struct McpConnection {
    /// The server configuration used to establish this connection.
    pub config: McpServerConfig,
    /// Discovered tools from the server (raw rmcp tool definitions).
    pub discovered_tools: Vec<rmcp::model::Tool>,
    /// Current connection status.
    pub status: McpConnectionStatus,
    /// The running rmcp client session (None if disconnected).
    service: Option<RunningService<RoleClient, ClientInfo>>,
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnection")
            .field("config", &self.config)
            .field("discovered_tools", &self.discovered_tools.len())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl McpConnection {
    /// Create a disconnected connection placeholder.
    ///
    /// Useful for tests that only inspect tool metadata without establishing
    /// a real connection. Calls to `call_tool()` will fail immediately.
    pub const fn disconnected(config: McpServerConfig) -> Self {
        Self {
            config,
            discovered_tools: Vec::new(),
            status: McpConnectionStatus::Disconnected,
            service: None,
        }
    }

    /// Create a connection from a pre-established rmcp service.
    ///
    /// Performs tool discovery on the already-connected service. Useful for
    /// testing with in-process mock servers or when the transport is managed
    /// externally.
    pub async fn from_service(
        config: McpServerConfig,
        service: RunningService<RoleClient, ClientInfo>,
    ) -> Result<Self, McpError> {
        let discovered_tools =
            service
                .peer()
                .list_all_tools()
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

        Ok(Self {
            config,
            discovered_tools,
            status: McpConnectionStatus::Connected,
            service: Some(service),
        })
    }

    /// Connect to an MCP server using the configured transport.
    ///
    /// Currently supports stdio transport only. SSE transport will be added
    /// in a later phase.
    pub async fn connect(config: McpServerConfig) -> Result<Self, McpError> {
        let service = match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                Self::connect_stdio(command, args, env, &config.name).await?
            }
            McpTransport::Sse { .. } => {
                return Err(McpError::ConnectionFailed {
                    server: config.name.clone(),
                    reason: "SSE transport not yet implemented".to_string(),
                });
            }
        };

        // Discover tools from the server.
        let discovered_tools =
            service
                .peer()
                .list_all_tools()
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

        Ok(Self {
            config,
            discovered_tools,
            status: McpConnectionStatus::Connected,
            service: Some(service),
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

        let transport = TokioChildProcess::new(&mut cmd).map_err(|e| McpError::SpawnFailed {
            server: server_name.to_string(),
            source: e,
        })?;

        let client_info = ClientInfo {
            client_info: Implementation {
                name: "swink-agent-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            ..ClientInfo::default()
        };

        let service = client_info
            .serve(transport)
            .await
            .map_err(|e: std::io::Error| McpError::ConnectionFailed {
                server: server_name.to_string(),
                reason: format!("connection handshake failed: {e}"),
            })?;

        Ok(service)
    }

    /// Call a tool on the connected MCP server.
    ///
    /// Returns an error if the connection is disconnected.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        if self.status == McpConnectionStatus::Disconnected {
            return Err(McpError::ToolCallFailed {
                server: self.config.name.clone(),
                tool: tool_name.to_string(),
                reason: "server is disconnected".to_string(),
            });
        }

        let service = self
            .service
            .as_ref()
            .ok_or_else(|| McpError::ToolCallFailed {
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

        let params = CallToolRequestParam {
            name: Cow::Owned(tool_name.to_string()),
            arguments: json_args,
        };

        service
            .peer()
            .call_tool(params)
            .await
            .map_err(|e| McpError::ToolCallFailed {
                server: self.config.name.clone(),
                tool: tool_name.to_string(),
                reason: e.to_string(),
            })
    }

    /// Shut down the connection gracefully.
    pub async fn shutdown(mut self) {
        if let Some(service) = self.service.take()
            && let Err(e) = service.cancel().await
        {
            warn!(
                server = %self.config.name,
                error = %e,
                "error during MCP server shutdown"
            );
        }
        self.status = McpConnectionStatus::Disconnected;
    }
}
