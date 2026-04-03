//! Multi-server MCP orchestration.
//!
//! [`McpManager`] connects to multiple MCP servers, collects their tools with
//! optional name prefixing, detects name collisions, and exposes a flat list
//! of [`AgentTool`] implementations ready for use in an agent.

use std::collections::HashMap;
use std::sync::Arc;

use swink_agent::tool::AgentTool;
use tracing::warn;

use crate::config::McpServerConfig;
use crate::connection::McpConnection;
use crate::error::McpError;
use crate::tool::McpTool;

/// Orchestrates connections to multiple MCP servers.
///
/// Provides a unified view of tools across all connected servers, handling
/// name prefixing and collision detection.
///
/// # Example
///
/// ```no_run
/// use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};
///
/// # async fn example() -> Result<(), swink_agent_mcp::McpError> {
/// let configs = vec![
///     McpServerConfig {
///         name: "fs".into(),
///         transport: McpTransport::Stdio {
///             command: "mcp-server-fs".into(),
///             args: vec![],
///             env: Default::default(),
///         },
///         tool_prefix: Some("fs".into()),
///         tool_filter: None,
///         requires_approval: true,
///     },
/// ];
///
/// let mut manager = McpManager::new(configs);
/// manager.connect_all().await?;
///
/// let tools = manager.tools();
/// // tools are ready to add to AgentOptions
/// # Ok(())
/// # }
/// ```
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    connections: Vec<Arc<McpConnection>>,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl McpManager {
    /// Create a manager from server configurations.
    ///
    /// No connections are established until [`connect_all()`](Self::connect_all)
    /// is called.
    pub fn new(configs: Vec<McpServerConfig>) -> Self {
        Self {
            configs,
            connections: Vec::new(),
            tools: Vec::new(),
        }
    }

    /// Create a manager from pre-established connections.
    ///
    /// Wraps each connection in an `Arc`, creates `McpTool` wrappers for all
    /// discovered tools, and checks for name collisions. Useful for testing
    /// with in-process mock servers.
    pub fn from_connections(connections: Vec<McpConnection>) -> Result<Self, McpError> {
        let mut all_tools: Vec<(String, String, Arc<dyn AgentTool>)> = Vec::new();
        let mut arc_connections = Vec::with_capacity(connections.len());

        for connection in connections {
            let conn = Arc::new(connection);
            let config = &conn.config;

            for tool_def in &conn.discovered_tools {
                let mcp_tool = McpTool::new(
                    tool_def,
                    config.tool_prefix.as_deref(),
                    &config.name,
                    config.requires_approval,
                    Arc::clone(&conn),
                );
                let name = mcp_tool.name().to_string();
                all_tools.push((name, config.name.clone(), Arc::new(mcp_tool)));
            }

            arc_connections.push(conn);
        }

        // Check for name collisions across all servers.
        let tools = detect_collisions_and_collect(all_tools)?;

        Ok(Self {
            configs: Vec::new(),
            connections: arc_connections,
            tools,
        })
    }

    /// Connect to all configured servers, discover tools.
    ///
    /// Servers that fail to connect are logged and skipped — the remaining
    /// servers' tools are still available. Returns an error only if a tool
    /// name collision is detected across servers.
    pub async fn connect_all(&mut self) -> Result<(), McpError> {
        let mut all_tools: Vec<(String, String, Arc<dyn AgentTool>)> = Vec::new();

        for config in self.configs.clone() {
            match McpConnection::connect(config.clone()).await {
                Ok(connection) => {
                    let conn = Arc::new(connection);

                    for tool_def in &conn.discovered_tools {
                        let mcp_tool = McpTool::new(
                            tool_def,
                            config.tool_prefix.as_deref(),
                            &config.name,
                            config.requires_approval,
                            Arc::clone(&conn),
                        );
                        let name = mcp_tool.name().to_string();
                        all_tools.push((name, config.name.clone(), Arc::new(mcp_tool)));
                    }

                    self.connections.push(conn);
                }
                Err(e) => {
                    warn!(
                        server = %config.name,
                        error = %e,
                        "MCP server connection failed, continuing without this server"
                    );
                }
            }
        }

        self.tools = detect_collisions_and_collect(all_tools)?;
        Ok(())
    }

    /// Get all discovered tools as `Arc<dyn AgentTool>`.
    ///
    /// Tools are ready to be added to `AgentOptions.tools`.
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.clone()
    }

    /// Disconnect all servers and clean up resources.
    pub async fn shutdown(&mut self) {
        // Drop tool references first so Arc refcount decreases.
        self.tools.clear();

        for conn in self.connections.drain(..) {
            match Arc::try_unwrap(conn) {
                Ok(c) => c.shutdown().await,
                Err(arc) => {
                    warn!(
                        server = %arc.config.name,
                        "MCP connection still referenced, cannot shut down cleanly"
                    );
                }
            }
        }
    }
}

impl std::fmt::Debug for McpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpManager")
            .field("configs", &self.configs.len())
            .field("connections", &self.connections.len())
            .field("tools", &self.tools.len())
            .finish()
    }
}

/// Check for tool name collisions and return the flat tool list.
///
/// Each tool is tracked as `(name, server_name, tool)`. If two tools share
/// the same name from different servers, returns `McpError::ToolNameCollision`.
fn detect_collisions_and_collect(
    all_tools: Vec<(String, String, Arc<dyn AgentTool>)>,
) -> Result<Vec<Arc<dyn AgentTool>>, McpError> {
    let mut seen: HashMap<String, String> = HashMap::new();

    for (name, server, _) in &all_tools {
        if let Some(existing_server) = seen.get(name) {
            return Err(McpError::ToolNameCollision {
                name: name.clone(),
                server_a: existing_server.clone(),
                server_b: server.clone(),
            });
        }
        seen.insert(name.clone(), server.clone());
    }

    Ok(all_tools.into_iter().map(|(_, _, tool)| tool).collect())
}
