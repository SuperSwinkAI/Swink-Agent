//! Plain-text, line-oriented console reporter.
//!
//! Spec 043 clarification Q8 and §FR-041 pin terminal output to plain text:
//! no ANSI color, no cursor control, no interactivity. The reporter emits
//! one line per case verdict followed by indented per-evaluator detail, plus
//! a trailing summary block.
//!
//! # Example output
//!
//! ```text
//! Eval set: demo-set (3/4 passed)
//! - case_a  PASS  (120ms)
//!     helpfulness  score=0.82  threshold=0.50  PASS
//!     correctness  score=0.91  threshold=0.60  PASS
//! - case_b  FAIL  (150ms)
//!     helpfulness  score=0.12  threshold=0.50  FAIL  reason: off-topic
//! Summary: 3 passed, 1 failed, total_cost=$0.012345, duration=420ms
//! ```

use std::fmt::Write as _;

use crate::{EvalCaseResult, EvalMetricResult, EvalSetResult, Verdict};

use super::{Reporter, ReporterError, ReporterOutput};

/// Always-on, plain-text terminal reporter (spec 043 §FR-041, Q8).
///
/// The reporter is a zero-sized struct because it holds no configuration:
/// the rendering is deterministic and produces the same bytes for a given
/// result regardless of terminal capability.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsoleReporter;

impl ConsoleReporter {
    /// Create a new reporter. Present for API symmetry with peer reporters.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Reporter for ConsoleReporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        let mut out = String::new();
        writeln!(
            out,
            "Eval set: {id} ({passed}/{total} passed)",
            id = result.eval_set_id,
            passed = result.summary.passed,
            total = result.summary.total_cases,
        )
        .map_err(|e| ReporterError::Format(e.to_string()))?;

        for case in &result.case_results {
            write_case_line(&mut out, case)?;
            for metric in &case.metric_results {
                write_metric_line(&mut out, metric)?;
            }
        }

        writeln!(
            out,
            "Summary: {passed} passed, {failed} failed, total_cost=${cost:.6}, duration={dur}ms",
            passed = result.summary.passed,
            failed = result.summary.failed,
            cost = result.summary.total_cost.total,
            dur = result.summary.total_duration.as_millis(),
        )
        .map_err(|e| ReporterError::Format(e.to_string()))?;

        Ok(ReporterOutput::Stdout(out))
    }
}

fn write_case_line(out: &mut String, case: &EvalCaseResult) -> Result<(), ReporterError> {
    writeln!(
        out,
        "- {id}  {verdict}  ({dur}ms)",
        id = case.case_id,
        verdict = verdict_label(case.verdict),
        dur = case.invocation.total_duration.as_millis(),
    )
    .map_err(|e| ReporterError::Format(e.to_string()))
}

fn write_metric_line(out: &mut String, metric: &EvalMetricResult) -> Result<(), ReporterError> {
    let verdict = metric.score.verdict();
    write!(
        out,
        "    {name}  score={score:.2}  threshold={th:.2}  {verdict}",
        name = metric.evaluator_name,
        score = metric.score.value,
        th = metric.score.threshold,
        verdict = verdict_label(verdict),
    )
    .map_err(|e| ReporterError::Format(e.to_string()))?;
    if let Some(details) = metric.details.as_ref().filter(|s| !s.is_empty()) {
        // Single-line only; strip embedded newlines so terminal output stays
        // strictly line-oriented per Q8.
        let sanitized = details.replace(['\n', '\r'], " ");
        write!(out, "  reason: {sanitized}")
            .map_err(|e| ReporterError::Format(e.to_string()))?;
    }
    writeln!(out).map_err(|e| ReporterError::Format(e.to_string()))?;
    Ok(())
}

const fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    }
}
