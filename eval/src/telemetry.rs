//! OpenTelemetry integration for eval runs (spec 043 US7, FR-035).
//!
//! When the `telemetry` feature is enabled, [`EvalsTelemetry`] can be wired
//! into [`EvalRunner`](crate::EvalRunner) to emit a three-level span tree
//! during `run_set`:
//!
//! * Root: `swink.eval.run_set` — attributes:
//!   `swink.eval.set_id`, `swink.eval.set_name`, `swink.eval.case_count`.
//! * Per-case child: `swink.eval.case` — attributes:
//!   `swink.eval.set_id`, `swink.eval.case_id`, `swink.eval.case_name`,
//!   `swink.eval.verdict`, `swink.eval.duration_ms`.
//! * Per-evaluator grandchild: `swink.eval.evaluator` — attributes:
//!   `swink.eval.evaluator_name`, `swink.eval.verdict`, `swink.eval.score`,
//!   `swink.eval.score_threshold`.
//!
//! Failed cases (overall verdict `Fail`) record OTel `Status::error` and an
//! `exception` event summarising the failure cause (research §R-005, §FR-035).
//!
//! If the caller has an active OTel context — e.g. an outer `agent.run` span
//! — it is inherited as the parent of `swink.eval.run_set`, enabling cross-
//! service trace correlation. Callers without an active context get a fresh
//! root trace.
//!
//! ## Example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use opentelemetry::global;
//! use swink_agent_eval::{EvalRunner, EvaluatorRegistry};
//! use swink_agent_eval::telemetry::EvalsTelemetry;
//!
//! let telemetry = EvalsTelemetry::builder()
//!     .with_tracer(global::tracer("swink.eval"))
//!     .build();
//! let runner = EvalRunner::with_defaults()
//!     .with_telemetry(Arc::new(telemetry));
//! ```

use std::borrow::Cow;
use std::time::Duration;

use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::{
    Span, SpanBuilder, SpanKind, Status, TraceContextExt, Tracer, TracerProvider,
};
use opentelemetry::{Context, KeyValue, global};

use crate::score::Verdict;
use crate::types::{EvalCase, EvalCaseResult, EvalMetricResult, EvalSet};

// ─── Attribute keys ─────────────────────────────────────────────────────────

/// Root span name for an entire `run_set` invocation.
pub const SPAN_RUN_SET: &str = "swink.eval.run_set";
/// Per-case child span name.
pub const SPAN_CASE: &str = "swink.eval.case";
/// Per-evaluator grandchild span name.
pub const SPAN_EVALUATOR: &str = "swink.eval.evaluator";

/// Eval set identifier. Present on every span in the tree.
pub const ATTR_SET_ID: &str = "swink.eval.set_id";
/// Human-readable eval set name.
pub const ATTR_SET_NAME: &str = "swink.eval.set_name";
/// Number of cases in the eval set (root span only).
pub const ATTR_CASE_COUNT: &str = "swink.eval.case_count";
/// Eval case identifier.
pub const ATTR_CASE_ID: &str = "swink.eval.case_id";
/// Human-readable case name.
pub const ATTR_CASE_NAME: &str = "swink.eval.case_name";
/// Evaluator name (e.g. `trajectory`, `response`, `budget`).
pub const ATTR_EVALUATOR_NAME: &str = "swink.eval.evaluator_name";
/// Verdict — one of `pass` or `fail`.
pub const ATTR_VERDICT: &str = "swink.eval.verdict";
/// Raw numeric score on the evaluator span.
pub const ATTR_SCORE: &str = "swink.eval.score";
/// Pass/fail threshold used to derive the verdict.
pub const ATTR_SCORE_THRESHOLD: &str = "swink.eval.score_threshold";
/// Wall-clock case duration in milliseconds.
pub const ATTR_DURATION_MS: &str = "swink.eval.duration_ms";
/// Aggregate pass/fail counters on the root span.
pub const ATTR_PASSED: &str = "swink.eval.passed";
/// Aggregate failed counter on the root span.
pub const ATTR_FAILED: &str = "swink.eval.failed";

