use std::path::PathBuf;

use swink_agent_eval::{
    Assertion, AssertionKind, Attachment, EvalCase, EvalSet, FewShotExample,
    InteractionExpectation, validate_eval_case, validate_eval_set,
};

fn base_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "system".to_string(),
        user_messages: vec!["hello".to_string()],
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
fn validate_accepts_extended_v043_fields() {
    let mut case = base_case("valid");
    case.expected_assertion = Some(Assertion {
        description: "user issue is resolved".into(),
        kind: AssertionKind::ToolInvoked("lookup_order".into()),
    });
    case.expected_interactions = Some(vec![InteractionExpectation {
        from: "planner".into(),
        to: "tool".into(),
        description: "delegates lookup".into(),
    }]);
    case.few_shot_examples = vec![FewShotExample {
        input: "Where is my order?".into(),
        expected: "I can help track that.".into(),
        reasoning: Some("Establishes the expected tone.".into()),
    }];
    case.attachments = vec![
        Attachment::Path(PathBuf::from("fixtures/order.png")),
        Attachment::Base64 {
            mime: "image/png".into(),
            bytes: vec![1, 2, 3],
        },
        Attachment::Url("https://example.com/order.png".into()),
    ];

    validate_eval_case(&case).expect("extended fields should validate");
}

#[test]
fn validate_rejects_blank_assertion_tool_name() {
    let mut case = base_case("blank-tool");
    case.expected_assertion = Some(Assertion {
        description: "must invoke a tool".into(),
        kind: AssertionKind::ToolInvoked("   ".into()),
    });

    let err = validate_eval_case(&case).expect_err("blank tool name must be rejected");
    assert!(
        err.to_string()
            .contains("expected_assertion.kind.tool_name")
    );
}

#[test]
fn validate_rejects_malformed_attachment_path_and_url() {
    let mut path_case = base_case("bad-path");
    path_case.attachments = vec![Attachment::Path(PathBuf::from("../escape.png"))];
    let err = validate_eval_case(&path_case).expect_err("parent traversal path must be rejected");
    assert!(err.to_string().contains("attachments[0] path"));

    let mut url_case = base_case("bad-url");
    url_case.attachments = vec![Attachment::Url("http://example.com/image.png".into())];
    let err = validate_eval_case(&url_case).expect_err("http URL must be rejected");
    assert!(err.to_string().contains("URL must use https"));
}

#[test]
fn validate_rejects_duplicate_case_ids_within_eval_set() {
    let set = EvalSet {
        id: "set".into(),
        name: "Set".into(),
        description: None,
        cases: vec![base_case("dup"), base_case("dup")],
    };

    let err = validate_eval_set(&set).expect_err("duplicate case IDs must be rejected");
    assert!(err.to_string().contains("duplicate case id `dup`"));
}

#[test]
fn deterministic_session_id_tracks_new_case_fields() {
    let mut left = base_case("session");
    left.expected_assertion = Some(Assertion {
        description: "complete the task".into(),
        kind: AssertionKind::GoalCompleted,
    });
    left.expected_interactions = Some(vec![InteractionExpectation {
        from: "agent".into(),
        to: "reviewer".into(),
        description: "hands off the result".into(),
    }]);
    left.few_shot_examples = vec![FewShotExample {
        input: "draft".into(),
        expected: "reviewed".into(),
        reasoning: None,
    }];

    let mut right = left.clone();
    assert_eq!(left.default_session_id(), right.default_session_id());

    right.few_shot_examples[0].expected = "approved".into();
    assert_ne!(left.default_session_id(), right.default_session_id());
}
