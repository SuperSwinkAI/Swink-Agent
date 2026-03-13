//! Integration tests for trajectory matching evaluator.

mod common;

use swink_agent_eval::{EvalCase, Evaluator, ExpectedToolCall, TrajectoryMatcher, Verdict};

use common::{case_with_trajectory, mock_invocation};

#[test]
fn exact_match_passes() {
    let matcher = TrajectoryMatcher::exact();
    let case = case_with_trajectory(vec![
        ExpectedToolCall {
            tool_name: "read".to_string(),
            arguments: None,
        },
        ExpectedToolCall {
            tool_name: "write".to_string(),
            arguments: None,
        },
    ]);
    let invocation = mock_invocation(&["read", "write"], Some("done"), 0.01, 100);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn exact_match_fails_on_extra_tool() {
    let matcher = TrajectoryMatcher::exact();
    let case = case_with_trajectory(vec![ExpectedToolCall {
        tool_name: "read".to_string(),
        arguments: None,
    }]);
    let invocation = mock_invocation(&["read", "write"], Some("done"), 0.01, 100);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

#[test]
fn in_order_allows_extras() {
    let matcher = TrajectoryMatcher::in_order();
    let case = case_with_trajectory(vec![
        ExpectedToolCall {
            tool_name: "read".to_string(),
            arguments: None,
        },
        ExpectedToolCall {
            tool_name: "write".to_string(),
            arguments: None,
        },
    ]);
    let invocation = mock_invocation(
        &["search", "read", "think", "write"],
        Some("done"),
        0.01,
        100,
    );
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn any_order_ignores_sequence() {
    let matcher = TrajectoryMatcher::any_order();
    let case = case_with_trajectory(vec![
        ExpectedToolCall {
            tool_name: "write".to_string(),
            arguments: None,
        },
        ExpectedToolCall {
            tool_name: "read".to_string(),
            arguments: None,
        },
    ]);
    let invocation = mock_invocation(&["read", "write"], Some("done"), 0.01, 100);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn returns_none_when_no_trajectory_expected() {
    let matcher = TrajectoryMatcher::exact();
    let case = EvalCase {
        id: "no-traj".to_string(),
        name: "No Trajectory".to_string(),
        description: None,
        system_prompt: String::new(),
        user_messages: vec!["hi".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
    };
    let invocation = mock_invocation(&["read"], Some("hello"), 0.0, 0);
    assert!(matcher.evaluate(&case, &invocation).is_none());
}
