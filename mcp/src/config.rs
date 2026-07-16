//! Configuration types for MCP server connections.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swink_agent::CredentialType;

/// Resolver-backed bearer auth for SSE MCP transports.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseBearerAuth {
    /// Credential store key resolved before establishing the connection.
    pub credential_key: String,
    /// Expected resolved credential type for this bearer header.
    pub credential_type: CredentialType,
}

impl SseBearerAuth {
    /// Create bearer auth for a resolver-backed credential.
    #[must_use]
    pub fn new(credential_key: impl Into<String>, credential_type: CredentialType) -> Self {
        Self {
            credential_key: credential_key.into(),
            credential_type,
        }
    }
}

/// Transport type for MCP server communication.
#[non_exhaustive]
#[derive(Clone, Serialize, Deserialize)]
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

impl fmt::Debug for McpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio { command, args, env } => f
                .debug_struct("McpTransport::Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &RedactedStringMap(env))
                .finish(),
            Self::Sse {
                url,
                bearer_token,
                bearer_auth,
                headers,
            } => f
                .debug_struct("McpTransport::Sse")
                .field("url", url)
                .field("bearer_token", &bearer_token.as_ref().map(|_| "[REDACTED]"))
                .field("bearer_auth", bearer_auth)
                .field("headers", &RedactedStringMap(headers))
                .finish(),
        }
    }
}

struct RedactedStringMap<'a>(&'a HashMap<String, String>);

impl fmt::Debug for RedactedStringMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (key, value) in self.0 {
            if is_sensitive_debug_key(key) {
                map.entry(key, &"[REDACTED]");
            } else {
                map.entry(key, value);
            }
        }
        map.finish()
    }
}

fn is_sensitive_debug_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("authorization")
        || key.contains("api-key")
        || key.contains("api_key")
        || key.contains("token")
        || key.contains("secret")
}

/// Controls which tools from a server are exposed.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolFilter {
    /// If set, only tools with names in this list are included.
    pub allow: Option<Vec<String>>,
    /// If set, tools with names in this list are excluded.
    pub deny: Option<Vec<String>>,
}

impl ToolFilter {
    /// Create an empty filter that allows all tools.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict to only tools whose names are in `allow`.
    #[must_use]
    pub fn with_allow(mut self, allow: Vec<String>) -> Self {
        self.allow = Some(allow);
        self
    }

    /// Exclude tools whose names are in `deny`.
    #[must_use]
    pub fn with_deny(mut self, deny: Vec<String>) -> Self {
        self.deny = Some(deny);
        self
    }

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
#[non_exhaustive]
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
    /// Create a server config with the given name and transport.
    ///
    /// Optional fields default to: no tool prefix, no tool filter, approval
    /// required, and no connect/discovery timeouts. Use the `with_*` methods
    /// to override them.
    #[must_use]
    pub fn new(name: impl Into<String>, transport: McpTransport) -> Self {
        Self {
            name: name.into(),
            transport,
            tool_prefix: None,
            tool_filter: None,
            requires_approval: default_requires_approval(),
            connect_timeout_ms: None,
            discovery_timeout_ms: None,
        }
    }

    /// Prefix all tool names discovered from this server with `{prefix}_`.
    #[must_use]
    pub fn with_tool_prefix(mut self, tool_prefix: impl Into<String>) -> Self {
        self.tool_prefix = Some(tool_prefix.into());
        self
    }

    /// Restrict which discovered tools are exposed.
    #[must_use]
    pub fn with_tool_filter(mut self, tool_filter: ToolFilter) -> Self {
        self.tool_filter = Some(tool_filter);
        self
    }

    /// Whether tools from this server require user approval before execution.
    #[must_use]
    pub const fn with_requires_approval(mut self, requires_approval: bool) -> Self {
        self.requires_approval = requires_approval;
        self
    }

    /// Timeout in milliseconds for the initial transport handshake.
    #[must_use]
    pub const fn with_connect_timeout_ms(mut self, connect_timeout_ms: u64) -> Self {
        self.connect_timeout_ms = Some(connect_timeout_ms);
        self
    }

    /// Timeout in milliseconds for the initial tool discovery request.
    #[must_use]
    pub const fn with_discovery_timeout_ms(mut self, discovery_timeout_ms: u64) -> Self {
        self.discovery_timeout_ms = Some(discovery_timeout_ms);
        self
    }

    pub(crate) fn connect_timeout(&self) -> Option<Duration> {
        self.connect_timeout_ms.map(Duration::from_millis)
    }

    pub(crate) fn discovery_timeout(&self) -> Option<Duration> {
        self.discovery_timeout_ms.map(Duration::from_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_transport_debug_redacts_bearer_and_sensitive_headers() {
        let transport = McpTransport::Sse {
            url: "https://mcp.example/sse".to_string(),
            bearer_token: Some("bearer-secret-token".to_string()),
            bearer_auth: None,
            headers: HashMap::from([
                (
                    "Authorization".to_string(),
                    "Bearer auth-secret".to_string(),
                ),
                ("x-api-key".to_string(), "api-secret".to_string()),
                ("x-trace-id".to_string(), "trace-123".to_string()),
            ]),
        };

        let debug = format!("{transport:?}");

        assert!(
            !debug.contains("bearer-secret-token"),
            "Debug leaks bearer token"
        );
        assert!(
            !debug.contains("auth-secret"),
            "Debug leaks Authorization header"
        );
        assert!(!debug.contains("api-secret"), "Debug leaks API key header");
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("trace-123"));
    }

    #[test]
    fn stdio_transport_debug_redacts_sensitive_env_values() {
        let transport = McpTransport::Stdio {
            command: "server".to_string(),
            args: vec![],
            env: HashMap::from([
                ("API_TOKEN".to_string(), "env-secret".to_string()),
                ("RUST_LOG".to_string(), "debug".to_string()),
            ]),
        };

        let debug = format!("{transport:?}");

        assert!(
            !debug.contains("env-secret"),
            "Debug leaks sensitive env value"
        );
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("debug"));
    }
}
