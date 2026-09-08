//! Tests for `types`.
#![cfg(test)]

use super::*;

fn base_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: String::new(),
        user_messages: vec!["hi".to_string()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
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

#[test]
fn validate_accepts_unique_environment_state_names() {
    let mut case = base_case("c1");
    case.expected_environment_state = Some(vec![
        EnvironmentState {
            name: "alpha".into(),
            state: serde_json::json!({"v": 1}),
        },
        EnvironmentState {
            name: "beta".into(),
            state: serde_json::json!({"v": 2}),
        },
    ]);
    assert!(validate_eval_case(&case).is_ok());
}

#[test]
fn validate_rejects_duplicate_environment_state_names() {
    let mut case = base_case("dup");
    case.expected_environment_state = Some(vec![
        EnvironmentState {
            name: "alpha".into(),
            state: serde_json::json!({"v": 1}),
        },
        EnvironmentState {
            name: "alpha".into(),
            state: serde_json::json!({"v": 2}),
        },
    ]);
    let err = validate_eval_case(&case).expect_err("duplicate should be rejected");
    match err {
        EvalError::InvalidCase { reason } => {
            assert!(reason.contains("alpha"), "reason: {reason}");
            assert!(reason.contains("dup"), "reason mentions case id: {reason}");
        }
        other => panic!("expected InvalidCase, got {other:?}"),
    }
}

#[test]
fn validate_none_environment_state_is_ok() {
    let case = base_case("none");
    assert!(validate_eval_case(&case).is_ok());
}

#[test]
fn validate_eval_set_propagates_case_errors() {
    let mut case = base_case("bad");
    case.expected_environment_state = Some(vec![
        EnvironmentState {
            name: "x".into(),
            state: serde_json::Value::Null,
        },
        EnvironmentState {
            name: "x".into(),
            state: serde_json::Value::Null,
        },
    ]);
    let set = EvalSet {
        id: "set".into(),
        name: "Set".into(),
        description: None,
        cases: vec![case],
    };
    assert!(validate_eval_set(&set).is_err());
}

#[test]
fn environment_state_serde_round_trip() {
    let state = EnvironmentState {
        name: "db".into(),
        state: serde_json::json!({"rows": 3, "schema": "public"}),
    };
    let json = serde_json::to_string(&state).unwrap();
    let back: EnvironmentState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, state.name);
    assert_eq!(back.state, state.state);
}

#[test]
fn eval_case_serde_round_trip_with_v2_fields() {
    let mut case = base_case("v2");
    case.expected_environment_state = Some(vec![EnvironmentState {
        name: "alpha".into(),
        state: serde_json::json!({"n": 1}),
    }]);
    case.expected_tool_intent = Some(ToolIntent {
        intent: "read config".into(),
        tool_name: Some("read_file".into()),
    });
    case.expected_assertion = Some(Assertion {
        description: "goal completed".into(),
        kind: AssertionKind::GoalCompleted,
    });
    case.expected_interactions = Some(vec![InteractionExpectation {
        from: "planner".into(),
        to: "worker".into(),
        description: "delegates the task".into(),
    }]);
    case.few_shot_examples = vec![FewShotExample {
        input: "hello".into(),
        expected: "world".into(),
        reasoning: Some("example".into()),
    }];
    case.session_id = Some(Uuid::nil());
    case.semantic_tool_selection = true;
    let yaml_like = serde_json::to_string(&case).unwrap();
    let back: EvalCase = serde_json::from_str(&yaml_like).unwrap();
    assert_eq!(back.expected_environment_state.as_ref().unwrap().len(), 1);
    assert_eq!(
        back.expected_tool_intent.as_ref().unwrap().intent,
        "read config"
    );
    assert_eq!(
        back.expected_assertion.as_ref().unwrap().description,
        "goal completed"
    );
    assert_eq!(back.expected_interactions.as_ref().unwrap().len(), 1);
    assert_eq!(back.few_shot_examples.len(), 1);
    assert_eq!(back.session_id, Some(Uuid::nil()));
    assert!(back.semantic_tool_selection);
    assert!(back.attachments.is_empty());
    assert!(back.state_capture.is_none());
}

#[test]
fn case_namespace_matches_oid_derived_value() {
    assert_eq!(
        CASE_NAMESPACE,
        Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case")
    );
}

#[test]
fn default_session_id_is_deterministic_for_same_case() {
    let mut case = base_case("stable");
    case.metadata = serde_json::json!({
        "beta": [2, {"y": true, "x": false}],
        "alpha": {"nested_b": 2, "nested_a": 1}
    });
    case.expected_response = Some(ResponseCriteria::Contains {
        substring: "ok".into(),
    });
    case.expected_trajectory = Some(vec![ExpectedToolCall {
        tool_name: "read_file".into(),
        arguments: Some(serde_json::json!({"path": "./project-alpha/config.toml"})),
    }]);

    let first = case.default_session_id();
    let second = case.default_session_id();
    assert_eq!(first, second);
}

#[test]
fn default_session_id_is_stable_across_json_key_order() {
    let mut left = base_case("ordered");
    left.metadata = serde_json::json!({
        "alpha": {"x": 1, "y": 2},
        "beta": [3, 4]
    });
    left.expected_environment_state = Some(vec![EnvironmentState {
        name: "workspace".into(),
        state: serde_json::json!({"files": {"b": 2, "a": 1}}),
    }]);

    let mut right = left.clone();
    right.metadata = serde_json::from_str(r#"{"beta":[3,4],"alpha":{"y":2,"x":1}}"#)
        .expect("valid metadata json");
    right.expected_environment_state = Some(vec![EnvironmentState {
        name: "workspace".into(),
        state: serde_json::from_str(r#"{"files":{"a":1,"b":2}}"#).expect("valid state json"),
    }]);

    assert_eq!(left.default_session_id(), right.default_session_id());
}

#[test]
fn default_session_id_changes_when_case_content_changes() {
    let mut case = base_case("mutates");
    let original = case.default_session_id();
    case.user_messages.push("follow-up".into());
    assert_ne!(original, case.default_session_id());
}

#[test]
fn builders_cover_every_field() {
    let case = EvalCase::new("b1", "Builder case", "sys", vec!["hi".to_string()])
        .with_description("a description")
        .with_budget(BudgetConstraints::default().with_max_cost(2.0))
        .with_semantic_tool_selection(true);
    assert_eq!(case.description.as_deref(), Some("a description"));
    assert_eq!(
        case.budget.as_ref().and_then(|budget| budget.max_cost),
        Some(2.0)
    );
    assert!(case.semantic_tool_selection);

    let budget = BudgetConstraints::default().with_max_cost(1.0);
    assert_eq!(budget.max_cost, Some(1.0));
}
