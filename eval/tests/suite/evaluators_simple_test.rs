//! Regression tests for the Simple family evaluators (T074).

use std::time::Duration;

use swink_agent::{ModelSpec, StopReason};
use swink_agent_eval::{
    EvalCase, Evaluator, ExactMatchEvaluator, Invocation, LevenshteinDistanceEvaluator,
};

fn make_case() -> EvalCase {
    EvalCase::new("case", "Case", "s", vec!["hi".into()])
}

fn make_invocation(response: Option<&str>) -> Invocation {
    let mut invocation = Invocation::new(StopReason::Stop, ModelSpec::new("test", "m"))
        .with_total_duration(Duration::from_millis(1));
    invocation.final_response = response.map(str::to_string);
    invocation
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