// ─── EvalsTelemetry ─────────────────────────────────────────────────────────

/// Emits OTel spans for an entire `run_set` invocation.
///
/// Holds a [`BoxedTracer`] obtained either from a caller-supplied
/// [`TracerProvider`] or from the global provider. Cloning is cheap — the
/// tracer itself is reference-counted by the underlying SDK.
///
/// Construct via [`EvalsTelemetry::builder`].
pub struct EvalsTelemetry {
    tracer: BoxedTracer,
}

impl std::fmt::Debug for EvalsTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvalsTelemetry").finish_non_exhaustive()
    }
}

impl EvalsTelemetry {
    /// Start a new builder.
    #[must_use]
    pub fn builder() -> EvalsTelemetryBuilder {
        EvalsTelemetryBuilder::default()
    }

    /// Borrow the underlying tracer. Exposed so downstream crates can mint
    /// auxiliary spans under the same instrumentation scope.
    #[must_use]
    pub fn tracer(&self) -> &BoxedTracer {
        &self.tracer
    }

    /// Start the root `swink.eval.run_set` span.
    ///
    /// The parent is the active OTel [`Context`] (`Context::current()`), so a
    /// caller-owned span — e.g. an outer `agent.run` or a scheduler tick — is
    /// inherited automatically. When no context is active the span becomes a
    /// new root trace.
    pub(crate) fn start_run_set_span(&self, eval_set: &EvalSet) -> RunSetSpan {
        let parent = Context::current();
        let builder = SpanBuilder::from_name(Cow::Borrowed(SPAN_RUN_SET))
            .with_kind(SpanKind::Internal)
            .with_attributes(vec![
                KeyValue::new(ATTR_SET_ID, eval_set.id.clone()),
                KeyValue::new(ATTR_SET_NAME, eval_set.name.clone()),
                KeyValue::new(ATTR_CASE_COUNT, eval_set.cases.len() as i64),
            ]);
        let span = self.tracer.build_with_context(builder, &parent);
        let cx = parent.with_span(span);
        RunSetSpan {
            context: cx,
            set_id: eval_set.id.clone(),
        }
    }

    /// Start a per-case span as a child of the supplied run-set context.
    ///
    /// Accepts a cloneable [`RunSetSpanRef`] so per-case futures in
    /// `join_all` can each carry their own copy of the parent context
    /// without borrowing across await points.
    pub(crate) fn start_case_span_raw(&self, parent: &RunSetSpanRef, case: &EvalCase) -> CaseSpan {
        let builder = SpanBuilder::from_name(Cow::Borrowed(SPAN_CASE))
            .with_kind(SpanKind::Internal)
            .with_attributes(vec![
                KeyValue::new(ATTR_SET_ID, parent.set_id.clone()),
                KeyValue::new(ATTR_CASE_ID, case.id.clone()),
                KeyValue::new(ATTR_CASE_NAME, case.name.clone()),
            ]);
        let span = self.tracer.build_with_context(builder, &parent.context);
        let cx = parent.context.with_span(span);
        CaseSpan {
            context: cx,
            set_id: parent.set_id.clone(),
            case_id: case.id.clone(),
        }
    }

    /// Start a per-evaluator span as a child of the supplied case context.
    pub(crate) fn start_evaluator_span(
        &self,
        parent: &CaseSpan,
        evaluator_name: &str,
    ) -> EvaluatorSpan {
        let builder = SpanBuilder::from_name(Cow::Borrowed(SPAN_EVALUATOR))
            .with_kind(SpanKind::Internal)
            .with_attributes(vec![
                KeyValue::new(ATTR_SET_ID, parent.set_id.clone()),
                KeyValue::new(ATTR_CASE_ID, parent.case_id.clone()),
                KeyValue::new(ATTR_EVALUATOR_NAME, evaluator_name.to_string()),
            ]);
        let span = self.tracer.build_with_context(builder, &parent.context);
        let cx = parent.context.with_span(span);
        EvaluatorSpan { context: cx }
    }
}

