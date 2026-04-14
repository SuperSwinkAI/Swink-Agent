//! Integration tests for eval store persistence.

use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    EvalCase, EvalCaseResult, EvalError, EvalMetricResult, EvalSet, EvalSetResult, EvalStore,
    EvalSummary, FsEvalStore, Invocation, Score, Verdict,
};

fn sample_eval_set() -> EvalSet {
    EvalSet {
        id: "test-set".to_string(),
        name: "Test Set".to_string(),
        description: Some("A test eval set".to_string()),
        cases: vec![EvalCase {
            id: "case-1".to_string(),
            name: "Case 1".to_string(),
            description: None,
            system_prompt: "test".to_string(),
            user_messages: vec!["hello".to_string()],
            expected_trajectory: None,
            expected_response: None,
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
        }],
    }
}

fn sample_result() -> EvalSetResult {
    EvalSetResult {
        eval_set_id: "test-set".to_string(),
        case_results: vec![EvalCaseResult {
            case_id: "case-1".to_string(),
            invocation: Invocation {
                turns: vec![],
                total_usage: Usage::default(),
                total_cost: Cost::default(),
                total_duration: Duration::from_millis(50),
                final_response: Some("hello".to_string()),
                stop_reason: StopReason::Stop,
                model: ModelSpec::new("test", "test-model"),
            },
            metric_results: vec![EvalMetricResult {
                evaluator_name: "budget".to_string(),
                score: Score::pass(),
                details: Some("all good".to_string()),
            }],
            verdict: Verdict::Pass,
        }],
        summary: EvalSummary {
            total_cases: 1,
            passed: 1,
            failed: 0,
            total_cost: Cost::default(),
            total_usage: Usage::default(),
            total_duration: Duration::from_millis(50),
        },
        timestamp: 1_000_000,
    }
}

#[test]
fn eval_set_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    let set = sample_eval_set();
    store.save_set(&set).unwrap();

    let loaded = store.load_set("test-set").unwrap();
    assert_eq!(loaded.id, set.id);
    assert_eq!(loaded.cases.len(), 1);
}

#[test]
fn eval_result_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    let result = sample_result();
    store.save_result(&result).unwrap();

    let loaded = store.load_result("test-set", 1_000_000).unwrap();
    assert_eq!(loaded.eval_set_id, "test-set");
    assert_eq!(loaded.case_results.len(), 1);
    assert_eq!(loaded.case_results[0].verdict, Verdict::Pass);
}

#[test]
fn list_results_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    let mut r1 = sample_result();
    r1.timestamp = 3_000;
    store.save_result(&r1).unwrap();

    let mut r2 = sample_result();
    r2.timestamp = 1_000;
    store.save_result(&r2).unwrap();

    let mut r3 = sample_result();
    r3.timestamp = 2_000;
    store.save_result(&r3).unwrap();

    let timestamps = store.list_results("test-set").unwrap();
    assert_eq!(timestamps, vec![1_000, 2_000, 3_000]);
}

#[test]
fn load_missing_set_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());
    assert!(store.load_set("nonexistent").is_err());
}

#[test]
fn load_missing_result_returns_result_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    let error = store.load_result("test-set", 1_000_000).unwrap_err();
    assert!(matches!(
        error,
        EvalError::ResultNotFound {
            eval_set_id,
            timestamp
        } if eval_set_id == "test-set" && timestamp == 1_000_000
    ));
}

#[test]
fn list_results_empty_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());
    let timestamps = store.list_results("nonexistent").unwrap();
    assert!(timestamps.is_empty());
}
