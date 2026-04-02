//! Tool execution ordering policy.
//!
//! By default the agent loop executes all tool calls concurrently via
//! `tokio::spawn`. This module provides [`ToolExecutionPolicy`] to control
//! dispatch ordering — sequential, priority-based, or fully custom via the
//! [`ToolExecutionStrategy`] trait.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

// ─── ToolCallSummary ─────────────────────────────────────────────────────────

/// Lightweight view of a pending tool call, exposed to policy callbacks.
///
/// This is intentionally a borrowed view so priority functions do not need
/// to clone arguments.
#[derive(Debug)]
pub struct ToolCallSummary<'a> {
    /// Unique identifier for this tool call.
    pub id: &'a str,
    /// Name of the tool being invoked.
    pub name: &'a str,
    /// Arguments passed to the tool.
    pub arguments: &'a Value,
}

// ─── PriorityFn ──────────────────────────────────────────────────────────────

/// Callback that assigns an integer priority to a tool call.
///
/// Higher values execute first. Tool calls with the same priority execute
/// concurrently within their group; groups execute sequentially from highest
/// to lowest priority.
pub type PriorityFn = dyn Fn(&ToolCallSummary<'_>) -> i32 + Send + Sync;

/// A boxed future returned by a [`ToolExecutionStrategy`].
pub type ToolExecutionStrategyFuture<'a> =
    Pin<Box<dyn Future<Output = Vec<Vec<usize>>> + Send + 'a>>;

// ─── ToolExecutionStrategy ───────────────────────────────────────────────────

/// Fully custom tool execution strategy.
///
/// Implementations receive the ordered list of tool call indices and can
/// decide which tools to execute in parallel, sequentially, or in any other
/// arrangement. The strategy returns execution groups — each group is a
/// `Vec<usize>` of indices into the original tool call list. Tools within a
/// group execute concurrently; groups execute sequentially in order.
pub trait ToolExecutionStrategy: Send + Sync {
    /// Partition tool calls into sequential execution groups.
    ///
    /// Each inner `Vec<usize>` contains indices (into the original tool call
    /// slice) that should execute concurrently. The outer `Vec` is processed
    /// sequentially — group 0 completes before group 1 starts, etc.
    fn partition(&self, tool_calls: &[ToolCallSummary<'_>]) -> ToolExecutionStrategyFuture<'_>;
}

// ─── ToolExecutionPolicy ─────────────────────────────────────────────────────

/// Controls how tool calls within a single turn are dispatched.
///
/// The default is [`Concurrent`](ToolExecutionPolicy::Concurrent), which
/// preserves backward compatibility by spawning all tool calls at once.
#[derive(Default)]
pub enum ToolExecutionPolicy {
    /// Execute all tool calls concurrently via `tokio::spawn` (default).
    #[default]
    Concurrent,

    /// Execute tool calls one at a time, in the order the model returned them.
    Sequential,

    /// Sort tool calls by priority (higher first), then execute groups of
    /// equal priority concurrently. Groups run sequentially from highest to
    /// lowest.
    Priority(Arc<PriorityFn>),

    /// Fully custom execution strategy.
    Custom(Arc<dyn ToolExecutionStrategy>),
}

impl Clone for ToolExecutionPolicy {
    fn clone(&self) -> Self {
        match self {
            Self::Concurrent => Self::Concurrent,
            Self::Sequential => Self::Sequential,
            Self::Priority(f) => Self::Priority(Arc::clone(f)),
            Self::Custom(s) => Self::Custom(Arc::clone(s)),
        }
    }
}

impl std::fmt::Debug for ToolExecutionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Concurrent => write!(f, "Concurrent"),
            Self::Sequential => write!(f, "Sequential"),
            Self::Priority(_) => write!(f, "Priority(...)"),
            Self::Custom(_) => write!(f, "Custom(...)"),
        }
    }
}

// ─── Compile-time Send + Sync assertions ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_concurrent() {
        assert!(matches!(
            ToolExecutionPolicy::default(),
            ToolExecutionPolicy::Concurrent
        ));
    }

    #[test]
    fn debug_formatting() {
        assert_eq!(
            format!("{:?}", ToolExecutionPolicy::Concurrent),
            "Concurrent"
        );
        assert_eq!(
            format!("{:?}", ToolExecutionPolicy::Sequential),
            "Sequential"
        );

        let pf: Arc<PriorityFn> = Arc::new(|_| 0);
        assert_eq!(
            format!("{:?}", ToolExecutionPolicy::Priority(pf)),
            "Priority(...)"
        );
    }

    #[test]
    fn tool_call_summary_debug() {
        let args = serde_json::json!({"cmd": "ls"});
        let summary = ToolCallSummary {
            id: "call_1",
            name: "bash",
            arguments: &args,
        };
        let debug = format!("{summary:?}");
        assert!(debug.contains("bash"));
        assert!(debug.contains("call_1"));
    }
}
