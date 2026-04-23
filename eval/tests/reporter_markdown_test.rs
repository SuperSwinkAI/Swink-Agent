//! Regression tests for `MarkdownReporter` (T144).
//!
//! Verifies the emitted document is PR-comment-ready: valid Markdown tables,
//! no ANSI escapes, per-case and per-metric detail preserved.

mod common;

use std::time::Duration;

use swink_agent::{Cost, Usage};
use swink_agent_eval::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, MarkdownReporter, Reporter,
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
    // Embed a pipe character in the case id to exercise Markdown escaping.
    let case_fail = EvalCaseResult {
        case_id: "case|beta".into(),
        invocation: mock_invocation(&[], Some("bad"), 0.01, 120),
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
                total: 0.02,
                ..Default::default()
            },
            total_usage: Usage::default(),
            total_duration: Duration::from_millis(220),
        },
        timestamp: 0,
    }
}

fn render(result: &EvalSetResult) -> String {
    match MarkdownReporter::new().render(result).expect("render ok") {
        ReporterOutput::Stdout(s) => s,
        other => panic!("expected Stdout output, got {other:?}"),
    }
}

#[test]
fn markdown_output_contains_header_and_sections() {
    let out = render(&sample_result());
    assert!(out.contains("# Eval Result: `demo-set`"));
    assert!(out.contains("## Summary"));
    assert!(out.contains("## Cases"));
    assert!(out.contains("## Metrics"));
}

#[test]
fn markdown_summary_table_is_valid() {
    let out = render(&sample_result());
    // Valid Markdown table: header row, delimiter row, N body rows.
    let summary_section: String = out
        .split("## Summary")
        .nth(1)
        .expect("summary section present")
        .split("## Cases")
        .next()
        .expect("summary terminator")
        .into();
    assert!(summary_section.contains("| Metric | Value |"));
    assert!(summary_section.contains("| --- | --- |"));
    assert!(summary_section.contains("| Total cases | 2 |"));
    assert!(summary_section.contains("| Passed | 1 |"));
    assert!(summary_section.contains("| Failed | 1 |"));
    assert!(summary_section.contains("| Duration | 220ms |"));
}

#[test]
fn markdown_cases_table_carries_per_case_detail() {
    let out = render(&sample_result());
    assert!(out.contains("| Case | Verdict | Duration |"));
    assert!(out.contains("| `case_alpha` | PASS | 100ms |"));
    // Pipe inside the case id must be escaped with a backslash so GFM
    // renders the row correctly.
    assert!(
        out.contains("| `case\\|beta` | FAIL | 100ms |"),
        "expected escaped pipe in case id:\n{out}"
    );
}

#[test]
fn markdown_metrics_table_carries_per_metric_detail() {
    let out = render(&sample_result());
    assert!(out.contains("| Case | Evaluator | Score | Threshold | Verdict | Reason |"));
    assert!(
        out.contains("| `case_alpha` | `helpfulness` | 0.82 | 0.50 | PASS | looks good |"),
        "\n{out}"
    );
    assert!(
        out.contains("| `case\\|beta` | `correctness` | 0.12 | 0.60 | FAIL | off-topic |"),
        "\n{out}"
    );
}

#[test]
fn markdown_output_has_no_ansi_or_html() {
    let out = render(&sample_result());
    assert!(
        !out.contains('\x1b'),
        "ANSI escape found in markdown output:\n{out}"
    );
    // Output must be Markdown, not HTML. No tag-like constructs.
    assert!(!out.contains("<table"));
    assert!(!out.contains("<div"));
    assert!(!out.contains("<span"));
    assert!(!out.contains("<script"));
}

#[test]
fn markdown_every_table_row_has_matching_pipe_count() {
    let out = render(&sample_result());
    // Every table row (a line beginning with '|') must have the same number
    // of pipes within a contiguous table block. We coarsely assert each
    // row has at least 2 pipes (a minimal table cell) and all rows within
    // a block share the same pipe count.
    let mut blocks: Vec<Vec<&str>> = vec![];
    let mut current: Vec<&str> = vec![];
    for line in out.lines() {
        if line.trim_start().starts_with('|') {
            current.push(line);
        } else if !current.is_empty() {
            blocks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    assert!(!blocks.is_empty(), "no table rows rendered");
    // Count cell-boundary pipes only — escaped `\|` within a cell does not
    // delimit a new cell in GFM.
    fn cell_boundary_pipes(line: &str) -> usize {
        line.replace("\\|", "")
            .chars()
            .filter(|c| *c == '|')
            .count()
    }
    for block in blocks {
        let counts: Vec<usize> = block.iter().map(|l| cell_boundary_pipes(l)).collect();
        let first = counts[0];
        assert!(first >= 2, "row has fewer than 2 pipes: {:?}", block[0]);
        for (i, c) in counts.iter().enumerate() {
            assert_eq!(
                *c, first,
                "row {i} pipe count mismatch in block starting with {:?}",
                block[0]
            );
        }
    }
}

#[test]
fn markdown_render_is_deterministic() {
    let result = sample_result();
    let a = render(&result);
    let b = render(&result);
    assert_eq!(a, b);
}

#[test]
fn markdown_omits_metrics_section_when_no_metrics() {
    let result = EvalSetResult {
        eval_set_id: "no-metrics".into(),
        case_results: vec![EvalCaseResult {
            case_id: "solo".into(),
            invocation: mock_invocation(&[], Some("ok"), 0.0, 0),
            metric_results: vec![],
            verdict: Verdict::Pass,
        }],
        summary: EvalSummary {
            total_cases: 1,
            passed: 1,
            failed: 0,
            total_cost: Cost::default(),
            total_usage: Usage::default(),
            total_duration: Duration::from_millis(5),
        },
        timestamp: 0,
    };
    let out = render(&result);
    assert!(out.contains("## Cases"));
    assert!(
        !out.contains("## Metrics"),
        "metrics section leaked:\n{out}"
    );
}
