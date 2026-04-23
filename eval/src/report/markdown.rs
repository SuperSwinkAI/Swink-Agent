//! PR-comment-ready Markdown reporter (always-on, spec 043 §FR-041).
//!
//! Emits a valid GFM-flavoured Markdown document:
//!
//! * A top-level header with the eval set id.
//! * A summary table (total / passed / failed / cost / duration).
//! * A per-case table listing verdict and duration.
//! * An optional per-metric section with evaluator score, verdict, and reason.
//!
//! The output contains no ANSI escapes, no HTML, and no interactivity —
//! suitable for direct inclusion in a GitHub PR comment.

use std::fmt::Write as _;

use crate::{EvalCaseResult, EvalMetricResult, EvalSetResult, Verdict};

use super::{Reporter, ReporterError, ReporterOutput};

/// Always-on Markdown reporter (spec 043 §FR-041).
///
/// Produces a self-contained Markdown document safe to paste into a PR
/// comment. The rendering is deterministic: identical `EvalSetResult`
/// inputs always produce byte-identical output.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownReporter;

impl MarkdownReporter {
    /// Create a new reporter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Reporter for MarkdownReporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        let mut out = String::new();
        fmt_err(writeln!(
            out,
            "# Eval Result: `{id}`",
            id = result.eval_set_id
        ))?;
        fmt_err(writeln!(out))?;

        // Summary table
        fmt_err(writeln!(out, "## Summary"))?;
        fmt_err(writeln!(out))?;
        fmt_err(writeln!(out, "| Metric | Value |"))?;
        fmt_err(writeln!(out, "| --- | --- |"))?;
        fmt_err(writeln!(
            out,
            "| Total cases | {} |",
            result.summary.total_cases
        ))?;
        fmt_err(writeln!(out, "| Passed | {} |", result.summary.passed))?;
        fmt_err(writeln!(out, "| Failed | {} |", result.summary.failed))?;
        fmt_err(writeln!(
            out,
            "| Total cost | ${:.6} |",
            result.summary.total_cost.total
        ))?;
        fmt_err(writeln!(
            out,
            "| Duration | {}ms |",
            result.summary.total_duration.as_millis()
        ))?;
        fmt_err(writeln!(out))?;

        // Per-case table
        fmt_err(writeln!(out, "## Cases"))?;
        fmt_err(writeln!(out))?;
        fmt_err(writeln!(out, "| Case | Verdict | Duration |"))?;
        fmt_err(writeln!(out, "| --- | --- | --- |"))?;
        for case in &result.case_results {
            fmt_err(writeln!(
                out,
                "| `{id}` | {verdict} | {dur}ms |",
                id = escape_md_cell(&case.case_id),
                verdict = verdict_label(case.verdict),
                dur = case.invocation.total_duration.as_millis(),
            ))?;
        }
        fmt_err(writeln!(out))?;

        // Per-metric details (only if any case has metrics)
        let has_metrics = result
            .case_results
            .iter()
            .any(|c| !c.metric_results.is_empty());
        if has_metrics {
            fmt_err(writeln!(out, "## Metrics"))?;
            fmt_err(writeln!(out))?;
            fmt_err(writeln!(
                out,
                "| Case | Evaluator | Score | Threshold | Verdict | Reason |"
            ))?;
            fmt_err(writeln!(out, "| --- | --- | --- | --- | --- | --- |"))?;
            for case in &result.case_results {
                for metric in &case.metric_results {
                    write_metric_row(&mut out, &case.case_id, metric)?;
                }
            }
        }

        Ok(ReporterOutput::Stdout(out))
    }
}

fn write_metric_row(
    out: &mut String,
    case_id: &str,
    metric: &EvalMetricResult,
) -> Result<(), ReporterError> {
    let reason = metric
        .details
        .as_deref()
        .map_or_else(String::new, escape_md_cell);
    fmt_err(writeln!(
        out,
        "| `{case}` | `{name}` | {score:.2} | {th:.2} | {verdict} | {reason} |",
        case = escape_md_cell(case_id),
        name = escape_md_cell(&metric.evaluator_name),
        score = metric.score.value,
        th = metric.score.threshold,
        verdict = verdict_label(metric.score.verdict()),
    ))
}

/// Escape characters that would break a Markdown table cell.
///
/// Pipes and newlines are the only Markdown-table-significant characters;
/// backslashes are escaped so a trailing `\` cannot consume the row's pipe.
fn escape_md_cell(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn fmt_err<T>(res: Result<T, std::fmt::Error>) -> Result<T, ReporterError> {
    res.map_err(|e| ReporterError::Format(e.to_string()))
}

const fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    }
}
