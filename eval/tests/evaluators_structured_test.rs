//! Regression tests for the Structured family evaluators (T071).

#![cfg(feature = "evaluator-structured")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use swink_agent::{ModelSpec, StopReason};
use swink_agent_eval::{
    EvalCase, Evaluator, Invocation, JsonMatchEvaluator, JsonSchemaEvaluator, KeyStrategy,
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
fn json_match_average_aggregates_per_key() {
    let evaluator = JsonMatchEvaluator::new(json!({"a": 1, "b": 2}));
    let response = json!({"a": 1, "b": 99}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&response)))
        .unwrap();
    // Average of 1.0 + 0.0 = 0.5; still passes default 0.5 threshold.
    assert!((result.score.value - 0.5).abs() < 1e-6);
}

#[test]
fn json_match_all_requires_every_key_match() {
    let evaluator =
        JsonMatchEvaluator::new(json!({"a": 1, "b": 2})).with_strategy(KeyStrategy::All);
    let mismatched = json!({"a": 1, "b": 3}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&mismatched)))
        .unwrap();
    assert!(!result.score.verdict().is_pass());

    let matched = json!({"a": 1, "b": 2}).to_string();
    let pass = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&matched)))
        .unwrap();
    assert!(pass.score.verdict().is_pass());
}

#[test]
fn json_match_none_passes_when_every_key_differs() {
    let evaluator =
        JsonMatchEvaluator::new(json!({"a": 1, "b": 2})).with_strategy(KeyStrategy::None);
    let response = json!({"a": 9, "b": 8}).to_string();
    let pass = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&response)))
        .unwrap();
    assert!(pass.score.verdict().is_pass());
}

#[test]
fn json_match_rubric_uses_custom_scorer() {
    let evaluator =
        JsonMatchEvaluator::new(json!({"a": 1, "b": 2})).with_strategy(KeyStrategy::Rubric {
            scorer: Arc::new(|_key, _expected, _actual| 0.7),
        });
    let response = json!({"a": 999, "b": 999}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&response)))
        .unwrap();
    assert!((result.score.value - 0.7).abs() < 1e-6);
}

#[test]
fn json_match_exclude_keys_skips_entries() {
    let evaluator = JsonMatchEvaluator::new(json!({"a": 1, "b": 2, "c": 3}))
        .with_strategy(KeyStrategy::All)
        .with_exclude_keys(["c"]);
    let response = json!({"a": 1, "b": 2, "c": 999}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&response)))
        .unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn json_match_malformed_json_is_fail_with_details() {
    let evaluator = JsonMatchEvaluator::new(json!({"a": 1}));
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some("not json {")))
        .unwrap();
    assert!(!result.score.verdict().is_pass());
    assert!(
        result
            .details
            .as_ref()
            .unwrap()
            .contains("malformed JSON response")
    );
}

#[test]
fn json_schema_happy_path() {
    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {"name": {"type": "string"}}
    });
    let evaluator = JsonSchemaEvaluator::new(&schema).unwrap();
    let valid = json!({"name": "wes"}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&valid)))
        .unwrap();
    assert!(result.score.verdict().is_pass());
}

#[test]
fn json_schema_unhappy_path_surfaces_errors() {
    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {"name": {"type": "string"}}
    });
    let evaluator = JsonSchemaEvaluator::new(&schema).unwrap();
    let invalid = json!({}).to_string();
    let result = evaluator
        .evaluate(&make_case(), &make_invocation(Some(&invalid)))
        .unwrap();
    assert!(!result.score.verdict().is_pass());
    assert!(!result.details.as_ref().unwrap().is_empty());
}

#[test]
fn json_schema_returns_none_without_response() {
    let schema = json!({"type": "object"});
    let evaluator = JsonSchemaEvaluator::new(&schema).unwrap();
    assert!(
        evaluator
            .evaluate(&make_case(), &make_invocation(None))
            .is_none()
    );
}
