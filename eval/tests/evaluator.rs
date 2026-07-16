//! Tests for `EvaluatorRegistry` composition, filtering, and defaults.

mod common;

use swink_agent_eval::{
    EvalCase, EvalError, EvalMetricResult, Evaluator, EvaluatorRegistry, Invocation, Score,
};

fn minimal_case() -> EvalCase {
    EvalCase::new("test", "Test", "test", vec!["test".to_string()])
}

struct AlwaysPass;

impl Evaluator for AlwaysPass {
    fn name(&self) -> &'static str {
        "always_pass"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        Some(EvalMetricResult::new("always_pass", Score::pass()))
    }
}

struct AlwaysFail;

impl Evaluator for AlwaysFail {
    fn name(&self) -> &'static str {
        "always_fail"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        Some(EvalMetricResult::new("always_fail", Score::fail()).with_details("forced failure"))
    }
}

struct ReturnsNone;

impl Evaluator for ReturnsNone {
    fn name(&self) -> &'static str {
        "returns_none"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        None
    }
}

struct Panics;

impl Evaluator for Panics {
    fn name(&self) -> &'static str {
        "panics"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        panic!("deliberate evaluator panic");
    }
}

#[test]
fn registry_with_defaults_has_four_evaluators() {
    let registry = EvaluatorRegistry::with_defaults();
    let invocation = common::mock_invocation(&["read", "write"], Some("hello"), 0.01, 500);
    let case = common::case_with_trajectory(vec![swink_agent_eval::ExpectedToolCall::new(
        "read",
    )]);
    let results = registry.evaluate(&case, &invocation);
    // with_defaults registers: trajectory, budget, response, efficiency
    // trajectory applies (has expected_trajectory), efficiency applies (has tool calls)
    // budget does not apply (no budget constraints), response does not apply (no expected_response)
    assert_eq!(
        results.len(),
        2,
        "trajectory + efficiency should produce results"
    );
    let names: Vec<_> = results.iter().map(|r| r.evaluator_name.as_str()).collect();
    assert!(names.contains(&"trajectory"));
    assert!(names.contains(&"efficiency"));
}

#[test]
fn custom_evaluator_alongside_defaults() {
    let mut registry = EvaluatorRegistry::with_defaults();
    registry.register(AlwaysPass);
    let invocation = common::mock_invocation(&[], Some("hello"), 0.01, 500);
    let case = minimal_case();
    let results = registry.evaluate(&case, &invocation);
    assert!(results.iter().any(|r| r.evaluator_name == "always_pass"));
}

#[test]
fn evaluator_returning_none_excluded() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);
    registry.register(ReturnsNone);
    registry.register(AlwaysFail);

    let invocation = common::mock_invocation(&[], None, 0.0, 0);
    let case = minimal_case();
    let results = registry.evaluate(&case, &invocation);

    assert_eq!(results.len(), 2);
    let names: Vec<_> = results.iter().map(|r| r.evaluator_name.as_str()).collect();
    assert!(names.contains(&"always_pass"));
    assert!(names.contains(&"always_fail"));
    assert!(!names.contains(&"returns_none"));
}

#[test]
fn panicking_evaluator_becomes_failure_and_other_evaluators_continue() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(Panics);
    registry.register(AlwaysPass);

    let invocation = common::mock_invocation(&[], None, 0.0, 0);
    let case = minimal_case();
    let results = registry.evaluate(&case, &invocation);

    assert_eq!(results.len(), 2);

    let panic_metric = results
        .iter()
        .find(|result| result.evaluator_name == "panics")
        .expect("panicking evaluator should produce a failure metric");
    assert_eq!(panic_metric.score.verdict(), Score::fail().verdict());
    assert!(
        panic_metric
            .details
            .as_deref()
            .is_some_and(|details| details.contains("deliberate evaluator panic")),
        "panic metric should preserve the panic message"
    );

    assert!(
        results
            .iter()
            .any(|result| result.evaluator_name == "always_pass"),
        "later evaluators should still run after a panic"
    );
}

#[test]
fn case_evaluator_filter_restricts_which_run() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);
    registry.register(AlwaysFail);
    registry.register(ReturnsNone);

    let invocation = common::mock_invocation(&[], None, 0.0, 0);
    let mut case = minimal_case();
    case.evaluators = vec!["always_pass".to_string()];

    let results = registry.evaluate(&case, &invocation);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].evaluator_name, "always_pass");
}

#[test]
fn empty_registry_returns_empty_results() {
    let registry = EvaluatorRegistry::new();
    let invocation = common::mock_invocation(&[], None, 0.0, 0);
    let case = minimal_case();
    let results = registry.evaluate(&case, &invocation);
    assert!(results.is_empty());
}

#[test]
fn closure_evaluator_works() {
    let mut registry = EvaluatorRegistry::new();
    registry.register((
        "my_closure",
        |_case: &EvalCase, _inv: &Invocation| -> Option<EvalMetricResult> {
            Some(EvalMetricResult::new("my_closure", Score::new(0.75, 0.5)).with_details("closure evaluator"))
        },
    ));

    let invocation = common::mock_invocation(&[], None, 0.0, 0);
    let case = minimal_case();
    let results = registry.evaluate(&case, &invocation);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].evaluator_name, "my_closure");
    assert!((results[0].score.value - 0.75).abs() < f64::EPSILON);
}

#[test]
fn duplicate_names_return_structured_error_from_add() {
    let mut registry = EvaluatorRegistry::new();
    registry
        .add(AlwaysPass)
        .expect("first registration should succeed");

    let err = registry
        .add((
            "always_pass",
            |_case: &EvalCase, _inv: &Invocation| -> Option<EvalMetricResult> { None },
        ))
        .expect_err("duplicate evaluator names must be rejected");

    match err {
        EvalError::DuplicateEvaluator { name } => assert_eq!(name, "always_pass"),
        other => panic!("expected DuplicateEvaluator, got {other:?}"),
    }
}