// ─── Span handles ───────────────────────────────────────────────────────────

/// RAII-style handle for the root `swink.eval.run_set` span.
pub(crate) struct RunSetSpan {
    context: Context,
    set_id: String,
}

/// Cloneable reference to the run-set context used by per-case futures.
///
/// The owned [`RunSetSpan`] lives in the outer `run_set` frame; each future
/// in `join_all` gets its own copy of this ref so it can mint child spans
/// without borrowing across await points.
#[derive(Clone)]
pub(crate) struct RunSetSpanRef {
    pub(crate) context: Context,
    pub(crate) set_id: String,
}

impl RunSetSpan {
    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    /// Record aggregate counters and end the span.
    pub(crate) fn end(self, passed: usize, failed: usize) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(ATTR_PASSED, passed as i64));
        span.set_attribute(KeyValue::new(ATTR_FAILED, failed as i64));
        if failed > 0 {
            span.set_status(Status::error(format!("{failed} case(s) failed")));
        } else {
            span.set_status(Status::Ok);
        }
        span.end();
    }
}

/// RAII-style handle for a `swink.eval.case` span.
pub(crate) struct CaseSpan {
    context: Context,
    set_id: String,
    case_id: String,
}

impl CaseSpan {
    /// Borrow the underlying OTel [`Context`]. Exposed so the runner can
    /// parent evaluator spans off the case span.
    #[allow(dead_code)]
    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    /// Record the final verdict + duration and end the span. On failure the
    /// span receives `Status::error` plus an `exception` event whose message
    /// summarises every failing metric (FR-035).
    pub(crate) fn end(self, result: &EvalCaseResult, duration: Duration) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(ATTR_VERDICT, verdict_str(result.verdict)));
        #[allow(clippy::cast_possible_truncation)]
        span.set_attribute(KeyValue::new(
            ATTR_DURATION_MS,
            duration.as_millis().min(i64::MAX as u128) as i64,
        ));

        if !result.verdict.is_pass() {
            let failing: Vec<String> = result
                .metric_results
                .iter()
                .filter(|m| !m.score.verdict().is_pass())
                .map(|m| {
                    let detail = m.details.clone().unwrap_or_default();
                    if detail.is_empty() {
                        m.evaluator_name.clone()
                    } else {
                        format!("{}: {}", m.evaluator_name, detail)
                    }
                })
                .collect();
            let message = if failing.is_empty() {
                format!("case `{}` failed", result.case_id)
            } else {
                format!("case `{}` failed: {}", result.case_id, failing.join(" | "))
            };
            span.add_event(
                Cow::Borrowed("exception"),
                vec![
                    KeyValue::new("exception.type", "EvalCaseFailure"),
                    KeyValue::new("exception.message", message.clone()),
                ],
            );
            span.set_status(Status::error(message));
        } else {
            span.set_status(Status::Ok);
        }
        span.end();
    }
}

/// RAII-style handle for a `swink.eval.evaluator` span.
pub(crate) struct EvaluatorSpan {
    context: Context,
}

impl EvaluatorSpan {
    /// Record the metric result and end the span. Failing metrics receive
    /// `Status::error` so observability backends show the evaluator as the
    /// responsible child of a failing case.
    pub(crate) fn end(self, metric: &EvalMetricResult) {
        let span = self.context.span();
        let verdict = metric.score.verdict();
        span.set_attribute(KeyValue::new(ATTR_VERDICT, verdict_str(verdict)));
        span.set_attribute(KeyValue::new(ATTR_SCORE, metric.score.value));
        span.set_attribute(KeyValue::new(ATTR_SCORE_THRESHOLD, metric.score.threshold));
        if let Some(detail) = &metric.details {
            span.set_attribute(KeyValue::new("swink.eval.details", detail.clone()));
        }
        if !verdict.is_pass() {
            let message = metric
                .details
                .clone()
                .unwrap_or_else(|| format!("evaluator `{}` failed", metric.evaluator_name));
            span.add_event(
                Cow::Borrowed("exception"),
                vec![
                    KeyValue::new("exception.type", "EvaluatorFailure"),
                    KeyValue::new("exception.message", message.clone()),
                ],
            );
            span.set_status(Status::error(message));
        } else {
            span.set_status(Status::Ok);
        }
        span.end();
    }

