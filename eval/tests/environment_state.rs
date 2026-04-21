//! Integration tests for `EnvironmentStateEvaluator` (spec 023 / US7).
//!
//! Covers AS-7.1 through AS-7.5 plus the "extra captured names are ignored"
//! edge case and the default-registry no-op behavior.

mod common;

use std::sync::Arc;

use swink_agent_eval::{
    EnvironmentState, EnvironmentStateEvaluator, EvalCase, Evaluator, EvaluatorRegistry,
    ExpectedToolCall, StateCapture, Verdict,
};

use common::{case_with_trajectory, mock_invocation};

fn case_with_env_state(expected: Vec<EnvironmentState>, capture: Option<StateCapture>) -> EvalCase {
    EvalCase {
        id: "env-case".to_string(),
        name: "Env Case".to_string(),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["do the thing".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: Some(expected),
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: capture,
    }
}

// AS-7.1: all named states match → Pass with matched names in details.
#[test]
fn all_named_states_match_passes_with_matched_names_in_details() {
    let expected = vec![
        EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        },
        EnvironmentState {
            name: "row_count".into(),
            state: serde_json::json!(3),
        },
    ];
    let capture: StateCapture = Arc::new(|_inv| {
        vec![
            EnvironmentState {
                name: "created_file".into(),
                state: serde_json::json!("out.md"),
            },
            EnvironmentState {
                name: "row_count".into(),
                state: serde_json::json!(3),
            },
        ]
    });
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should produce a result");
    assert_eq!(result.score.verdict(), Verdict::Pass);
    assert_eq!(result.evaluator_name, "environment_state");
    let details = result.details.expect("details present");
    assert!(
        details.contains("created_file"),
        "details should list matched names, got: {details}"
    );
    assert!(
        details.contains("row_count"),
        "details should list matched names, got: {details}"
    );
}

// AS-7.2: missing expected name → Fail identifying the missing name.
#[test]
fn missing_expected_name_fails_with_name_in_details() {
    let expected = vec![
        EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        },
        EnvironmentState {
            name: "row_count".into(),
            state: serde_json::json!(3),
        },
    ];
    let capture: StateCapture = Arc::new(|_inv| {
        vec![EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        }]
    });
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should produce a result");
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.expect("details present");
    assert!(
        details.contains("row_count"),
        "details should identify missing name, got: {details}"
    );
    assert!(
        details.to_lowercase().contains("missing"),
        "details should say it's missing, got: {details}"
    );
}

// AS-7.3: value mismatch → Fail with expected and actual JSON in details.
#[test]
fn value_mismatch_fails_with_expected_and_actual_json() {
    let expected = vec![EnvironmentState {
        name: "row_count".into(),
        state: serde_json::json!(3),
    }];
    let capture: StateCapture = Arc::new(|_inv| {
        vec![EnvironmentState {
            name: "row_count".into(),
            state: serde_json::json!(5),
        }]
    });
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should produce a result");
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.expect("details present");
    assert!(
        details.contains("row_count"),
        "details should identify the offending name, got: {details}"
    );
    assert!(
        details.contains('3'),
        "details should mention expected value, got: {details}"
    );
    assert!(
        details.contains('5'),
        "details should mention actual value, got: {details}"
    );
}

// AS-7.4: case with `expected_environment_state` but no `state_capture` → None;
// eval set continues (other evaluators still run).
#[test]
fn missing_state_capture_returns_none_and_other_evaluators_still_run() {
    // Direct evaluator check: returns None.
    let case = case_with_env_state(
        vec![EnvironmentState {
            name: "x".into(),
            state: serde_json::json!({}),
        }],
        None,
    );
    let invocation = mock_invocation(&["read"], Some("done"), 0.0, 0);
    assert!(
        EnvironmentStateEvaluator
            .evaluate(&case, &invocation)
            .is_none(),
        "evaluator must return None when state_capture is absent",
    );

    // And the registry continues: construct a case with both an
    // expected_trajectory (so trajectory evaluator fires) and an
    // expected_environment_state with no capture. The run should still
    // produce a trajectory result without any environment_state result.
    let mut case = case_with_trajectory(vec![ExpectedToolCall {
        tool_name: "read".into(),
        arguments: None,
    }]);
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "x".into(),
        state: serde_json::json!({}),
    }]);

    let registry = EvaluatorRegistry::with_defaults();
    let results = registry.evaluate(&case, &invocation);
    let names: Vec<_> = results.iter().map(|r| r.evaluator_name.as_str()).collect();
    assert!(
        names.contains(&"trajectory"),
        "trajectory evaluator should still run, got: {names:?}",
    );
    assert!(
        !names.contains(&"environment_state"),
        "environment_state should not produce a result without state_capture, got: {names:?}",
    );
}

// AS-7.5: capture callback panics → Score::fail() with panic message in
// details, no propagation.
#[test]
fn capture_callback_panic_becomes_fail_without_propagating() {
    let expected = vec![EnvironmentState {
        name: "irrelevant".into(),
        state: serde_json::json!({}),
    }];
    let capture: StateCapture = Arc::new(|_inv| panic!("boom"));
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should produce a result on panic");
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.expect("details present");
    assert!(
        details.contains("panicked"),
        "details should mention panic, got: {details}"
    );
    assert!(
        details.contains("boom"),
        "details should include the panic payload, got: {details}"
    );
}

// Edge case: captured snapshot contains extra names not in expected → ignored,
// evaluator still Pass.
#[test]
fn extra_captured_names_are_ignored() {
    let expected = vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }];
    let capture: StateCapture = Arc::new(|_inv| {
        vec![
            EnvironmentState {
                name: "created_file".into(),
                state: serde_json::json!("out.md"),
            },
            EnvironmentState {
                name: "extra".into(),
                state: serde_json::json!({"ignored": true}),
            },
            EnvironmentState {
                name: "another_extra".into(),
                state: serde_json::json!(42),
            },
        ]
    });
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should produce a result");
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

// Registry wiring: EnvironmentStateEvaluator is registered in with_defaults()
// and produces a result when the case configures both expected + capture.
#[test]
fn registered_in_with_defaults() {
    let expected = vec![EnvironmentState {
        name: "ok".into(),
        state: serde_json::json!(true),
    }];
    let capture: StateCapture = Arc::new(|_inv| {
        vec![EnvironmentState {
            name: "ok".into(),
            state: serde_json::json!(true),
        }]
    });
    let case = case_with_env_state(expected, Some(capture));
    let invocation = mock_invocation(&[], None, 0.0, 0);

    let registry = EvaluatorRegistry::with_defaults();
    let results = registry.evaluate(&case, &invocation);
    let env_result = results
        .iter()
        .find(|r| r.evaluator_name == "environment_state")
        .expect("environment_state evaluator should be registered by default");
    assert_eq!(env_result.score.verdict(), Verdict::Pass);
}
