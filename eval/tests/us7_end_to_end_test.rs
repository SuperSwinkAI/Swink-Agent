//! Spec 043 US7 / T138: end-to-end assertion of the US7 user story.
//!
//! Runs a mixed eval set (some passing, some regressing on `correctness`) and
//! confirms:
//!
//! 1. A full three-level span tree is emitted:
//!    `swink.eval.run_set` → `swink.eval.case`* → `swink.eval.evaluator`*.
//! 2. A regression at a known case surfaces on its `swink.eval.case` span as
//!    `Status::error` + `exception` event — per US7 scenario 3 in
//!    `specs/043-evals-adv-features/spec.md`.

#![cfg(feature = "telemetry")]

use std::sync::Arc;

use opentelemetry::trace::{Status, TracerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalMetricResult, EvalRunner, EvalSet, EvalsTelemetry,
    Evaluator, EvaluatorRegistry, Invocation, Score,
};

mod common;

// ─── Fixture: a "correctness" evaluator that regresses on a known case ──────

/// Emits a passing score for every case except those in `regressed_ids`,
/// which receive a failing score — emulating a real regression in
/// `correctness` surfacing through `EvalsTelemetry` (US7 scenario 3).
struct CorrectnessEvaluator {
    regressed_ids: Vec<String>,
}

impl Evaluator for CorrectnessEvaluator {
    fn name(&self) -> &'static str {
        "correctness"
    }
    fn evaluate(&self, case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        let regressed = self.regressed_ids.iter().any(|id| id == &case.id);
        let score = if regressed {
            Score::new(0.2, 0.7)
        } else {
            Score::new(0.95, 0.7)
        };
        Some(EvalMetricResult {
            evaluator_name: "correctness".to_string(),
            score,
            details: if regressed {
                Some(format!("regression on case `{}`", case.id))
            } else {
                None
            },
        })
    }
}

struct EchoFactory;

impl AgentFactory for EchoFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["done".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

fn find_by_name<'a>(spans: &'a [SpanData], name: &str) -> Vec<&'a SpanData> {
    spans.iter().filter(|s| s.name == name).collect()
}

fn case_id_attr(span: &SpanData) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == "swink.eval.case_id")
        .map(|kv| kv.value.as_str().to_string())
}

fn evaluator_name_attr(span: &SpanData) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == "swink.eval.evaluator_name")
        .map(|kv| kv.value.as_str().to_string())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn us7_full_run_produces_expected_span_tree() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("us7-smoke");
    let telemetry = EvalsTelemetry::builder()
        .with_tracer(opentelemetry::global::BoxedTracer::new(Box::new(tracer)))
        .build();

    let mut registry = EvaluatorRegistry::new();
    registry.register(CorrectnessEvaluator {
        regressed_ids: vec![], // happy path — no regressions
    });

    let set = EvalSet {
        id: "us7-smoke".into(),
        name: "US7 smoke".into(),
        description: None,
        cases: (0..3)
            .map(|i| common::make_case(&format!("case-{i:02}")))
            .collect(),
    };

    let runner = EvalRunner::new(registry)
        .with_parallelism(2)
        .with_telemetry(Arc::new(telemetry));
    let factory = EchoFactory;

    let result = runner.run_set(&set, &factory).await.unwrap();
    assert_eq!(result.summary.passed, 3);
    assert_eq!(result.summary.failed, 0);

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");

    let run_sets = find_by_name(&spans, "swink.eval.run_set");
    let cases = find_by_name(&spans, "swink.eval.case");
    let evaluators = find_by_name(&spans, "swink.eval.evaluator");
    assert_eq!(run_sets.len(), 1);
    assert_eq!(cases.len(), 3);
    assert_eq!(evaluators.len(), 3);

    // Each case span is a child of the run-set span.
    let root_id = run_sets[0].span_context.span_id();
    for case_span in &cases {
        assert_eq!(case_span.parent_span_id, root_id);
    }

    // Each evaluator span is a child of a case span.
    let case_ids: std::collections::HashSet<_> =
        cases.iter().map(|s| s.span_context.span_id()).collect();
    for ev in &evaluators {
        assert!(
            case_ids.contains(&ev.parent_span_id),
            "evaluator span's parent must be a case span"
        );
    }

    // Every span shares the same trace id.
    let trace = run_sets[0].span_context.trace_id();
    for s in spans.iter().filter(|s| s.name.starts_with("swink.eval.")) {
        assert_eq!(s.span_context.trace_id(), trace);
    }

    // No errors on a fully-passing run.
    assert!(matches!(run_sets[0].status, Status::Ok | Status::Unset));
    for case_span in &cases {
        assert!(matches!(case_span.status, Status::Ok));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn us7_regression_on_known_case_surfaces_as_errored_span() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let telemetry = EvalsTelemetry::builder()
        .with_tracer_provider(&provider)
        .build();

    let regressed = "case-01".to_string();
    let mut registry = EvaluatorRegistry::new();
    registry.register(CorrectnessEvaluator {
        regressed_ids: vec![regressed.clone()],
    });

    let set = EvalSet {
        id: "us7-regression".into(),
        name: "US7 regression".into(),
        description: None,
        cases: (0..3)
            .map(|i| common::make_case(&format!("case-{i:02}")))
            .collect(),
    };

    let runner = EvalRunner::new(registry)
        .with_parallelism(2)
        .with_telemetry(Arc::new(telemetry));
    let factory = EchoFactory;

    let result = runner.run_set(&set, &factory).await.unwrap();
    assert_eq!(result.summary.passed, 2);
    assert_eq!(result.summary.failed, 1);

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");

    let cases = find_by_name(&spans, "swink.eval.case");
    let evaluators = find_by_name(&spans, "swink.eval.evaluator");

    // Exactly the regressed case has Status::Error and an exception event.
    let failing_case_spans: Vec<&&SpanData> = cases
        .iter()
        .filter(|s| matches!(s.status, Status::Error { .. }))
        .collect();
    assert_eq!(
        failing_case_spans.len(),
        1,
        "exactly one failing case span on a single-regression run"
    );
    assert_eq!(case_id_attr(failing_case_spans[0]), Some(regressed.clone()));
    assert!(
        failing_case_spans[0]
            .events
            .iter()
            .any(|e| e.name.as_ref() == "exception"),
        "failed case span carries an `exception` event (per US7 scenario 3)"
    );

    // The failing evaluator span is marked Error + carries the correct name.
    let failing_evaluator_spans: Vec<&&SpanData> = evaluators
        .iter()
        .filter(|s| matches!(s.status, Status::Error { .. }))
        .collect();
    assert_eq!(
        failing_evaluator_spans.len(),
        1,
        "exactly one failing evaluator span"
    );
    assert_eq!(
        evaluator_name_attr(failing_evaluator_spans[0]),
        Some("correctness".to_string())
    );

    // Passing cases remain Ok.
    let passing_count = cases
        .iter()
        .filter(|s| matches!(s.status, Status::Ok))
        .count();
    assert_eq!(passing_count, 2, "non-regressed cases stay at Status::Ok");
}
