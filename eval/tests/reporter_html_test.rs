//! Regression tests for `HtmlReporter` (T146).
//!
//! Verifies the emitted artifact is a single self-contained HTML file that
//! uses `<details>` / `<summary>` for collapsible case sections and stays
//! reasonably bounded even for large result sets.

#![cfg(feature = "html-report")]

mod common;

use std::path::PathBuf;
use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, HtmlReporter, Reporter,
    ReporterOutput, Score, Verdict,
};

use common::mock_invocation;

fn sample_result() -> EvalSetResult {
    let case_pass = EvalCaseResult {
        case_id: "case_alpha".into(),
        invocation: mock_invocation(&[], Some("ok"), 0.01, 120),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "helpfulness".into(),
            score: Score::new(0.82, 0.5),
            details: Some("looks good".into()),
        }],
        verdict: Verdict::Pass,
    };
    let case_fail = EvalCaseResult {
        case_id: "case_beta".into(),
        invocation: mock_invocation(&[], Some("bad"), 0.02, 140),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "correctness".into(),
            score: Score::new(0.12, 0.6),
            details: Some("off-topic".into()),
        }],
        verdict: Verdict::Fail,
    };
    EvalSetResult {
        eval_set_id: "demo-set".into(),
        case_results: vec![case_pass, case_fail],
        summary: EvalSummary {
            total_cases: 2,
            passed: 1,
            failed: 1,
            total_cost: Cost {
                total: 0.03,
                ..Default::default()
            },
            total_usage: Usage {
                input: 120,
                output: 140,
                total: 260,
                ..Default::default()
            },
            total_duration: Duration::from_millis(220),
        },
        timestamp: 42,
    }
}

fn large_result(case_count: usize) -> EvalSetResult {
    let total_cost = (0..case_count).fold(0.0, |acc, _| acc + 0.001);
    let case_results = (0..case_count)
        .map(|idx| EvalCaseResult {
            case_id: format!("case_{idx}"),
            invocation: mock_invocation(&[], Some("ok"), 0.001, 10),
            metric_results: vec![
                EvalMetricResult {
                    evaluator_name: "helpfulness".into(),
                    score: Score::new(0.82, 0.5),
                    details: Some("looks good".into()),
                },
                EvalMetricResult {
                    evaluator_name: "correctness".into(),
                    score: Score::new(0.91, 0.6),
                    details: Some("on target".into()),
                },
            ],
            verdict: Verdict::Pass,
        })
        .collect();

    EvalSetResult {
        eval_set_id: "large-set".into(),
        case_results,
        summary: EvalSummary {
            total_cases: case_count,
            passed: case_count,
            failed: 0,
            total_cost: Cost {
                total: total_cost,
                ..Default::default()
            },
            total_usage: Usage {
                input: case_count as u64 * 5,
                output: case_count as u64 * 5,
                total: case_count as u64 * 10,
                ..Default::default()
            },
            total_duration: Duration::from_secs(case_count as u64),
        },
        timestamp: 99,
    }
}

fn render_html(result: &EvalSetResult) -> (PathBuf, String) {
    match HtmlReporter::new().render(result).expect("render ok") {
        ReporterOutput::Artifact { path, bytes } => (
            path,
            String::from_utf8(bytes).expect("html bytes should be utf-8"),
        ),
        other => panic!("expected Artifact output, got {other:?}"),
    }
}

#[test]
fn html_reporter_emits_single_self_contained_file() {
    let (path, html) = render_html(&sample_result());
    assert_eq!(path, PathBuf::from("eval-report.html"));
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(html.contains("<style>"), "{html}");
    assert!(!html.contains("<script src="), "{html}");
    assert!(!html.contains("<link rel=\"stylesheet\""), "{html}");
    assert!(!html.contains("http://"), "{html}");
    assert!(!html.contains("https://"), "{html}");
}

#[test]
fn html_reporter_uses_details_summary_collapsible_sections() {
    let (_, html) = render_html(&sample_result());
    assert!(html.contains("<details"), "{html}");
    assert_eq!(html.matches("<summary>").count(), 2, "{html}");
    assert!(html.contains("case_alpha"), "{html}");
    assert!(html.contains("case_beta"), "{html}");
    assert!(html.contains("helpfulness"), "{html}");
    assert!(html.contains("correctness"), "{html}");
}

#[test]
fn html_reporter_renders_summary_metrics() {
    let (_, html) = render_html(&sample_result());
    assert!(html.contains("demo-set"), "{html}");
    assert!(html.contains("1 / 2"), "{html}");
    assert!(html.contains("$0.030000"), "{html}");
    assert!(html.contains("220ms"), "{html}");
}

#[test]
fn html_reporter_is_deterministic() {
    let result = sample_result();
    let (_, first) = render_html(&result);
    let (_, second) = render_html(&result);
    assert_eq!(first, second);
}

#[test]
fn html_reporter_output_stays_bounded_for_thousand_case_results() {
    let (_, html) = render_html(&large_result(1_000));
    assert!(
        html.len() < 1_500_000,
        "html output unexpectedly large: {} bytes",
        html.len()
    );
    assert_eq!(html.matches("<details").count(), 1_000);
}
