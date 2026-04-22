mod common;

use std::sync::Arc;

use swink_agent_eval::{
    EnvironmentState, EnvironmentStateEvaluator, EvalCase, Evaluator, EvaluatorRegistry, Score,
};

use common::mock_invocation_with_response;

fn base_case() -> EvalCase {
    EvalCase {
        id: "env-state".into(),
        name: "Environment state".into(),
        description: None,
        system_prompt: "test".into(),
        user_messages: vec!["write out.md".into()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

#[test]
fn all_named_states_match_passes() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![
        EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        },
        EnvironmentState {
            name: "row_count".into(),
            state: serde_json::json!(3),
        },
    ]);
    case.state_capture = Some(Arc::new(|_| {
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
    }));

    let invocation = mock_invocation_with_response(&[], "done");
    let result = EnvironmentStateEvaluator
        .evaluate(&case, &invocation)
        .expect("evaluator should apply");

    assert!(result.score.verdict().is_pass());
    let details = result.details.expect("details should be present");
    assert!(details.contains("created_file"), "details: {details}");
    assert!(details.contains("row_count"), "details: {details}");
}

#[test]
fn missing_expected_name_fails() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }]);
    case.state_capture = Some(Arc::new(|_| {
        vec![EnvironmentState {
            name: "different_state".into(),
            state: serde_json::json!("out.md"),
        }]
    }));

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &mock_invocation_with_response(&[], "done"))
        .expect("evaluator should apply");

    assert_eq!(result.score.value, Score::fail().value);
    let details = result.details.expect("details should be present");
    assert!(
        details.contains("missing expected environment state `created_file`"),
        "details: {details}"
    );
}

#[test]
fn mismatched_value_fails_with_expected_and_actual() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "row_count".into(),
        state: serde_json::json!(3),
    }]);
    case.state_capture = Some(Arc::new(|_| {
        vec![EnvironmentState {
            name: "row_count".into(),
            state: serde_json::json!(2),
        }]
    }));

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &mock_invocation_with_response(&[], "done"))
        .expect("evaluator should apply");

    assert_eq!(result.score.value, Score::fail().value);
    let details = result.details.expect("details should be present");
    assert!(details.contains("row_count"), "details: {details}");
    assert!(details.contains("expected 3"), "details: {details}");
    assert!(details.contains("actual 2"), "details: {details}");
}

#[test]
fn missing_state_capture_returns_none() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }]);

    assert!(
        EnvironmentStateEvaluator
            .evaluate(&case, &mock_invocation_with_response(&[], "done"))
            .is_none()
    );
}

#[test]
fn state_capture_panic_becomes_failure() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }]);
    case.state_capture = Some(Arc::new(|_| {
        panic!("capture exploded");
    }));

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &mock_invocation_with_response(&[], "done"))
        .expect("evaluator should apply");

    assert_eq!(result.score.value, Score::fail().value);
    let details = result.details.expect("details should be present");
    assert!(
        details.contains("state capture panicked"),
        "details: {details}"
    );
    assert!(details.contains("capture exploded"), "details: {details}");
}

#[test]
fn extra_captured_states_are_ignored() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }]);
    case.state_capture = Some(Arc::new(|_| {
        vec![
            EnvironmentState {
                name: "created_file".into(),
                state: serde_json::json!("out.md"),
            },
            EnvironmentState {
                name: "ignored_extra".into(),
                state: serde_json::json!({"anything": true}),
            },
        ]
    }));

    let result = EnvironmentStateEvaluator
        .evaluate(&case, &mock_invocation_with_response(&[], "done"))
        .expect("evaluator should apply");

    assert!(result.score.verdict().is_pass());
}

#[test]
fn with_defaults_registers_environment_state_evaluator() {
    let mut case = base_case();
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("out.md"),
    }]);
    case.state_capture = Some(Arc::new(|_| {
        vec![EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        }]
    }));

    let results = EvaluatorRegistry::with_defaults()
        .evaluate(&case, &mock_invocation_with_response(&[], "done"));

    assert!(results.iter().any(|result| {
        result.evaluator_name == "environment_state" && result.score.verdict().is_pass()
    }));
}
