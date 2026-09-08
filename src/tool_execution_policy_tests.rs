//! Tests for `tool_execution_policy`.
#![cfg(test)]

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
