//! Integration tests for eval store persistence.

use std::time::Duration;

use swink_agent::{ModelSpec, StopReason};
use swink_agent_eval::{
    EvalCase, EvalCaseResult, EvalError, EvalMetricResult, EvalSet, EvalSetResult, EvalStore,
    EvalSummary, FsEvalStore, Invocation, Score, Verdict,
};

fn sample_eval_set() -> EvalSet {
    EvalSet::new(
        "test-set",
        "Test Set",
        vec![EvalCase::new(
            "case-1",
            "Case 1",
            "test",
            vec!["hello".to_string()],
        )],
    )
    .with_description("A test eval set")
}

fn sample_result() -> EvalSetResult {
    EvalSetResult::new(
        "test-set",
        vec![
            EvalCaseResult::new(
                "case-1",
                Invocation::new(StopReason::Stop, ModelSpec::new("test", "test-model"))
                    .with_total_duration(Duration::from_millis(50))
                    .with_final_response("hello"),
                Verdict::Pass,
            )
            .with_metric_results(vec![
                EvalMetricResult::new("budget", Score::pass()).with_details("all good"),
            ]),
        ],
        EvalSummary::default()
            .with_total_cases(1)
            .with_passed(1)
            .with_total_duration(Duration::from_millis(50)),
        1_000_000,
    )
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

#[test]
fn save_set_rejects_invalid_identifier() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    for invalid_id in ["", "..", "nested/id", r"nested\id", "nul\0byte"] {
        let mut set = sample_eval_set();
        set.id = invalid_id.to_string();

        assert!(
            matches!(
                store.save_set(&set),
                Err(EvalError::InvalidIdentifier { kind, id })
                    if kind == "eval set" && id == invalid_id
            ),
            "expected invalid eval set id to be rejected: {invalid_id:?}"
        );
        assert!(
            matches!(
                store.load_set(invalid_id),
                Err(EvalError::InvalidIdentifier { kind, id })
                    if kind == "eval set" && id == invalid_id
            ),
            "expected invalid eval set id to be rejected on load: {invalid_id:?}"
        );
    }
}

#[test]
fn result_operations_reject_invalid_eval_set_identifier() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsEvalStore::new(dir.path());

    for invalid_id in ["", "..", "nested/id", r"nested\id", "nul\0byte"] {
        let mut result = sample_result();
        result.eval_set_id = invalid_id.to_string();

        assert!(
            matches!(
                store.save_result(&result),
                Err(EvalError::InvalidIdentifier { kind, id })
                    if kind == "eval result set" && id == invalid_id
            ),
            "expected invalid eval result set id to be rejected on save: {invalid_id:?}"
        );
        assert!(
            matches!(
                store.load_result(invalid_id, 1_000_000),
                Err(EvalError::InvalidIdentifier { kind, id })
                    if kind == "eval result set" && id == invalid_id
            ),
            "expected invalid eval result set id to be rejected on load: {invalid_id:?}"
        );
        assert!(
            matches!(
                store.list_results(invalid_id),
                Err(EvalError::InvalidIdentifier { kind, id })
                    if kind == "eval result set" && id == invalid_id
            ),
            "expected invalid eval result set id to be rejected when listing: {invalid_id:?}"
        );
    }
}
