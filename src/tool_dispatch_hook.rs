//! Hook trait for intercepting tool dispatch before and after execution.

use crate::tool::AgentToolResult;

/// Hook for intercepting tool calls before and after execution.
///
/// Use this to log, audit, or transform tool interactions without
/// modifying the dispatch pipeline itself.
pub trait ToolDispatchHook: Send + Sync {
    /// Called before a tool is dispatched for execution.
    fn before_dispatch(&self, tool_name: &str, args: &serde_json::Value);
    /// Called after a tool completes execution.
    fn after_dispatch(&self, tool_name: &str, result: &AgentToolResult);
}
