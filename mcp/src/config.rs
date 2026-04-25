//! Configuration types for MCP server connections.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swink_agent::CredentialType;

/// Resolver-backed bearer auth for SSE MCP transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseBearerAuth {
    /// Credential store key resolved before establishing the connection.
    pub credential_key: String,
    /// Expected resolved credential type for this bearer header.
    pub credential_type: CredentialType,
}

/// Transport type for MCP server communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// Subprocess with stdin/stdout JSON-RPC.
    Stdio {
        command: String,
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// HTTP Server-Sent Events.
    Sse {
        url: String,
        bearer_token: Option<String>,
        #[serde(default)]
        bearer_auth: Option<SseBearerAuth>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// Controls which tools from a server are exposed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolFilter {
    /// If set, only tools with names in this list are included.
    pub allow: Option<Vec<String>>,
    /// If set, tools with names in this list are excluded.
    pub deny: Option<Vec<String>>,
}

impl ToolFilter {
    /// Apply the filter to a list of tool names.
    ///
    /// If `allow` is set, keep only matching names. Then if `deny` is set,
    /// remove matching names.
    pub fn matches(&self, name: &str) -> bool {
        if let Some(allow) = &self.allow
            && !allow.iter().any(|a| a == name)
        {
            return false;
        }
        if let Some(deny) = &self.deny
            && deny.iter().any(|d| d == name)
        {
            return false;
        }
        true
    }
}

/// Configuration for connecting to a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique identifier for the server.
    pub name: String,
    /// How to connect to the server.
    pub transport: McpTransport,
    /// If set, prefixes all tool names from this server with `{prefix}_`.
    pub tool_prefix: Option<String>,
    /// Controls which discovered tools are exposed.
    pub tool_filter: Option<ToolFilter>,
    /// Whether tools from this server require user approval before execution.
    #[serde(default = "default_requires_approval")]
    pub requires_approval: bool,
    /// Optional timeout for the initial transport handshake.
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,
    /// Optional timeout for the initial tool discovery request.
    #[serde(default)]
    pub discovery_timeout_ms: Option<u64>,
}

const fn default_requires_approval() -> bool {
    true
}

impl McpServerConfig {
    pub(crate) fn connect_timeout(&self) -> Option<Duration> {
        self.connect_timeout_ms.map(Duration::from_millis)
    }

    pub(crate) fn discovery_timeout(&self) -> Option<Duration> {
        self.discovery_timeout_ms.map(Duration::from_millis)
    }
}
