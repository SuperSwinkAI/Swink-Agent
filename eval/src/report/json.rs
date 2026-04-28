//! Self-contained JSON reporter (always-on, spec 043 §FR-041).
//!
//! Emits a document that validates against
//! `specs/043-evals-adv-features/contracts/eval-result.schema.json`. The
//! schema version is carried explicitly as [`SCHEMA_VERSION`] so downstream
//! consumers can detect breaking changes across spec revisions.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::{EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, Score, Verdict};

use super::{Reporter, ReporterError, ReporterOutput};

/// Stable schema version advertised in every JSON artifact.
///
/// Matches `schema_version` in `eval-result.schema.json`. Bumping this value
/// is a breaking change for any downstream consumer (e.g. the `report`
/// subcommand of `swink-eval`).
pub const SCHEMA_VERSION: &str = "043";

/// Default artifact filename suggested to callers via
/// [`ReporterOutput::Artifact`].
pub const DEFAULT_ARTIFACT_NAME: &str = "eval-result.json";

/// Always-on JSON reporter (spec 043 §FR-041).
///
/// Produces pretty-printed, deterministic JSON suitable for persistence and
/// later re-rendering through other reporters (see `swink-eval report`).
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonReporter {
    pretty: bool,
}

impl JsonReporter {
    /// Create a reporter that emits pretty-printed JSON (default).
    #[must_use]
    pub const fn new() -> Self {
        Self { pretty: true }
    }

    /// Toggle pretty-printing (off → single-line compact JSON).
    #[must_use]
    pub const fn pretty(mut self, pretty: bool) -> Self {
        self.pretty = pretty;
        self
    }
}

impl Reporter for JsonReporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        let doc = EvalResultDoc::from(result);
        let bytes = if self.pretty {
            serde_json::to_vec_pretty(&doc)
        } else {
            serde_json::to_vec(&doc)
        }
        .map_err(|e| ReporterError::Format(e.to_string()))?;
        Ok(ReporterOutput::Artifact {
            path: PathBuf::from(DEFAULT_ARTIFACT_NAME),
            bytes,
        })
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────
//
// These mirror the shape pinned in public-api.md §JSON wire schemas. They are
// intentionally independent of the in-memory `EvalSetResult` so schema
// evolution can happen without forcing breaking changes on Rust consumers.

#[derive(Debug, Serialize)]
struct EvalResultDoc<'a> {
    schema_version: &'static str,
    eval_set: EvalSetDoc<'a>,
    cases: Vec<CaseDoc<'a>>,
    summary: SummaryDoc,
    timestamp: u64,
}

#[derive(Debug, Serialize)]
struct EvalSetDoc<'a> {
    id: &'a str,
    case_count: usize,
}

#[derive(Debug, Serialize)]
struct CaseDoc<'a> {
    case_id: &'a str,
    verdict: VerdictLabel,
    duration_ms: u128,
    metrics: Vec<MetricDoc<'a>>,
}

#[derive(Debug, Serialize)]
struct MetricDoc<'a> {
    evaluator: &'a str,
    score: f64,
    threshold: f64,
    verdict: VerdictLabel,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SummaryDoc {
    total_cases: usize,
    passed: usize,
    failed: usize,
    total_cost: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_duration_ms: u128,
}

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum VerdictLabel {
    Pass,
    Fail,
}

impl From<Verdict> for VerdictLabel {
    fn from(v: Verdict) -> Self {
        match v {
            Verdict::Pass => Self::Pass,
            Verdict::Fail => Self::Fail,
        }
    }
}

impl<'a> From<&'a EvalSetResult> for EvalResultDoc<'a> {
    fn from(r: &'a EvalSetResult) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            eval_set: EvalSetDoc {
                id: &r.eval_set_id,
                case_count: r.case_results.len(),
            },
            cases: r.case_results.iter().map(CaseDoc::from).collect(),
            summary: SummaryDoc::from(&r.summary),
            timestamp: r.timestamp,
        }
    }
}

impl<'a> From<&'a EvalCaseResult> for CaseDoc<'a> {
    fn from(c: &'a EvalCaseResult) -> Self {
        Self {
            case_id: &c.case_id,
            verdict: c.verdict.into(),
            duration_ms: c.invocation.total_duration.as_millis(),
            metrics: c.metric_results.iter().map(MetricDoc::from).collect(),
        }
    }
}

impl<'a> From<&'a EvalMetricResult> for MetricDoc<'a> {
    fn from(m: &'a EvalMetricResult) -> Self {
        let Score { value, threshold } = m.score;
        Self {
            evaluator: &m.evaluator_name,
            score: value,
            threshold,
            verdict: m.score.verdict().into(),
            reason: m.details.as_deref(),
        }
    }
}

impl From<&EvalSummary> for SummaryDoc {
    fn from(s: &EvalSummary) -> Self {
        Self {
            total_cases: s.total_cases,
            passed: s.passed,
            failed: s.failed,
            total_cost: s.total_cost.total,
            total_input_tokens: s.total_usage.input,
            total_output_tokens: s.total_usage.output,
            total_duration_ms: duration_ms(s.total_duration),
        }
    }
}

fn duration_ms(d: Duration) -> u128 {
    d.as_millis()
}
