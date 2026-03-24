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

// ── Spec 023 Acceptance Tests (US3) ──────────────────────────────────────────

/// AS-3.1: Perfect efficiency — no duplicates, ideal turns → score 1.0.
#[test]
fn us3_perfect_efficiency_score_1() {
    let eval = EfficiencyEvaluator::new();
    let invocation = mock_invocation_multi_turn(&[&[
        ("read", serde_json::json!({"file": "a.rs"})),
        ("write", serde_json::json!({"file": "b.rs"})),
    ]]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: Some(1),
        max_duration: None,
    });
    let result = eval.evaluate(&case, &invocation).unwrap();
    assert!(
        (result.score.value - 1.0).abs() < f64::EPSILON,
        "expected 1.0, got {}",
        result.score.value
    );
}

/// AS-3.2: 50% duplicates + 2x expected turns → weighted penalty.
#[test]
fn us3_half_duplicates_double_turns() {
    let eval = EfficiencyEvaluator::new();
    // 2 turns with same call = 2 total, 1 unique → dup_ratio = 0.5
    let invocation = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({"file": "a.rs"}))],
        &[("read", serde_json::json!({"file": "a.rs"}))],
    ]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: Some(1), // ideal = 1, actual = 2
        max_duration: None,
    });
    let result = eval.evaluate(&case, &invocation).unwrap();
    // 0.6 * 0.5 + 0.4 * (1/2) = 0.3 + 0.2 = 0.5
    assert!(
        (result.score.value - 0.5).abs() < 0.01,
        "expected ~0.5, got {}",
        result.score.value
    );
}

/// AS-3.3: Empty trajectory (zero tool calls) → returns None.
#[test]
fn us3_empty_trajectory_returns_none() {
    let eval = EfficiencyEvaluator::new();
    let invocation = mock_invocation_multi_turn(&[&[]]);
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    assert!(eval.evaluate(&case, &invocation).is_none());
}

/// AS-3.4: More efficient trajectory scores higher.
#[test]
fn us3_more_efficient_scores_higher() {
    let eval = EfficiencyEvaluator::new();
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: Some(1),
        max_duration: None,
    });

    // Efficient: 1 turn, 2 unique calls
    let efficient = mock_invocation_multi_turn(&[&[
        ("read", serde_json::json!({})),
        ("write", serde_json::json!({})),
    ]]);
    let score_efficient = eval.evaluate(&case, &efficient).unwrap().score.value;

    // Inefficient: 3 turns, 1 unique call repeated 3 times
    let inefficient = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({}))],
        &[("read", serde_json::json!({}))],
        &[("read", serde_json::json!({}))],
    ]);
    let score_inefficient = eval.evaluate(&case, &inefficient).unwrap().score.value;

    assert!(
        score_efficient > score_inefficient,
        "efficient ({score_efficient}) should score higher than inefficient ({score_inefficient})"
    );
}

/// Edge case: `ideal_turns` uses `budget.max_turns` when set.
#[test]
fn us3_ideal_turns_from_budget() {
    let eval = EfficiencyEvaluator::new();
    // 2 unique calls across 4 turns
    let invocation = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({}))],
        &[("write", serde_json::json!({}))],
        &[("read", serde_json::json!({"file": "c.rs"}))],
        &[("write", serde_json::json!({"file": "d.rs"}))],
    ]);

    // With budget max_turns = 2
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: None,
        max_turns: Some(2),
        max_duration: None,
    });
    let result = eval.evaluate(&case, &invocation).unwrap();
    // 4 unique / 4 total → dup_ratio = 1.0
    // ideal = 2, actual = 4 → step_ratio = 2/4 = 0.5
    // composite = 0.6*1.0 + 0.4*0.5 = 0.8
    assert!(
        (result.score.value - 0.8).abs() < 0.01,
        "expected ~0.8, got {}",
        result.score.value
    );
}
