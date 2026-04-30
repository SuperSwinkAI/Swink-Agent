//! Self-contained JSON reporter (always-on, spec 043 §FR-041).
//!
//! Emits a document that validates against
//! `specs/043-evals-adv-features/contracts/eval-result.schema.json`. The
//! schema version is carried explicitly as [`SCHEMA_VERSION`] so downstream
//! consumers can detect breaking changes across spec revisions.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swink_agent::{Cost, ModelSpec, StopReason, Usage};

use crate::{
    EvalCaseResult, EvalMetricResult, EvalSetResult, EvalSummary, Invocation, Score, Verdict,
};

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

/// Decode a persisted eval result JSON file.
///
/// Accepts both the raw [`EvalSetResult`] store shape and the versioned
/// [`JsonReporter`] artifact shape so `swink-eval report` and `swink-eval gate`
/// can consume either local artifact format.
///
/// # Errors
///
/// Returns [`ReporterError::Format`] when the input is not valid JSON for
/// either supported shape, carries an unsupported schema version, or contains
/// durations that cannot fit in [`Duration`].
pub fn decode_result_json(bytes: &[u8]) -> Result<EvalSetResult, ReporterError> {
    match serde_json::from_slice::<EvalSetResult>(bytes) {
        Ok(result) => Ok(result),
        Err(raw_err) => {
            let doc = serde_json::from_slice::<EvalResultDocOwned>(bytes).map_err(|doc_err| {
                ReporterError::Format(format!(
                    "expected raw EvalSetResult or JsonReporter artifact; \
                     raw error: {raw_err}; artifact error: {doc_err}"
                ))
            })?;
            doc.into_result()
        }
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

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum VerdictLabel {
    Pass,
    Fail,
}

#[derive(Debug, Deserialize)]
struct EvalResultDocOwned {
    schema_version: String,
    eval_set: EvalSetDocOwned,
    cases: Vec<CaseDocOwned>,
    summary: SummaryDocOwned,
    timestamp: u64,
}

#[derive(Debug, Deserialize)]
struct EvalSetDocOwned {
    id: String,
    case_count: usize,
}

#[derive(Debug, Deserialize)]
struct CaseDocOwned {
    case_id: String,
    verdict: VerdictLabel,
    duration_ms: u128,
    metrics: Vec<MetricDocOwned>,
}

#[derive(Debug, Deserialize)]
struct MetricDocOwned {
    evaluator: String,
    score: f64,
    threshold: f64,
    verdict: VerdictLabel,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SummaryDocOwned {
    total_cases: usize,
    passed: usize,
    failed: usize,
    total_cost: f64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_duration_ms: u128,
}

impl EvalResultDocOwned {
    fn into_result(self) -> Result<EvalSetResult, ReporterError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ReporterError::Format(format!(
                "unsupported eval result schema version `{}`; expected `{SCHEMA_VERSION}`",
                self.schema_version
            )));
        }
        if self.eval_set.case_count != self.cases.len() {
            return Err(ReporterError::Format(format!(
                "eval_set.case_count {} does not match {} case records",
                self.eval_set.case_count,
                self.cases.len()
            )));
        }
        Ok(EvalSetResult {
            eval_set_id: self.eval_set.id,
            case_results: self
                .cases
                .into_iter()
                .map(CaseDocOwned::into_result)
                .collect::<Result<Vec<_>, _>>()?,
            summary: self.summary.into_summary()?,
            timestamp: self.timestamp,
        })
    }
}

impl CaseDocOwned {
    fn into_result(self) -> Result<EvalCaseResult, ReporterError> {
        let duration = millis_to_duration(self.duration_ms, "case duration_ms")?;
        Ok(EvalCaseResult {
            case_id: self.case_id,
            invocation: Invocation {
                turns: Vec::new(),
                total_usage: Usage::default(),
                total_cost: Cost::default(),
                total_duration: duration,
                final_response: None,
                stop_reason: StopReason::Stop,
                model: ModelSpec::new("json-reporter", "artifact"),
            },
            metric_results: self
                .metrics
                .into_iter()
                .map(MetricDocOwned::into_result)
                .collect(),
            verdict: self.verdict.into(),
        })
    }
}

impl MetricDocOwned {
    fn into_result(self) -> EvalMetricResult {
        let _ = self.verdict;
        EvalMetricResult {
            evaluator_name: self.evaluator,
            score: Score::new(self.score, self.threshold),
            details: self.reason,
        }
    }
}

impl SummaryDocOwned {
    fn into_summary(self) -> Result<EvalSummary, ReporterError> {
        Ok(EvalSummary {
            total_cases: self.total_cases,
            passed: self.passed,
            failed: self.failed,
            total_cost: Cost {
                total: self.total_cost,
                ..Default::default()
            },
            total_usage: Usage {
                input: self.total_input_tokens,
                output: self.total_output_tokens,
                total: self
                    .total_input_tokens
                    .saturating_add(self.total_output_tokens),
                ..Default::default()
            },
            total_duration: millis_to_duration(
                self.total_duration_ms,
                "summary total_duration_ms",
            )?,
        })
    }
}

impl From<VerdictLabel> for Verdict {
    fn from(v: VerdictLabel) -> Self {
        match v {
            VerdictLabel::Pass => Self::Pass,
            VerdictLabel::Fail => Self::Fail,
        }
    }
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

fn millis_to_duration(ms: u128, field: &str) -> Result<Duration, ReporterError> {
    let millis = u64::try_from(ms)
        .map_err(|_| ReporterError::Format(format!("{field} exceeds supported duration range")))?;
    Ok(Duration::from_millis(millis))
}
