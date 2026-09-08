//! Tests for `metrics`.
#![cfg(test)]

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingCollector {
    count: AtomicUsize,
}

impl MetricsCollector for CountingCollector {
    fn on_metrics<'a>(&'a self, _metrics: &'a TurnMetrics) -> MetricsFuture<'a> {
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
