//! End-to-end coverage for US8 reporter integration (T150).
//!
//! The same `EvalSetResult` should render cleanly through every shipped
//! reporter. This test focuses on the cross-reporter contract: shared case and
//! metric content must survive each format-specific projection, and JSON must
//! still validate against the published schema.

#![cfg(feature = "html-report")]

mod common;

use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    ConsoleReporter, EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, HtmlReporter,
    JsonReporter, MarkdownReporter, Reporter, ReporterOutput, Score, Verdict,
};

use common::mock_invocation;

const SCHEMA_JSON: &str =
    include_str!("../../specs/043-evals-adv-features/contracts/eval-result.schema.json");

fn sample_result() -> EvalSetResult {
    let case_pass = EvalCaseResult {
        case_id: "case_alpha".into(),
        invocation: mock_invocation(&["search_docs"], Some("Refund approved"), 0.0125, 180),
        metric_results: vec![
            EvalMetricResult {
                evaluator_name: "helpfulness".into(),
                score: Score::new(0.87, 0.5),
                details: Some("grounded in policy".into()),
            },
            EvalMetricResult {
                evaluator_name: "correctness".into(),
                score: Score::new(0.93, 0.7),
                details: Some("quoted the refund window".into()),
            },
        ],
        verdict: Verdict::Pass,
    };
    let case_fail = EvalCaseResult {
        case_id: "case_beta".into(),
        invocation: mock_invocation(&["lookup_order"], Some("Need more info"), 0.008, 140),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "task_completion".into(),
            score: Score::new(0.24, 0.6),
            details: Some("did not resolve the missing package".into()),
        }],
        verdict: Verdict::Fail,
    };

    EvalSetResult {
        eval_set_id: "support-evals".into(),
        case_results: vec![case_pass, case_fail],
        summary: EvalSummary {
            total_cases: 2,
            passed: 1,
            failed: 1,
            total_cost: Cost {
                total: 0.0205,
                ..Default::default()
            },
            total_usage: Usage {
                input: 190,
                output: 130,
                total: 320,
                ..Default::default()
            },
            total_duration: Duration::from_millis(275),
        },
        timestamp: 1_713_901_234,
    }
}

fn render_stdout(reporter: &dyn Reporter, result: &EvalSetResult) -> String {
    match reporter.render(result).expect("render ok") {
        ReporterOutput::Stdout(text) => text,
        other => panic!("expected stdout output, got {other:?}"),
    }
}

fn render_artifact(reporter: &dyn Reporter, result: &EvalSetResult) -> (String, Vec<u8>) {
    match reporter.render(result).expect("render ok") {
        ReporterOutput::Artifact { path, bytes } => (path.display().to_string(), bytes),
        other => panic!("expected artifact output, got {other:?}"),
    }
}

#[test]
fn reporters_render_the_same_eval_result_across_all_formats() {
    let result = sample_result();

    let console = render_stdout(&ConsoleReporter::new(), &result);
    let markdown = render_stdout(&MarkdownReporter::new(), &result);
    let (json_path, json_bytes) = render_artifact(&JsonReporter::new(), &result);
    let (html_path, html_bytes) = render_artifact(&HtmlReporter::new(), &result);
    let html = String::from_utf8(html_bytes).expect("html is utf-8");

    assert_eq!(json_path, "eval-result.json");
    assert_eq!(html_path, "eval-report.html");

    for needle in [
        "support-evals",
        "case_alpha",
        "case_beta",
        "helpfulness",
        "correctness",
        "task_completion",
    ] {
        assert!(
            console.contains(needle),
            "console missing {needle}:\n{console}"
        );
        assert!(
            markdown.contains(needle),
            "markdown missing {needle}:\n{markdown}"
        );
        assert!(html.contains(needle), "html missing {needle}:\n{html}");
    }

    assert!(console.contains("1/2 passed"), "\n{console}");
    assert!(markdown.contains("## Summary"), "\n{markdown}");
    assert!(html.starts_with("<!DOCTYPE html>"), "\n{html}");
    assert_eq!(html.matches("<details").count(), result.case_results.len());
    assert_eq!(html.matches("<summary>").count(), result.case_results.len());

    let json_doc: serde_json::Value =
        serde_json::from_slice(&json_bytes).expect("json reporter emits valid JSON");
    let schema: serde_json::Value =
        serde_json::from_str(SCHEMA_JSON).expect("embedded schema parses");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let errors: Vec<_> = validator
        .iter_errors(&json_doc)
        .map(|err| format!("{err} @ {}", err.instance_path))
        .collect();
    assert!(errors.is_empty(), "schema validation failed: {errors:#?}");

    assert_eq!(json_doc["eval_set"]["id"], "support-evals");
    assert_eq!(
        json_doc["cases"][0]["metrics"][0]["evaluator"],
        "helpfulness"
    );
    assert_eq!(
        json_doc["cases"][1]["metrics"][0]["evaluator"],
        "task_completion"
    );
}
