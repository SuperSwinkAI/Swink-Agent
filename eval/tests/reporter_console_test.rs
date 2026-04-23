//! Regression tests for `ConsoleReporter` (T139).
//!
//! Spec 043 Q8 clarification pins terminal output to plain text:
//! no ANSI color, no cursor control, no interactivity. These tests assert
//! those invariants alongside line-oriented structure and per-case/metric
//! detail.

mod common;

use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    ConsoleReporter, EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, Reporter,
    ReporterOutput, Score, Verdict,
};

use common::mock_invocation;

fn sample_result() -> EvalSetResult {
    let case_pass = EvalCaseResult {
        case_id: "case_alpha".into(),
        invocation: mock_invocation(&[], Some("ok"), 0.0012, 32),
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

    let case_fail = EvalCaseResult {
        case_id: "case_beta".into(),
        invocation: mock_invocation(&[], Some("nope"), 0.0034, 48),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "helpfulness".into(),
            score: Score::new(0.12, 0.5),
            // Contains an embedded newline — the reporter MUST strip it.
            details: Some("off-topic\nmultiline".into()),
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
                total: 0.0046,
                ..Default::default()
            },
            total_usage: Usage::default(),
            total_duration: Duration::from_millis(220),
        },
        timestamp: 0,
    }
}

fn render_console(result: &EvalSetResult) -> String {
    match ConsoleReporter::new().render(result).expect("render ok") {
        ReporterOutput::Stdout(s) => s,
        other => panic!("expected Stdout output, got {other:?}"),
    }
}

#[test]
fn console_output_header_and_line_count() {
    let out = render_console(&sample_result());
    // 1 header + 2 cases + 3 metrics + 1 summary = 7
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 7, "unexpected line count:\n{out}");
    assert!(
        lines[0].starts_with("Eval set: demo-set"),
        "header: {}",
        lines[0]
    );
    assert!(lines[0].contains("1/2 passed"));
}

#[test]
fn console_output_contains_per_case_verdict_and_duration() {
    let out = render_console(&sample_result());
    assert!(out.contains("- case_alpha  PASS"), "\n{out}");
    assert!(out.contains("- case_beta  FAIL"), "\n{out}");
    assert!(out.contains("100ms"), "each case shows ms duration\n{out}");
}

#[test]
fn console_output_contains_indented_metric_lines() {
    let out = render_console(&sample_result());
    // Metric lines must be indented (4 spaces) to visually group under the
    // parent case; any indentation loss would collapse grouping.
    assert!(
        out.contains("    helpfulness  score=0.82  threshold=0.50  PASS"),
        "\n{out}"
    );
    assert!(
        out.contains("    correctness  score=0.91  threshold=0.60  PASS"),
        "\n{out}"
    );
    assert!(
        out.contains("    helpfulness  score=0.12  threshold=0.50  FAIL"),
        "\n{out}"
    );
}

#[test]
fn console_output_includes_reason_when_present_single_line() {
    let out = render_console(&sample_result());
    // The failing metric has embedded newline "off-topic\nmultiline"; the
    // reporter MUST keep it on a single line (Q8 "line-oriented").
    let failing_line = out
        .lines()
        .find(|l| l.contains("helpfulness  score=0.12"))
        .expect("failing metric line");
    assert!(
        failing_line.contains("reason: off-topic multiline"),
        "reason must be sanitized onto one line: {failing_line}"
    );
}

#[test]
fn console_output_has_no_ansi_or_cursor_control() {
    let out = render_console(&sample_result());
    // ESC (0x1B) is the prefix of every ANSI sequence; CSI ("\x1b[") is the
    // most common cursor-control / color prefix.
    assert!(
        !out.contains('\x1b'),
        "ANSI escape found in console output:\n{out}"
    );
    // Control characters other than \n MUST NOT appear.
    for ch in out.chars() {
        if ch.is_control() {
            assert_eq!(
                ch, '\n',
                "unexpected control character {ch:?} in console output"
            );
        }
    }
}

#[test]
fn console_output_summary_line_carries_totals() {
    let out = render_console(&sample_result());
    let summary = out
        .lines()
        .last()
        .expect("non-empty reporter output has a last line");
    assert!(summary.starts_with("Summary: "));
    assert!(summary.contains("1 passed"));
    assert!(summary.contains("1 failed"));
    assert!(summary.contains("total_cost=$0.004600"));
    assert!(summary.contains("duration=220ms"));
}

#[test]
fn console_output_is_deterministic_across_two_renders() {
    let result = sample_result();
    let first = render_console(&result);
    let second = render_console(&result);
    assert_eq!(first, second);
}

#[test]
fn console_output_handles_empty_eval_set() {
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
    let out = render_console(&empty);
    assert!(out.lines().count() == 2, "header + summary only:\n{out}");
    assert!(out.contains("0/0 passed"));
    assert!(out.contains("0 passed, 0 failed"));
}
