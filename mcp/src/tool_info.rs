//! Owned MCP tool metadata, decoupled from `rmcp` wire types.
//!
//! [`McpToolInfo`] carries the tool definition fields this crate actually
//! consumes (name, description, input schema) as owned data, so the public
//! API never exposes `rmcp` types and an `rmcp` major version bump cannot
//! force a semver-major bump on `swink-agent-mcp`.

use serde_json::Value;

/// Owned metadata for a tool discovered from an MCP server.
///
/// Captured at discovery time from the server's advertised tool definition.
/// This is the crate's public representation of a discovered tool; the raw
/// wire type stays internal.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolInfo {
    /// Tool name as advertised by the server (before any prefixing or
    /// sanitization applied by [`McpTool`](crate::McpTool)).
    pub name: String,
    /// Human-readable description (empty when the server omits one).
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
}

impl McpToolInfo {
    /// Create tool metadata from owned fields.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }

    /// Convert from the `rmcp` wire type.
    ///
    /// Internal on purpose: keeping this `pub(crate)` is what guarantees no
    /// `rmcp` type appears in the crate's public API surface.
    pub(crate) fn from_rmcp(tool: &rmcp::model::Tool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or("").to_string(),
            input_schema: tool.schema_as_json_value(),
        }
    }
}
