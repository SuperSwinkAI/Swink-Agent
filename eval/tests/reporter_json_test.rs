//! Regression tests for `JsonReporter` (T142).
//!
//! Validates every emitted artifact against
//! `specs/043-evals-adv-features/contracts/eval-result.schema.json`.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, JsonReporter, Reporter,
    ReporterOutput, Score, Verdict,
};

use common::mock_invocation;

/// Embed the schema at compile time so the test suite is hermetic.
const SCHEMA_JSON: &str =
    include_str!("../../specs/043-evals-adv-features/contracts/eval-result.schema.json");

fn sample_result() -> EvalSetResult {
    let case = EvalCaseResult {
        case_id: "case_alpha".into(),
        invocation: mock_invocation(&[], Some("ok"), 0.01, 120),
        metric_results: vec![
            EvalMetricResult {
                evaluator_name: "helpfulness".into(),
                score: Score::new(0.82, 0.5),
                details: Some("looks good".into()),
            },
            EvalMetricResult {
                evaluator_name: "correctness".into(),
                score: Score::new(0.91, 0.6),
                details: None,
            },
        ],
        verdict: Verdict::Pass,
    };
    EvalSetResult {
        eval_set_id: "demo-set".into(),
        case_results: vec![case],
        summary: EvalSummary {
            total_cases: 1,
            passed: 1,
            failed: 0,
            total_cost: Cost {
                input: 0.004,
                output: 0.006,
                total: 0.01,
                ..Default::default()
            },
            total_usage: Usage {
                input: 60,
                output: 60,
                total: 120,
                ..Default::default()
            },
            total_duration: Duration::from_millis(150),
        },
        timestamp: 1_700_000_000,
    }
}

fn render(reporter: JsonReporter, result: &EvalSetResult) -> (PathBuf, Vec<u8>) {
    match reporter.render(result).expect("render ok") {
        ReporterOutput::Artifact { path, bytes } => (path, bytes),
        other => panic!("expected Artifact output, got {other:?}"),
    }
}

fn schema_value() -> serde_json::Value {
    serde_json::from_str(SCHEMA_JSON).expect("schema parses as JSON")
}

#[test]
fn json_artifact_parses_and_validates_against_schema() {
    let result = sample_result();
    let (path, bytes) = render(JsonReporter::new(), &result);
    assert_eq!(path, PathBuf::from("eval-result.json"));

    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).expect("JsonReporter output is valid JSON");

    let schema = schema_value();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let errors: Vec<_> = validator
        .iter_errors(&doc)
        .map(|e| format!("{e} @ {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "schema validation failed: {errors:#?}");
}

#[test]
fn json_artifact_carries_schema_version() {
    let (_, bytes) = render(JsonReporter::new(), &sample_result());
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["schema_version"], "043");
}

#[test]
fn json_artifact_preserves_per_case_and_per_metric_detail() {
    let (_, bytes) = render(JsonReporter::new(), &sample_result());
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(doc["eval_set"]["id"], "demo-set");
    assert_eq!(doc["eval_set"]["case_count"], 1);
    assert_eq!(doc["cases"][0]["case_id"], "case_alpha");
    assert_eq!(doc["cases"][0]["verdict"], "pass");
    assert_eq!(doc["cases"][0]["metrics"].as_array().unwrap().len(), 2);

    let m0 = &doc["cases"][0]["metrics"][0];
    assert_eq!(m0["evaluator"], "helpfulness");
    assert_eq!(m0["reason"], "looks good");
    assert_eq!(m0["verdict"], "pass");

    // The second metric omitted `details`; `reason` MUST be absent (not
    // null) so the schema's additionalProperties=false stays green.
    let m1 = &doc["cases"][0]["metrics"][1];
    assert!(m1.get("reason").is_none(), "missing reason must be absent");
}

#[test]
fn json_artifact_summary_matches_totals() {
    let (_, bytes) = render(JsonReporter::new(), &sample_result());
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let summary = &doc["summary"];
    assert_eq!(summary["total_cases"], 1);
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["total_duration_ms"], 150);
    assert_eq!(summary["total_input_tokens"], 60);
    assert_eq!(summary["total_output_tokens"], 60);
}

#[test]
fn json_compact_and_pretty_both_validate() {
    let result = sample_result();
    let schema = schema_value();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");

    for (label, reporter) in [
        ("pretty", JsonReporter::new().pretty(true)),
        ("compact", JsonReporter::new().pretty(false)),
    ] {
        let (_, bytes) = render(reporter, &result);
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let errs: Vec<_> = validator
            .iter_errors(&doc)
            .map(|e| format!("{e}"))
            .collect();
        assert!(errs.is_empty(), "{label} output failed schema: {errs:?}");
    }
}

#[test]
fn json_renders_are_deterministic() {
    let result = sample_result();
    let (_, bytes_a) = render(JsonReporter::new(), &result);
    let (_, bytes_b) = render(JsonReporter::new(), &result);
    assert_eq!(bytes_a, bytes_b);
}

#[test]
fn json_artifact_validates_empty_case_set() {
    let empty = EvalSetResult {
        eval_set_id: "empty".into(),
        case_results: vec![],
        summary: EvalSummary {
            total_cases: 0,
            passed: 0,
            failed: 0,
            total_cost: Cost::default(),
            total_usage: Usage::default(),
            total_duration: Duration::ZERO,
        },
        timestamp: 0,
    };
    let (_, bytes) = render(JsonReporter::new(), &empty);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let schema = schema_value();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(&doc)
        .map(|e| format!("{e}"))
        .collect();
    assert!(errors.is_empty(), "empty result schema errors: {errors:?}");
    assert_eq!(doc["cases"], serde_json::json!([]));
}
