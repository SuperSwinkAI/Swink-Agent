//! Integration tests for trajectory matching evaluator.

mod common;

use swink_agent_eval::{EvalCase, Evaluator, ExpectedToolCall, TrajectoryMatcher, Verdict};

use common::{case_with_trajectory, mock_invocation, mock_invocation_multi_turn};

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

// ── Spec 023 Acceptance Tests (US2) ──────────────────────────────────────────

/// AS-2.1: Exact match — all steps matched, score 1.0.
#[test]
fn us2_exact_match_all_steps() {
    let matcher = TrajectoryMatcher::exact();
    let case = case_with_trajectory(vec![
        ExpectedToolCall { tool_name: "read".into(), arguments: None },
        ExpectedToolCall { tool_name: "write".into(), arguments: None },
        ExpectedToolCall { tool_name: "deploy".into(), arguments: None },
    ]);
    let invocation = mock_invocation(&["read", "write", "deploy"], None, 0.0, 0);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

/// AS-2.2: Missing steps identified — score < 1.0.
#[test]
fn us2_missing_steps_identified() {
    let matcher = TrajectoryMatcher::in_order();
    let case = case_with_trajectory(vec![
        ExpectedToolCall { tool_name: "read".into(), arguments: None },
        ExpectedToolCall { tool_name: "write".into(), arguments: None },
        ExpectedToolCall { tool_name: "deploy".into(), arguments: None },
    ]);
    // Only "read" and "deploy" present, "write" missing — but InOrder requires order,
    // so only "read" matches (deploy can't match before write in order)
    let invocation = mock_invocation(&["read", "deploy"], None, 0.0, 0);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert!(result.score.value < 1.0, "expected partial match, got {}", result.score.value);
}

/// AS-2.3: Extra (unexpected) steps — `Exact` fails, `InOrder` passes.
#[test]
fn us2_extra_steps_exact_fails_inorder_passes() {
    let case = case_with_trajectory(vec![
        ExpectedToolCall { tool_name: "read".into(), arguments: None },
        ExpectedToolCall { tool_name: "write".into(), arguments: None },
    ]);
    let invocation = mock_invocation(&["search", "read", "think", "write"], None, 0.0, 0);

    let exact = TrajectoryMatcher::exact();
    let result = exact.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail, "Exact should fail with extras");

    let in_order = TrajectoryMatcher::in_order();
    let result = in_order.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass, "InOrder should pass with extras");
}

/// AS-2.4: Ordering deviation — `Exact` fails (wrong order), `AnyOrder` passes.
#[test]
fn us2_ordering_deviation_exact_fails_anyorder_passes() {
    let case = case_with_trajectory(vec![
        ExpectedToolCall { tool_name: "read".into(), arguments: None },
        ExpectedToolCall { tool_name: "write".into(), arguments: None },
    ]);
    let invocation = mock_invocation(&["write", "read"], None, 0.0, 0);

    let exact = TrajectoryMatcher::exact();
    let result = exact.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail, "Exact should fail on wrong order");

    let any = TrajectoryMatcher::any_order();
    let result = any.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass, "AnyOrder should pass regardless of order");
}

/// Edge case: Empty golden path — `InOrder`/`AnyOrder` return pass (vacuous truth).
#[test]
fn us2_empty_golden_path_vacuous_truth() {
    let case = case_with_trajectory(vec![]);
    let invocation = mock_invocation(&["read", "write"], None, 0.0, 0);

    let in_order = TrajectoryMatcher::in_order();
    let result = in_order.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);

    let any = TrajectoryMatcher::any_order();
    let result = any.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

/// Edge case: Empty golden path — Exact fails when actual has steps.
#[test]
fn us2_empty_golden_path_exact_fails() {
    let case = case_with_trajectory(vec![]);
    let invocation = mock_invocation(&["read"], None, 0.0, 0);

    let exact = TrajectoryMatcher::exact();
    let result = exact.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

/// Edge case: `ExpectedToolCall` with arguments requires JSON equality.
#[test]
fn us2_arguments_matching_exact_json() {
    let matcher = TrajectoryMatcher::exact();

    // With arguments: Some — must match exactly
    let case = case_with_trajectory(vec![ExpectedToolCall {
        tool_name: "read".into(),
        arguments: Some(serde_json::json!({"file": "a.txt"})),
    }]);

    // Matching arguments
    let invocation = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({"file": "a.txt"}))],
    ]);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);

    // Non-matching arguments
    let invocation = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({"file": "b.txt"}))],
    ]);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
}

/// Edge case: `ExpectedToolCall` with arguments: `None` matches by name only.
#[test]
fn us2_arguments_none_matches_by_name_only() {
    let matcher = TrajectoryMatcher::exact();
    let case = case_with_trajectory(vec![ExpectedToolCall {
        tool_name: "read".into(),
        arguments: None,
    }]);
    let invocation = mock_invocation_multi_turn(&[
        &[("read", serde_json::json!({"file": "anything.txt"}))],
    ]);
    let result = matcher.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}
