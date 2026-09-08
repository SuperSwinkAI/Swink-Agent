//! Tests for `evaluator`.
#![cfg(test)]

use super::*;

use crate::testing::MockJudge;
use crate::types::BudgetConstraints;
use swink_agent::{Cost, ModelSpec, StopReason, Usage};

fn case_with_budget(budget: BudgetConstraints) -> EvalCase {
    EvalCase {
        id: "c1".into(),
        name: "C1".into(),
        description: None,
        system_prompt: "sp".into(),
        user_messages: vec!["hi".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: Some(budget),
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn empty_invocation() -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: std::time::Duration::from_secs(0),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

/// FR-048: the default `evaluate_async` must reproduce whatever the
/// blocking `evaluate` returns, for an evaluator that doesn't override it.
#[tokio::test]
async fn evaluate_async_default_matches_blocking_evaluate() {
    let evaluator = crate::budget::BudgetEvaluator;
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(1.0),
        max_input: None,
        max_output: None,
        max_turns: None,
    });
    let invocation = empty_invocation();

    let sync_result = evaluator.evaluate(&case, &invocation);
    let async_result = evaluator.evaluate_async(&case, &invocation).await;

    assert_eq!(
        sync_result.map(|r| r.score.value),
        async_result.map(|r| r.score.value)
    );
}

#[test]
fn with_defaults_has_no_judge() {
    let registry = EvaluatorRegistry::with_defaults();
    assert!(registry.judge().is_none());
}

#[test]
fn with_judge_stores_client() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let registry = EvaluatorRegistry::with_judge(judge);
    assert!(registry.judge().is_some());
}

#[test]
fn with_defaults_and_judge_has_defaults_plus_judge() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let registry = EvaluatorRegistry::with_defaults_and_judge(judge);
    assert!(registry.judge().is_some());
    // environment_state) + Phase 9 semantic_tool_selection
    // + Phase 10 semantic_tool_parameter = 7 evaluators.
    assert_eq!(registry.evaluators.len(), 7);
    assert!(
        registry
            .evaluators
            .iter()
            .any(|e| e.name() == "semantic_tool_selection"),
        "semantic_tool_selection should be registered"
    );
    assert!(
        registry
            .evaluators
            .iter()
            .any(|e| e.name() == "semantic_tool_parameter"),
        "semantic_tool_parameter should be registered"
    );
}

#[test]
fn with_judge_registers_semantic_evaluators() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let registry = EvaluatorRegistry::with_judge(judge);
    assert!(registry.judge().is_some());
    assert_eq!(registry.evaluators.len(), 2);
    let names: Vec<&str> = registry.evaluators.iter().map(|e| e.name()).collect();
    assert!(names.contains(&"semantic_tool_selection"));
    assert!(names.contains(&"semantic_tool_parameter"));
}

#[test]
fn with_defaults_does_not_register_semantic_evaluators() {
    let registry = EvaluatorRegistry::with_defaults();
    assert!(registry.judge().is_none());
    assert!(
        registry
            .evaluators
            .iter()
            .all(|e| e.name() != "semantic_tool_selection"),
        "semantic_tool_selection must NOT be in with_defaults()"
    );
    assert!(
        registry
            .evaluators
            .iter()
            .all(|e| e.name() != "semantic_tool_parameter"),
        "semantic_tool_parameter must NOT be in with_defaults()"
    );
}

#[test]
fn add_rejects_duplicate_evaluator_names() {
    let mut registry = EvaluatorRegistry::new();
    registry
        .add(crate::match_::TrajectoryMatcher::in_order())
        .expect("first registration should succeed");

    let err = registry
        .add(crate::match_::TrajectoryMatcher::in_order())
        .expect_err("duplicate evaluator names must be rejected");

    match err {
        EvalError::DuplicateEvaluator { name } => assert_eq!(name, "trajectory"),
        other => panic!("expected DuplicateEvaluator, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolate_panic_uses_tokio_runtime_when_available() {
    let result = isolate_panic("panics", || -> Option<EvalMetricResult> {
        panic!("deliberate runtime panic");
    })
    .expect("panic isolation should emit a failure metric");

    assert_eq!(result.evaluator_name, "panics");
    assert_eq!(result.score.verdict(), Score::fail().verdict());
    assert!(
        result
            .details
            .as_deref()
            .is_some_and(|details| details.contains("deliberate runtime panic")),
        "panic metric should preserve the runtime panic message"
    );
}
