//! Helper functions for emitting MCP-related agent events.

use swink_agent::AgentEvent;

/// Create an event for when an MCP server connects successfully.
pub fn server_connected(server_name: &str) -> AgentEvent {
    AgentEvent::McpServerConnected {
        server_name: server_name.to_string(),
    }
}

/// Create an event for when an MCP server disconnects.
pub fn server_disconnected(server_name: &str, reason: &str) -> AgentEvent {
    AgentEvent::McpServerDisconnected {
        server_name: server_name.to_string(),
        reason: reason.to_string(),
    }
}

/// Create an event for when tools are discovered from an MCP server.
pub fn tools_discovered(server_name: &str, tool_count: usize) -> AgentEvent {
    AgentEvent::McpToolsDiscovered {
        server_name: server_name.to_string(),
        tool_count,
    }
}

/// Create an event for when an MCP tool call starts.
pub fn tool_call_started(server_name: &str, tool_name: &str) -> AgentEvent {
    AgentEvent::McpToolCallStarted {
        server_name: server_name.to_string(),
        tool_name: tool_name.to_string(),
    }
}

/// Create an event for when an MCP tool call completes.
pub fn tool_call_completed(server_name: &str, tool_name: &str, is_error: bool) -> AgentEvent {
    AgentEvent::McpToolCallCompleted {
        server_name: server_name.to_string(),
        tool_name: tool_name.to_string(),
        is_error,
    }
}
