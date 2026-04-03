//! MCP tool wrapper implementing the `AgentTool` trait.
//!
//! Each discovered MCP tool is wrapped in an [`McpTool`] that delegates
//! execution to the MCP server via the connection.

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use swink_agent::SessionState;
use swink_agent::credential::ResolvedCredential;
use swink_agent::tool::{AgentTool, AgentToolResult, ToolFuture, ToolMetadata};

use crate::connection::McpConnection;
use crate::convert;

/// An MCP-discovered tool that implements `AgentTool`.
///
/// Delegates execution to the MCP server via the shared connection.
/// The tool name may include a prefix if configured on the server.
pub struct McpTool {
    /// The tool name used for routing (possibly prefixed).
    name: String,
    /// The original tool name as advertised by the MCP server.
    original_name: String,
    /// Human-readable description from the MCP server.
    description: String,
    /// JSON Schema for input parameters.
    input_schema: Value,
    /// The server name this tool belongs to.
    server_name: String,
    /// Whether this tool requires approval before execution.
    requires_approval: bool,
    /// Shared reference to the MCP connection for forwarding calls.
    connection: Arc<McpConnection>,
}

impl McpTool {
    /// Create a new MCP tool wrapper.
    ///
    /// If `prefix` is provided, the tool name becomes `{prefix}_{original_name}`.
    pub fn new(
        tool: &rmcp::model::Tool,
        prefix: Option<&str>,
        server_name: &str,
        requires_approval: bool,
        connection: Arc<McpConnection>,
    ) -> Self {
        let (original_name, description, input_schema) = convert::tool_definition(tool);
        let name = prefix.map_or_else(|| original_name.clone(), |p| format!("{p}_{original_name}"));

        Self {
            name,
            original_name,
            description,
            input_schema,
            server_name: server_name.to_string(),
            requires_approval,
            connection,
        }
    }

    /// The original tool name as advertised by the MCP server.
    pub fn original_name(&self) -> &str {
        &self.original_name
    }

    /// The server name this tool belongs to.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.name)
            .field("original_name", &self.original_name)
            .field("server_name", &self.server_name)
            .field("requires_approval", &self.requires_approval)
            .finish_non_exhaustive()
    }
}

impl AgentTool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> &Value {
        &self.input_schema
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn metadata(&self) -> Option<ToolMetadata> {
        Some(ToolMetadata::with_namespace(&self.server_name))
    }

    fn approval_context(&self, params: &Value) -> Option<Value> {
        Some(params.clone())
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<SessionState>>,
        _credential: Option<ResolvedCredential>,
    ) -> ToolFuture<'_> {
        let original_name = self.original_name.clone();
        Box::pin(async move {
            tokio::select! {
                result = self.connection.call_tool(&original_name, params) => {
                    match result {
                        Ok(call_result) => convert::call_result_to_agent_result(&call_result),
                        Err(e) => AgentToolResult::error(e.to_string()),
                    }
                }
                () = cancellation_token.cancelled() => {
                    AgentToolResult::error("MCP tool call cancelled")
                }
            }
        })
    }
}
