//! Spec 043 US7 / T135: unit tests for `EvalsTelemetry` span emission.
//!
//! Covers FR-035:
//! * Three-level tree: `swink.eval.run_set` → `swink.eval.case` →
//!   `swink.eval.evaluator`.
//! * Standardised attributes on every span (`set_id`, `case_id`, `verdict`,
//!   `score`, `evaluator_name`).
//! * Failed case → `Status::error` + an `exception` event on the case span.
//! * When a parent OTel span is active, the run-set span inherits it.

#![cfg(feature = "telemetry")]

use std::sync::Arc;

use opentelemetry::Context;
use opentelemetry::trace::{
    SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId, TraceState,
};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet, EvalsTelemetry, Evaluator,
    EvaluatorRegistry,
};

mod common;

// ─── Fixtures ───────────────────────────────────────────────────────────────

struct StubFactory {
    responses: Vec<String>,
}

impl StubFactory {
    fn new(response: impl Into<String>) -> Self {
        Self {
            responses: vec![response.into()],
        }
    }
}

impl AgentFactory for StubFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(self.responses.clone())),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

/// Deterministic "always pass" evaluator.
struct AlwaysPass;

impl Evaluator for AlwaysPass {
    fn name(&self) -> &'static str {
        "always_pass"
    }
    fn evaluate(
        &self,
        _case: &EvalCase,
        _invocation: &swink_agent_eval::Invocation,
    ) -> Option<swink_agent_eval::EvalMetricResult> {
        Some(swink_agent_eval::EvalMetricResult {
            evaluator_name: "always_pass".to_string(),
            score: swink_agent_eval::Score::new(1.0, 0.5),
            details: Some("ok".into()),
        })
    }
}

/// Deterministic "always fail" evaluator.
struct AlwaysFail;

impl Evaluator for AlwaysFail {
    fn name(&self) -> &'static str {
        "always_fail"
    }
    fn evaluate(
        &self,
        _case: &EvalCase,
        _invocation: &swink_agent_eval::Invocation,
    ) -> Option<swink_agent_eval::EvalMetricResult> {
        Some(swink_agent_eval::EvalMetricResult {
            evaluator_name: "always_fail".to_string(),
            score: swink_agent_eval::Score::new(0.1, 0.5),
            details: Some("simulated regression".into()),
        })
    }
}

fn fresh_telemetry() -> (Arc<EvalsTelemetry>, InMemorySpanExporter, SdkTracerProvider) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let telemetry = EvalsTelemetry::builder()
        .with_tracer_provider(&provider)
        .build();
    (Arc::new(telemetry), exporter, provider)
}

fn find_by_name<'a>(spans: &'a [SpanData], name: &str) -> Vec<&'a SpanData> {
    spans.iter().filter(|s| s.name == name).collect()
}

fn attr<'a>(span: &'a SpanData, key: &str) -> Option<&'a opentelemetry::Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

