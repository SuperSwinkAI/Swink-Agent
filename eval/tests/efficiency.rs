//! Integration tests for efficiency evaluator.

mod common;

use swink_agent_eval::{
    BudgetConstraints, EfficiencyEvaluator, Evaluator, EvaluatorRegistry, Verdict,
};

use common::{case_with_budget, mock_invocation_multi_turn};

#[test]
fn all_unique_passes() {
    let eval = EfficiencyEvaluator::new();
    let invocation = mock_invocation_multi_turn(&[&[
        ("read", serde_json::json!({"file": "a.rs"})),
        ("write", serde_json::json!({"file": "b.rs"})),
    ]]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    let result = eval.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn duplicates_reduce_score() {
    let eval = EfficiencyEvaluator::new();
    let invocation = mock_invocation_multi_turn(&[
        &[
            ("read", serde_json::json!({"file": "a.rs"})),
            ("read", serde_json::json!({"file": "a.rs"})),
        ],
        &[
            ("read", serde_json::json!({"file": "a.rs"})),
            ("read", serde_json::json!({"file": "a.rs"})),
        ],
    ]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    let result = eval.evaluate(&case, &invocation).unwrap();
    // 1 unique / 4 total → dup_ratio = 0.25
    // ideal = max(1, 1) = 1, actual = 2 → step_ratio = 0.5
    // composite = 0.6*0.25 + 0.4*0.5 = 0.35
    assert!(result.score.value < 0.5);
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

#[test]
fn in_default_registry() {
    let registry = EvaluatorRegistry::with_defaults();
    // Verify the efficiency evaluator is present by running it on a case with tool calls.
    let invocation = mock_invocation_multi_turn(&[&[("read", serde_json::json!({}))]]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    let results = registry.evaluate(&case, &invocation);
    let names: Vec<_> = results.iter().map(|r| r.evaluator_name.as_str()).collect();
    assert!(
        names.contains(&"efficiency"),
        "expected efficiency evaluator in defaults, got: {names:?}"
    );
}
