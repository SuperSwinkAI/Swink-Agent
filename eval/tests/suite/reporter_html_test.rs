//! Regression tests for `HtmlReporter` (T146).
//!
//! Verifies the emitted artifact is a single self-contained HTML file that
//! uses `<details>` / `<summary>` for collapsible case sections and stays
//! reasonably bounded even for large result sets.

use std::path::PathBuf;
use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, HtmlReporter, Reporter,
    ReporterOutput, Score, Verdict,
};

use crate::common::mock_invocation;

fn sample_result() -> EvalSetResult {
    let case_pass = EvalCaseResult::new(
        "case_alpha",
        mock_invocation(&[], Some("ok"), 0.01, 120),
        Verdict::Pass,
    )
    .with_metric_results(vec![
        EvalMetricResult::new("helpfulness", Score::new(0.82, 0.5)).with_details("looks good"),
    ]);
    let case_fail = EvalCaseResult::new(
        "case_beta",
        mock_invocation(&[], Some("bad"), 0.02, 140),
        Verdict::Fail,
    )
    .with_metric_results(vec![
        EvalMetricResult::new("correctness", Score::new(0.12, 0.6)).with_details("off-topic"),
    ]);
    EvalSetResult::new(
        "demo-set",
        vec![case_pass, case_fail],
        EvalSummary::default()
            .with_total_cases(2)
            .with_passed(1)
            .with_failed(1)
            .with_total_cost(Cost::default().with_total(0.03))
            .with_total_usage(
                Usage::default()
                    .with_input(120)
                    .with_output(140)
                    .with_total(260),
            )
            .with_total_duration(Duration::from_millis(220)),
        42,
    )
}

fn large_result(case_count: usize) -> EvalSetResult {
    let total_cost = (0..case_count).fold(0.0, |acc, _| acc + 0.001);
    let case_results = (0..case_count)
        .map(|idx| {
            EvalCaseResult::new(
                format!("case_{idx}"),
                mock_invocation(&[], Some("ok"), 0.001, 10),
                Verdict::Pass,
            )
            .with_metric_results(vec![
                EvalMetricResult::new("helpfulness", Score::new(0.82, 0.5))
                    .with_details("looks good"),
                EvalMetricResult::new("correctness", Score::new(0.91, 0.6))
                    .with_details("on target"),
            ])
        })
        .collect();

    EvalSetResult::new(
        "large-set",
        case_results,
        EvalSummary::default()
            .with_total_cases(case_count)
            .with_passed(case_count)
            .with_total_cost(Cost::default().with_total(total_cost))
            .with_total_usage(
                Usage::default()
                    .with_input(case_count as u64 * 5)
                    .with_output(case_count as u64 * 5)
                    .with_total(case_count as u64 * 10),
            )
            .with_total_duration(Duration::from_secs(case_count as u64)),
        99,
    )
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
