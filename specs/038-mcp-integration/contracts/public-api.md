# Public API Contract: swink-agent-mcp

**Crate**: `swink-agent-mcp`
**Feature**: 038-mcp-integration
**Date**: 2026-04-01

## Crate Re-exports (lib.rs)

```rust
// Configuration
pub use config::{McpServerConfig, McpTransport, SseBearerAuth, ToolFilter};

// Manager (main entry point)
pub use manager::McpManager;

// Error type
pub use error::McpError;

// Tool type (for introspection; consumers rarely need this directly)
pub use tool::McpTool;

// Connection type (for introspection; consumers rarely need this directly)
pub use connection::{McpConnection, McpConnectionStatus};
```

## Configuration API

### McpServerConfig

```rust
/// Configuration for connecting to a single MCP server.
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    pub tool_prefix: Option<String>,
    pub tool_filter: Option<ToolFilter>,
    pub requires_approval: bool,  // default: true
    pub connect_timeout_ms: Option<u64>,
    pub discovery_timeout_ms: Option<u64>,
}
```

### McpTransport

```rust
/// Transport type for MCP server communication.
pub enum McpTransport {
    /// Subprocess with stdin/stdout JSON-RPC.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// MCP Streamable HTTP transport.
    StreamableHttp {
        url: String,
        bearer_token: Option<String>,
        bearer_auth: Option<SseBearerAuth>,
        headers: HashMap<String, String>,
    },
}
```

### SseBearerAuth

```rust
/// Resolver-backed bearer auth for SSE MCP transports. When set, the bearer
/// secret is resolved from the credential store at connect time instead of
/// being embedded directly in `bearer_token`.
pub struct SseBearerAuth {
    pub credential_key: String,
    pub credential_type: CredentialType,
}
```

### ToolFilter

```rust
/// Controls which tools from a server are exposed.
pub struct ToolFilter {
    pub allow: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
}
```

## Manager API

### McpManager

```rust
impl McpManager {
    /// Create a manager from server configurations.
    pub fn new(configs: Vec<McpServerConfig>) -> Self;

    /// Connect to all configured servers, discover tools.
    /// Returns Ok with partial results if some servers fail.
    /// Emits AgentEvent for each connection/discovery.
    pub async fn connect_all(&mut self) -> Result<(), McpError>;

    /// Get all discovered tools as Arc<dyn AgentTool>.
    /// Tools are ready to be added to AgentOptions.tools.
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>>;

    /// Disconnect all servers and clean up resources.
    pub async fn shutdown(&mut self);
}
```

## Usage Pattern

```rust
use swink_agent::Agent;
use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};

// 1. Configure MCP servers
let configs = vec![
    McpServerConfig {
        name: "filesystem".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
            env: Default::default(),
        },
        tool_prefix: Some("fs".into()),
        tool_filter: None,
        requires_approval: true,
        connect_timeout_ms: Some(5_000),
        discovery_timeout_ms: Some(5_000),
    },
];

// 2. Connect and discover tools
let mut mcp = McpManager::new(configs);
mcp.connect_all().await?;

// 3. Add MCP tools to agent alongside native tools
let mut tools: Vec<Arc<dyn AgentTool>> = vec![/* native tools */];
tools.extend(mcp.tools());

// 4. Build agent with combined tool set
let agent = Agent::new(/* stream_fn, options with tools */);
```

## Error Types

```rust
/// Errors from MCP operations.
#[non_exhaustive]
pub enum McpError {
    /// Failed to spawn subprocess for stdio transport.
    SpawnFailed { server: String, source: std::io::Error },
    /// Failed to connect to MCP server.
    ConnectionFailed {
        server: String,
        reason: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// Tool name collision detected across servers.
    ToolNameCollision { name: String, server_a: String, server_b: String },
    /// MCP server returned an error during tool call.
    ToolCallFailed {
        server: String,
        tool: String,
        reason: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// MCP protocol error (JSON-RPC level).
    ProtocolError {
        server: String,
        context: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
```

## AgentEvent Additions (in core crate)

New variants added to `AgentEvent` (non-exhaustive enum):

```rust
// In src/loop_/event.rs
McpServerConnected { server_name: String },
McpServerDisconnected { server_name: String, reason: String },
McpToolsDiscovered { server_name: String, tool_count: usize },
McpToolCallStarted { server_name: String, tool_name: String },
McpToolCallCompleted { server_name: String, tool_name: String, is_error: bool },
```
