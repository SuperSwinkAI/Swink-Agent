//! Structured per-turn metrics and observability.
//!
//! The [`MetricsCollector`] trait receives a [`TurnMetrics`] snapshot at the
//! end of each agent loop turn, capturing LLM call duration, per-tool timing,
//! token usage breakdowns, and cost attribution.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::{Cost, Usage};

// ─── ToolExecMetrics ────────────────────────────────────────────────────────

/// Timing and outcome data for a single tool execution within a turn.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecMetrics {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// Wall-clock duration of the tool execution.
    pub duration: Duration,
    /// Whether the tool execution succeeded (`true`) or returned an error.
    pub success: bool,
}

impl ToolExecMetrics {
    /// Create a new tool execution metrics record.
    #[must_use]
    pub fn new(tool_name: impl Into<String>, duration: Duration, success: bool) -> Self {
        Self {
            tool_name: tool_name.into(),
            duration,
            success,
        }
    }
}

// ─── TurnMetrics ────────────────────────────────────────────────────────────

/// Metrics snapshot emitted at the end of each agent loop turn.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnMetrics {
    /// Zero-based index of the turn within the current run.
    pub turn_index: usize,
    /// Wall-clock duration of the LLM streaming call (excludes tool execution).
    pub llm_call_duration: Duration,
    /// Per-tool execution metrics for this turn (empty if no tools were called).
    pub tool_executions: Vec<ToolExecMetrics>,
    /// Token usage for this turn's LLM call.
    pub usage: Usage,
    /// Cost attributed to this turn's LLM call.
    pub cost: Cost,
    /// Total wall-clock duration of the entire turn (LLM + tools).
    pub turn_duration: Duration,
}

impl TurnMetrics {
    /// Create a new turn metrics snapshot with no tool executions recorded.
    #[must_use]
    pub fn new(
        turn_index: usize,
        llm_call_duration: Duration,
        usage: Usage,
        cost: Cost,
        turn_duration: Duration,
    ) -> Self {
        Self {
            turn_index,
            llm_call_duration,
            tool_executions: Vec::new(),
            usage,
            cost,
            turn_duration,
        }
    }

    /// Attach per-tool execution metrics for this turn.
    #[must_use]
    pub fn with_tool_executions(mut self, tool_executions: Vec<ToolExecMetrics>) -> Self {
        self.tool_executions = tool_executions;
        self
    }
}

// ─── MetricsCollector Trait ─────────────────────────────────────────────────

pub type MetricsFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
///
/// Async observer that receives structured metrics at the end of each turn.
///
/// Implementations can persist metrics, forward to monitoring systems, or
/// accumulate for post-run analysis.
///
/// # Example
///
/// ```rust
/// use swink_agent::{MetricsCollector, MetricsFuture, TurnMetrics};
///
/// struct LogMetrics;
///
/// impl MetricsCollector for LogMetrics {
///     fn on_metrics<'a>(
///         &'a self,
///         metrics: &'a TurnMetrics,
///     ) -> MetricsFuture<'a> {
///         Box::pin(async move {
///             println!("Turn {}: LLM took {:?}", metrics.turn_index, metrics.llm_call_duration);
///         })
///     }
/// }
/// ```
pub trait MetricsCollector: Send + Sync {
    /// Called at the end of each turn with the collected metrics.
    fn on_metrics<'a>(&'a self, metrics: &'a TurnMetrics) -> MetricsFuture<'a>;
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolExecMetrics>();
    assert_send_sync::<TurnMetrics>();
};

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