    /// End the span without a metric (evaluator returned `None` — inapplicable
    /// to the case). The span is closed with `Status::Ok` to signal a no-op.
    pub(crate) fn end_inapplicable(self, evaluator_name: &str) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(
            ATTR_EVALUATOR_NAME,
            evaluator_name.to_string(),
        ));
        span.set_attribute(KeyValue::new(ATTR_VERDICT, "inapplicable"));
        span.set_status(Status::Ok);
        span.end();
    }
}

fn verdict_str(verdict: Verdict) -> &'static str {
    if verdict.is_pass() { "pass" } else { "fail" }
}

// ─── Builder ────────────────────────────────────────────────────────────────

/// Builder for [`EvalsTelemetry`].
///
/// Defaults to the globally-installed OTel [`TracerProvider`], which a
/// caller-supplied tracer can override. Use this in production to pick up
/// whatever provider is already wired (OTLP, stdout, …); in tests,
/// [`Self::with_tracer`] lets you inject a tracer backed by an
/// `InMemorySpanExporter`.
#[derive(Default)]
pub struct EvalsTelemetryBuilder {
    tracer: Option<BoxedTracer>,
}

impl EvalsTelemetryBuilder {
    /// Use a caller-supplied tracer. Most direct path for tests; wire a
    /// `SdkTracerProvider` with an `InMemorySpanExporter` and pass
    /// `provider.tracer("swink.eval")` through here.
    #[must_use]
    pub fn with_tracer(mut self, tracer: BoxedTracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Derive a tracer from an arbitrary [`TracerProvider`]. The tracer is
    /// named `swink.eval`, matching the span-name prefix.
    #[must_use]
    pub fn with_tracer_provider<S, T, P>(mut self, provider: &P) -> Self
    where
        S: opentelemetry::trace::Span + Send + Sync + 'static,
        T: Tracer<Span = S> + Send + Sync + 'static,
        P: TracerProvider<Tracer = T>,
    {
        // Any `T: Tracer<Span = S>` with the Send+Sync+'static bounds implements
        // `ObjectSafeTracer` via the blanket impl in `opentelemetry::global`.
        let tracer = provider.tracer("swink.eval");
        self.tracer = Some(BoxedTracer::new(Box::new(tracer)));
        self
    }

    /// Build the [`EvalsTelemetry`]. If no tracer has been supplied, derive
    /// one from the globally-installed provider.
    #[must_use]
    pub fn build(self) -> EvalsTelemetry {
        let tracer = self.tracer.unwrap_or_else(|| global::tracer("swink.eval"));
        EvalsTelemetry { tracer }
    }
}

// ─── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    fn fresh_provider() -> (SdkTracerProvider, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }

    #[test]
    fn builder_uses_injected_tracer() {
        let (provider, exporter) = fresh_provider();
        let telemetry = EvalsTelemetry::builder()
            .with_tracer_provider(&provider)
            .build();
        // Emit a span via the configured tracer to confirm it flows through.
        let mut span = telemetry.tracer().start("selftest");
        span.end();
        provider.force_flush().expect("flush ok");
        let spans = exporter.get_finished_spans().expect("get spans");
        assert!(spans.iter().any(|s| s.name == "selftest"));
    }

    #[test]
    fn verdict_str_rendering() {
        assert_eq!(verdict_str(Verdict::Pass), "pass");
        assert_eq!(verdict_str(Verdict::Fail), "fail");
    }
}
