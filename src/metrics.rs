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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecMetrics {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// Wall-clock duration of the tool execution.
    pub duration: Duration,
    /// Whether the tool execution succeeded (`true`) or returned an error.
    pub success: bool,
}

// ─── TurnMetrics ────────────────────────────────────────────────────────────

/// Metrics snapshot emitted at the end of each agent loop turn.
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

// ─── MetricsCollector Trait ─────────────────────────────────────────────────

/// Async observer that receives structured metrics at the end of each turn.
///
/// Implementations can persist metrics, forward to monitoring systems, or
/// accumulate for post-run analysis.
///
/// # Example
///
/// ```rust
/// use std::future::Future;
/// use std::pin::Pin;
/// use swink_agent::metrics::{MetricsCollector, TurnMetrics};
///
/// struct LogMetrics;
///
/// impl MetricsCollector for LogMetrics {
///     fn on_metrics<'a>(
///         &'a self,
///         metrics: &'a TurnMetrics,
///     ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Turn {}: LLM took {:?}", metrics.turn_index, metrics.llm_call_duration);
///         })
///     }
/// }
/// ```
pub trait MetricsCollector: Send + Sync {
    /// Called at the end of each turn with the collected metrics.
    fn on_metrics<'a>(
        &'a self,
        metrics: &'a TurnMetrics,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolExecMetrics>();
    assert_send_sync::<TurnMetrics>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingCollector {
        count: AtomicUsize,
    }

    impl MetricsCollector for CountingCollector {
        fn on_metrics<'a>(
            &'a self,
            _metrics: &'a TurnMetrics,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                self.count.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    #[tokio::test]
    async fn collector_receives_metrics() {
        let collector = CountingCollector {
            count: AtomicUsize::new(0),
        };
        let metrics = TurnMetrics {
            turn_index: 0,
            llm_call_duration: Duration::from_millis(150),
            tool_executions: vec![
                ToolExecMetrics {
                    tool_name: "bash".into(),
                    duration: Duration::from_millis(50),
                    success: true,
                },
                ToolExecMetrics {
                    tool_name: "read_file".into(),
                    duration: Duration::from_millis(10),
                    success: false,
                },
            ],
            usage: Usage {
                input: 100,
                output: 50,
                total: 150,
                ..Default::default()
            },
            cost: Cost {
                input: 0.001,
                output: 0.002,
                total: 0.003,
                ..Default::default()
            },
            turn_duration: Duration::from_millis(210),
        };
        collector.on_metrics(&metrics).await;
        assert_eq!(collector.count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn metrics_captures_tool_details() {
        let metrics = TurnMetrics {
            turn_index: 2,
            llm_call_duration: Duration::from_secs(1),
            tool_executions: vec![ToolExecMetrics {
                tool_name: "bash".into(),
                duration: Duration::from_millis(500),
                success: true,
            }],
            usage: Usage::default(),
            cost: Cost::default(),
            turn_duration: Duration::from_millis(1500),
        };
        assert_eq!(metrics.tool_executions.len(), 1);
        assert_eq!(metrics.tool_executions[0].tool_name, "bash");
        assert!(metrics.tool_executions[0].success);
        assert_eq!(metrics.turn_index, 2);
    }

    #[tokio::test]
    async fn arc_collector_is_send_sync() {
        let collector: Arc<dyn MetricsCollector> = Arc::new(CountingCollector {
            count: AtomicUsize::new(0),
        });
        let metrics = TurnMetrics {
            turn_index: 0,
            llm_call_duration: Duration::ZERO,
            tool_executions: vec![],
            usage: Usage::default(),
            cost: Cost::default(),
            turn_duration: Duration::ZERO,
        };
        collector.on_metrics(&metrics).await;
    }

    #[test]
    fn turn_metrics_serde_roundtrip() {
        let metrics = TurnMetrics {
            turn_index: 1,
            llm_call_duration: Duration::from_millis(200),
            tool_executions: vec![ToolExecMetrics {
                tool_name: "write_file".into(),
                duration: Duration::from_millis(30),
                success: true,
            }],
            usage: Usage {
                input: 50,
                output: 25,
                total: 75,
                ..Default::default()
            },
            cost: Cost {
                total: 0.005,
                ..Default::default()
            },
            turn_duration: Duration::from_millis(230),
        };
        let json = serde_json::to_string(&metrics).unwrap();
        let parsed: TurnMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.turn_index, 1);
        assert_eq!(parsed.tool_executions.len(), 1);
        assert_eq!(parsed.usage.input, 50);
    }
}
