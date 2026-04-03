//! Mock MCP server helpers for testing.
//!
//! Provides utilities to spawn in-process mock MCP servers that advertise
//! configurable tools and return configurable results.

use std::collections::HashMap;

use serde_json::Value;

/// Configuration for a single mock tool.
#[derive(Debug, Clone)]
pub struct MockToolDef {
    /// Tool name as advertised by the mock server.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
    /// The result text to return when this tool is called.
    pub result_text: String,
    /// Whether the result should be marked as an error.
    pub is_error: bool,
}

impl MockToolDef {
    /// Create a simple mock tool that returns text.
    pub fn simple(name: &str, result_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock tool: {name}"),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            result_text: result_text.to_string(),
            is_error: false,
        }
    }

    /// Create a mock tool with a specified input schema.
    pub fn with_schema(name: &str, schema: Value, result_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock tool: {name}"),
            input_schema: schema,
            result_text: result_text.to_string(),
            is_error: false,
        }
    }

    /// Create a mock tool that returns an error.
    pub fn error(name: &str, error_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock error tool: {name}"),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            result_text: error_text.to_string(),
            is_error: true,
        }
    }
}

/// Configuration for a mock MCP server.
#[derive(Debug, Clone)]
pub struct MockServerConfig {
    /// Tools to advertise.
    pub tools: Vec<MockToolDef>,
    /// Custom tool results keyed by `(tool_name, serialized_args)`.
    pub custom_results: HashMap<String, MockToolDef>,
}

impl MockServerConfig {
    /// Create a mock server config with the given tools.
    pub fn new(tools: Vec<MockToolDef>) -> Self {
        Self {
            tools,
            custom_results: HashMap::new(),
        }
    }

    /// Create an empty mock server config (no tools).
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            custom_results: HashMap::new(),
        }
    }
}