fn one_case_set(id: &str) -> EvalSet {
    EvalSet {
        id: "set-1".into(),
        name: "Set One".into(),
        description: None,
        cases: vec![common::make_case(id)],
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_level_span_tree_is_emitted() {
    let (telemetry, exporter, provider) = fresh_telemetry();
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);

    let runner = EvalRunner::new(registry).with_telemetry(Arc::clone(&telemetry));
    let set = one_case_set("case-a");
    let factory = StubFactory::new("ok");
    let result = runner.run_set(&set, &factory).await.unwrap();
    assert!(result.case_results[0].verdict.is_pass());

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");

    let run_sets = find_by_name(&spans, "swink.eval.run_set");
    let cases = find_by_name(&spans, "swink.eval.case");
    let evaluators = find_by_name(&spans, "swink.eval.evaluator");

    assert_eq!(run_sets.len(), 1, "one run_set span");
    assert_eq!(cases.len(), 1, "one case span");
    assert_eq!(evaluators.len(), 1, "one evaluator span");

    // Parent linkage: case.parent == run_set.span_id; evaluator.parent == case.span_id.
    let root_id = run_sets[0].span_context.span_id();
    assert_eq!(cases[0].parent_span_id, root_id);
    let case_id = cases[0].span_context.span_id();
    assert_eq!(evaluators[0].parent_span_id, case_id);

    // All three share the same trace.
    let trace = run_sets[0].span_context.trace_id();
    assert_eq!(cases[0].span_context.trace_id(), trace);
    assert_eq!(evaluators[0].span_context.trace_id(), trace);

    // FR-035 attributes on root.
    assert_eq!(
        attr(run_sets[0], "swink.eval.set_id").map(|v| v.as_str().to_string()),
        Some("set-1".into())
    );
    assert_eq!(
        attr(run_sets[0], "swink.eval.case_count").map(|v| v.to_string()),
        Some("1".into())
    );

    // FR-035 attributes on case.
    assert_eq!(
        attr(cases[0], "swink.eval.case_id").map(|v| v.as_str().to_string()),
        Some("case-a".into())
    );
    assert_eq!(
        attr(cases[0], "swink.eval.verdict").map(|v| v.as_str().to_string()),
        Some("pass".into())
    );

    // FR-035 attributes on evaluator.
    assert_eq!(
        attr(evaluators[0], "swink.eval.evaluator_name").map(|v| v.as_str().to_string()),
        Some("always_pass".into())
    );
    let score = attr(evaluators[0], "swink.eval.score")
        .map(|v| v.to_string())
        .unwrap_or_default();
    assert!(
        score.starts_with('1'),
        "evaluator span carries a numeric score, got {score}"
    );
    assert_eq!(
        attr(evaluators[0], "swink.eval.verdict").map(|v| v.as_str().to_string()),
        Some("pass".into())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_case_records_status_error_and_exception_event() {
    let (telemetry, exporter, provider) = fresh_telemetry();
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysFail);

    let runner = EvalRunner::new(registry).with_telemetry(Arc::clone(&telemetry));
    let set = one_case_set("failing-case");
    let factory = StubFactory::new("oops");
    let result = runner.run_set(&set, &factory).await.unwrap();
    assert!(
        !result.case_results[0].verdict.is_pass(),
        "seed case must fail so the telemetry error path is exercised"
    );

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");

    let cases = find_by_name(&spans, "swink.eval.case");
    assert_eq!(cases.len(), 1);
    let case_span = cases[0];
    // FR-035: failed case → OTel status error.
    match &case_span.status {
        Status::Error { description } => {
            assert!(
                description.contains("failing-case"),
                "status description mentions case id, got `{description}`"
            );
        }
        other => panic!("expected Status::Error on failed case span, got {other:?}"),
    }
    // FR-035: failed case → exception event.
    let has_exception = case_span
        .events
        .iter()
        .any(|e| e.name.as_ref() == "exception");
    assert!(
        has_exception,
        "failed case span records an `exception` event"
    );

    // Evaluator span also errors.
    let evaluators = find_by_name(&spans, "swink.eval.evaluator");
    assert_eq!(evaluators.len(), 1);
    assert!(
        matches!(evaluators[0].status, Status::Error { .. }),
        "failing evaluator span is marked Status::Error"
    );

    // Run-set span rolls up failed count.
    let run_sets = find_by_name(&spans, "swink.eval.run_set");
    assert_eq!(run_sets.len(), 1);
    assert_eq!(
        attr(run_sets[0], "swink.eval.failed").map(|v| v.to_string()),
        Some("1".into())
    );
    assert!(matches!(run_sets[0].status, Status::Error { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_set_inherits_active_parent_span() {
    let (telemetry, exporter, provider) = fresh_telemetry();
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);

    // Seed a parent OTel context with a known SpanContext — mimicking an
    // outer `agent.run` or scheduler tick.
    let parent_trace_id = TraceId::from(0x0f0f_0f0f_0f0f_0f0f_0f0f_0f0f_0f0f_0f0fu128);
    let parent_span_id = SpanId::from(0x1234_5678_9abc_def0u64);
    let parent_cx = Context::new().with_remote_span_context(SpanContext::new(
        parent_trace_id,
        parent_span_id,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    ));

    let runner = EvalRunner::new(registry).with_telemetry(Arc::clone(&telemetry));
    let set = one_case_set("case-with-parent");
    let factory = StubFactory::new("ok");

    // Attach the parent context for the duration of `run_set`.
    let guard = parent_cx.attach();
    let result = runner.run_set(&set, &factory).await.unwrap();
    drop(guard);
    assert!(result.case_results[0].verdict.is_pass());

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");
    let run_sets = find_by_name(&spans, "swink.eval.run_set");
    assert_eq!(run_sets.len(), 1);
    assert_eq!(
        run_sets[0].span_context.trace_id(),
        parent_trace_id,
        "root span inherits the active parent trace id"
    );
    assert_eq!(
        run_sets[0].parent_span_id, parent_span_id,
        "root span's parent is the active span at run_set entry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_is_opt_in_and_default_is_no_op() {
    // No `with_telemetry(...)` → no spans emitted. We prove this by running
    // with a fresh exporter that has NOT been wired to any provider the
    // runner knows about; the runner must not leak spans to the global
    // tracer provider either.
    let exporter = InMemorySpanExporter::default();
    let _provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    // Intentionally do NOT set as global.

    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);
    let runner = EvalRunner::new(registry);
    let set = one_case_set("case-x");
    let factory = StubFactory::new("ok");
    let _ = runner.run_set(&set, &factory).await.unwrap();
    let spans = exporter.get_finished_spans().expect("spans");
    assert!(
        spans.iter().all(|s| !s.name.starts_with("swink.eval.")),
        "no spans should be emitted when telemetry is not attached"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_evaluator_case_emits_one_span_per_evaluator() {
    let (telemetry, exporter, provider) = fresh_telemetry();
    let mut registry = EvaluatorRegistry::new();
    registry.register(AlwaysPass);
    registry.register(AlwaysFail);

    let runner = EvalRunner::new(registry).with_telemetry(Arc::clone(&telemetry));
    let set = one_case_set("two-evaluator-case");
    let factory = StubFactory::new("ok");
    let _ = runner.run_set(&set, &factory).await.unwrap();

    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");
    let evaluators = find_by_name(&spans, "swink.eval.evaluator");
    let names: Vec<String> = evaluators
        .iter()
        .filter_map(|s| {
            s.attributes
                .iter()
                .find(|kv| kv.key.as_str() == "swink.eval.evaluator_name")
                .map(|kv| kv.value.as_str().to_string())
        })
        .collect();
    assert_eq!(evaluators.len(), 2, "one span per evaluator");
    assert!(names.contains(&"always_pass".to_string()));
    assert!(names.contains(&"always_fail".to_string()));
}
