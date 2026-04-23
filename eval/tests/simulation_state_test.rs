//! US4 state-semantics regression tests (T113).

#![cfg(feature = "simulation")]

use std::sync::Arc;
use std::time::SystemTime;

use swink_agent_eval::JudgeVerdict;
use swink_agent_eval::simulation::{StateRegistry, ToolCallRecord, ToolSchema, ToolSimulator};
use swink_agent_eval::testing::MockJudge;

fn record(tool: &str) -> ToolCallRecord {
    ToolCallRecord {
        tool: tool.into(),
        args: serde_json::json!({}),
        result: serde_json::json!({}),
        timestamp: SystemTime::now(),
    }
}

#[test]
fn shared_state_within_bucket_is_visible_across_two_tools() {
    let reg = StateRegistry::with_history_cap(8);
    reg.with_bucket("shared", |b| {
        b.shared_state = serde_json::json!({"counter": 1});
        b.record(record("tool_a"));
    });
    reg.with_bucket("shared", |b| {
        assert_eq!(b.shared_state["counter"], 1);
        b.record(record("tool_b"));
    });
    let snap = reg.history_snapshot("shared");
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].tool, "tool_a");
    assert_eq!(snap[1].tool, "tool_b");
}

#[test]
fn fifo_eviction_drops_oldest_when_cap_exceeded() {
    let reg = StateRegistry::with_history_cap(2);
    reg.with_bucket("k", |b| {
        b.record(record("first"));
        b.record(record("second"));
        b.record(record("third"));
    });
    let snap = reg.history_snapshot("k");
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].tool, "second");
    assert_eq!(snap[1].tool, "third");
}

#[test]
fn buckets_are_separate_across_state_keys() {
    let reg = StateRegistry::with_history_cap(4);
    reg.with_bucket("alpha", |b| b.record(record("one")));
    reg.with_bucket("beta", |b| {
        b.record(record("two"));
        b.record(record("three"));
    });
    assert_eq!(reg.history_snapshot("alpha").len(), 1);
    assert_eq!(reg.history_snapshot("beta").len(), 2);
    assert_eq!(reg.history_snapshot("alpha")[0].tool, "one");
    assert_eq!(reg.history_snapshot("beta")[0].tool, "two");
}

#[tokio::test]
async fn tool_simulator_emits_schema_validation_error_on_bad_response() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"status": {"type": "string"}},
        "required": ["status"],
    });
    let judge = Arc::new(MockJudge::with_verdicts(vec![JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("{\"status\": 42}".into()),
        label: None,
    }]));
    let sim = ToolSimulator::new(
        vec![ToolSchema::new("status_check", schema)],
        judge,
        "test-model",
    );
    let err = sim
        .invoke("status_check", &serde_json::json!({}), "bucket")
        .await
        .expect_err("schema mismatch must surface an error");
    assert!(err.to_string().contains("schema validation"));
}

#[tokio::test]
async fn tool_simulator_records_valid_invocations_in_bucket() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"],
    });
    let judge = Arc::new(MockJudge::with_verdicts(vec![JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("{\"ok\": true}".into()),
        label: None,
    }]));
    let sim = ToolSimulator::new(vec![ToolSchema::new("probe", schema)], judge, "test-model");
    let out = sim
        .invoke("probe", &serde_json::json!({"q": 1}), "my-bucket")
        .await
        .unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(sim.registry().history_snapshot("my-bucket").len(), 1);
}
