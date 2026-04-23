//! Regression tests for the Simple family evaluators (T074).

#![cfg(feature = "evaluator-simple")]

use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    EvalCase, Evaluator, ExactMatchEvaluator, Invocation, LevenshteinDistanceEvaluator,
};

fn make_case() -> EvalCase {
    EvalCase {
        id: "case".into(),
        name: "Case".into(),
        description: None,
        system_prompt: "s".into(),
        user_messages: vec!["hi".into()],
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

fn make_invocation(response: Option<&str>) -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: response.map(str::to_string),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "m"),
    }
}

#[test]
fn exact_match_passes_when_strings_match() {
    let evaluator = ExactMatchEvaluator::new("hello world");
    let case = make_case();
    let invocation = make_invocation(Some("hello world"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn exact_match_case_sensitive_by_default() {
    let evaluator = ExactMatchEvaluator::new("Hello");
    let case = make_case();
    let invocation = make_invocation(Some("hello"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(!result.score.verdict().is_pass());
}

#[test]
fn exact_match_honors_case_insensitive_toggle() {
    let evaluator = ExactMatchEvaluator::new("Hello").case_sensitive(false);
    let case = make_case();
    let invocation = make_invocation(Some("hello"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn exact_match_honors_trim_toggle() {
    let evaluator = ExactMatchEvaluator::new("hello").trim(true);
    let case = make_case();
    let invocation = make_invocation(Some("   hello\n"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn exact_match_returns_none_when_no_response() {
    let evaluator = ExactMatchEvaluator::new("x");
    let case = make_case();
    let invocation = make_invocation(None);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn levenshtein_passes_when_above_threshold() {
    let evaluator = LevenshteinDistanceEvaluator::new("kitten").with_threshold(0.5);
    let case = make_case();
    let invocation = make_invocation(Some("sitting"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    // distance=3, max=7, similarity ≈ 0.571 > 0.5
    assert!(result.score.verdict().is_pass());
    assert!(result.details.as_ref().unwrap().contains("distance=3"));
}

#[test]
fn levenshtein_fails_when_below_threshold() {
    let evaluator = LevenshteinDistanceEvaluator::new("hello").with_threshold(0.95);
    let case = make_case();
    let invocation = make_invocation(Some("world"));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(!result.score.verdict().is_pass());
}

#[test]
fn levenshtein_empty_strings_are_perfect_match() {
    let evaluator = LevenshteinDistanceEvaluator::new("");
    let case = make_case();
    let invocation = make_invocation(Some(""));
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert!(result.score.verdict().is_pass());
}
