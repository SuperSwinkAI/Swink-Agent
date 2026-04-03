//! Error types for MCP operations.

use std::fmt;

/// Errors from MCP operations.
#[derive(Debug)]
pub enum McpError {
    /// Failed to spawn subprocess for stdio transport.
    SpawnFailed {
        server: String,
        source: std::io::Error,
    },
    /// Failed to connect to MCP server.
    ConnectionFailed { server: String, reason: String },
    /// Tool name collision detected across servers.
    ToolNameCollision {
        name: String,
        server_a: String,
        server_b: String,
    },
    /// MCP server returned an error during tool call.
    ToolCallFailed {
        server: String,
        tool: String,
        reason: String,
    },
    /// MCP protocol error (JSON-RPC level).
    ProtocolError {
        server: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpawnFailed { server, source } => {
                write!(f, "failed to spawn MCP server '{server}': {source}")
            }
            Self::ConnectionFailed { server, reason } => {
                write!(f, "failed to connect to MCP server '{server}': {reason}")
            }
            Self::ToolNameCollision {
                name,
                server_a,
                server_b,
            } => {
                write!(
                    f,
                    "tool name '{name}' collides between servers '{server_a}' and '{server_b}'"
                )
            }
            Self::ToolCallFailed {
                server,
                tool,
                reason,
            } => {
                write!(
                    f,
                    "tool call '{tool}' failed on MCP server '{server}': {reason}"
                )
            }
            Self::ProtocolError { server, source } => {
                write!(f, "protocol error with MCP server '{server}': {source}")
            }
        }
    }
}

impl std::error::Error for McpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SpawnFailed { source, .. } => Some(source),
            Self::ProtocolError { source, .. } => Some(source.as_ref()),
            Self::ConnectionFailed { .. }
            | Self::ToolNameCollision { .. }
            | Self::ToolCallFailed { .. } => None,
        }
    }
}

impl From<std::io::Error> for McpError {
    fn from(err: std::io::Error) -> Self {
        Self::SpawnFailed {
            server: String::new(),
            source: err,
        }
    }
}
