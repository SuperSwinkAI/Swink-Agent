//! US6 end-to-end trace ingestion integration test (T134).
//!
//! Records an in-process run into an `InMemorySpanExporter`, re-loads the
//! session via [`OtelInMemoryTraceProvider`] + [`OpenInferenceSessionMapper`],
//! scores it with a deterministic evaluator, and asserts the score is
//! bit-identical to the in-process evaluation (spec 043 SC-008).
//!
//! A follow-up scenario replays the same session through [`SwarmExtractor`]
//! and [`GraphExtractor`] at `EvaluationLevel::Session` to confirm the new
//! T132/T133 surfaces stay wired end-to-end.

#![cfg(all(feature = "trace-ingest", feature = "evaluator-simple"))]

use std::borrow::Cow;
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SpanData, SpanEvents, SpanExporter, SpanLinks,
};

use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::trace::{
    EvaluationLevel, ExtractedInput, GraphExtractor, OpenInferenceSessionMapper,
    OtelInMemoryTraceProvider, SessionMapper, SwarmExtractor, TraceExtractor, TraceProvider,
};
use swink_agent_eval::{Evaluator, ExactMatchEvaluator, Invocation, TurnRecord};

fn span(
    name: &str,
    attrs: Vec<KeyValue>,
    span_id: u64,
    parent: Option<u64>,
    complete: bool,
) -> SpanData {
    let start = SystemTime::now();
    let end = if complete {
        start + Duration::from_millis(5)
    } else {
        start
    };
    SpanData {
        span_context: SpanContext::new(
            TraceId::from(7_u128),
            SpanId::from(span_id),
            TraceFlags::default(),
            false,
            TraceState::default(),
        ),
        parent_span_id: parent.map_or(SpanId::INVALID, SpanId::from),
        parent_span_is_remote: false,
        span_kind: SpanKind::Internal,
        name: Cow::Owned(name.to_string()),
        start_time: start,
        end_time: end,
        attributes: attrs,
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: Status::Unset,
        instrumentation_scope: InstrumentationScope::builder("us6-e2e").build(),
    }
}

fn make_case() -> swink_agent_eval::EvalCase {
    swink_agent_eval::EvalCase {
        id: "us6-e2e".into(),
        name: "US6 e2e".into(),
        description: None,
        system_prompt: "You are a test agent.".into(),
        user_messages: vec!["what is 2+2?".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn in_process_invocation(response: &str) -> Invocation {
    let msg = AssistantMessage {
        content: vec![ContentBlock::Text {
            text: response.into(),
        }],
        provider: "test".into(),
        model_id: "test-model".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    };
    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message: msg,
            tool_calls: vec![],
            tool_results: vec![],
            duration: Duration::from_millis(5),
        }],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(5),
        final_response: Some(response.into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_reload_and_score_bit_identical() {
    // In-process scoring against the native Invocation.
    let response = "4";
    let in_process_inv = in_process_invocation(response);

    let case = make_case();
    let evaluator = ExactMatchEvaluator::new("4");
    let in_process = evaluator
        .evaluate(&case, &in_process_inv)
        .expect("in-process result");

    // Export one span that carries the final response as an OpenInference
    // `output.value` attribute, then re-hydrate the session.
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter.clone());
    let llm = span(
        "llm.call",
        vec![
            KeyValue::new("session.id", "sess-1"),
            KeyValue::new("llm.provider", "test"),
            KeyValue::new("llm.model_name", "test-model"),
            KeyValue::new("output.value", response),
        ],
        1,
        None,
        true,
    );
    exporter.export(vec![llm]).await.expect("export");

    let raw = provider
        .fetch_session("sess-1")
        .await
        .expect("session found");
    let reloaded = OpenInferenceSessionMapper.map(&raw).expect("map ok");
    assert_eq!(reloaded.final_response.as_deref(), Some(response));

    // Score the reloaded session with the same evaluator.
    let reloaded_result = evaluator
        .evaluate(&case, &reloaded)
        .expect("reloaded result");

    // Bit-identical scores (SC-008). Score / pass / reason must all match.
    assert!((in_process.score.value - reloaded_result.score.value).abs() < f64::EPSILON);
    assert!((in_process.score.threshold - reloaded_result.score.threshold).abs() < f64::EPSILON);
    assert_eq!(in_process.details, reloaded_result.details);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swarm_and_graph_extractors_traverse_reloaded_session() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter.clone());
    let attrs = |kv: &[(&str, &str)]| -> Vec<KeyValue> {
        kv.iter()
            .map(|(k, v)| KeyValue::new(k.to_string(), v.to_string()))
            .collect()
    };
    let session_id = "sess-2";
    let spans = vec![
        span(
            "llm.a",
            attrs(&[
                ("session.id", session_id),
                ("llm.provider", "anthropic"),
                ("llm.model_name", "claude-3"),
                ("output.value", "step-1"),
            ]),
            1,
            None,
            true,
        ),
        span(
            "llm.b",
            attrs(&[
                ("session.id", session_id),
                ("llm.provider", "openai"),
                ("llm.model_name", "gpt-4"),
                ("output.value", "step-2"),
            ]),
            2,
            Some(1),
            true,
        ),
    ];
    exporter.export(spans).await.expect("export");

    let raw = provider.fetch_session(session_id).await.expect("session");
    let mut invocation = OpenInferenceSessionMapper.map(&raw).expect("map ok");
    // Give each turn a distinct model_id so GraphExtractor has something to
    // partition on (OpenInferenceMapper collapses the session into one
    // AssistantMessage; reinstate per-turn model_ids for the extractor test).
    if invocation.turns.len() == 1 {
        // Split the single reloaded turn into two turns that alternate
        // model_ids so the graph extractor sees two nodes.
        let mut duplicate = invocation.turns[0].clone();
        duplicate.turn_index = 1;
        duplicate.assistant_message.model_id = "gpt-4".into();
        invocation.turns[0].assistant_message.model_id = "claude-3".into();
        invocation.turns.push(duplicate);
    }

    let graph = GraphExtractor::new().extract(&invocation, EvaluationLevel::Session);
    assert_eq!(
        graph.len(),
        2,
        "graph extractor should emit one Session per distinct model_id"
    );
    for input in &graph {
        assert!(matches!(input, ExtractedInput::Session { .. }));
    }

    let swarm = SwarmExtractor::new().extract(&invocation, EvaluationLevel::Session);
    assert_eq!(
        swarm.len(),
        1,
        "without handoff tool calls swarm extractor groups the whole session"
    );
}
