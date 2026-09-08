//! Tests for `mapper`.
#![cfg(test)]

use super::*;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};
use std::borrow::Cow;
use std::time::{Duration, SystemTime};

fn make_span(name: &str, attrs: Vec<KeyValue>) -> SpanData {
    let start = SystemTime::now();
    SpanData {
        span_context: SpanContext::new(
            TraceId::from(7_u128),
            SpanId::from(7_u64),
            TraceFlags::default(),
            false,
            TraceState::default(),
        ),
        parent_span_id: SpanId::INVALID,
        parent_span_is_remote: false,
        span_kind: SpanKind::Internal,
        name: Cow::Owned(name.to_string()),
        start_time: start,
        end_time: start + Duration::from_millis(5),
        attributes: attrs,
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: Status::Unset,
        instrumentation_scope: InstrumentationScope::builder("test").build(),
    }
}

fn session(spans: Vec<SpanData>) -> RawSession {
    RawSession::OtelSpans {
        session_id: "s".into(),
        spans,
    }
}

#[test]
fn openinference_missing_provider_returns_missing_attribute() {
    let raw = session(vec![make_span(
        "llm",
        vec![KeyValue::new("llm.model_name", "m")],
    )]);
    let err = OpenInferenceSessionMapper
        .map(&raw)
        .expect_err("provider absent");
    match err {
        MappingError::MissingAttribute { name } => {
            assert_eq!(name, OpenInferenceSessionMapper::PROVIDER_KEY);
        }
        other => panic!("expected MissingAttribute, got {other:?}"),
    }
}

#[test]
fn openinference_builds_invocation_with_usage_and_tool_calls() {
    let llm = make_span(
        "llm",
        vec![
            KeyValue::new("llm.provider", "anthropic"),
            KeyValue::new("llm.model_name", "claude-3"),
            KeyValue::new("llm.token_count.prompt", 10_i64),
            KeyValue::new("llm.token_count.completion", 20_i64),
            KeyValue::new("output.value", "hello"),
        ],
    );
    let tool = make_span(
        "tool.exec",
        vec![
            KeyValue::new("tool.name", "read_file"),
            KeyValue::new("tool.parameters", r#"{"path":"/etc"}"#),
            KeyValue::new("tool.call_id", "call_42"),
        ],
    );
    let inv = OpenInferenceSessionMapper
        .map(&session(vec![llm, tool]))
        .unwrap();
    assert_eq!(inv.model.provider, "anthropic");
    assert_eq!(inv.model.model_id, "claude-3");
    assert_eq!(inv.total_usage.input, 10);
    assert_eq!(inv.total_usage.output, 20);
    assert_eq!(inv.total_usage.total, 30);
    assert_eq!(inv.final_response.as_deref(), Some("hello"));
    assert_eq!(inv.turns.len(), 1);
    assert_eq!(inv.turns[0].tool_calls.len(), 1);
    assert_eq!(inv.turns[0].tool_calls[0].name, "read_file");
    assert_eq!(inv.turns[0].tool_calls[0].id, "call_42");
}

#[test]
fn langchain_missing_model_returns_missing_attribute() {
    let raw = session(vec![make_span(
        "chain",
        vec![KeyValue::new("langchain.llm.provider", "openai")],
    )]);
    let err = LangChainSessionMapper.map(&raw).expect_err("model absent");
    match err {
        MappingError::MissingAttribute { name } => {
            assert_eq!(name, LangChainSessionMapper::MODEL_KEY);
        }
        other => panic!("expected MissingAttribute, got {other:?}"),
    }
}

#[test]
fn langchain_round_trips_provider_and_tokens() {
    let raw = session(vec![make_span(
        "chain",
        vec![
            KeyValue::new("langchain.llm.provider", "openai"),
            KeyValue::new("langchain.llm.model", "gpt-4"),
            KeyValue::new("langchain.llm.usage.prompt_tokens", 3_i64),
            KeyValue::new("langchain.llm.usage.completion_tokens", 4_i64),
        ],
    )]);
    let inv = LangChainSessionMapper.map(&raw).unwrap();
    assert_eq!(inv.model.provider, "openai");
    assert_eq!(inv.model.model_id, "gpt-4");
    assert_eq!(inv.total_usage.total, 7);
}

#[test]
fn genai_v1_27_and_v1_30_have_distinct_response_keys() {
    let t27 = GenAIAttributeTable::for_version(GenAIConventionVersion::V1_27);
    let t30 = GenAIAttributeTable::for_version(GenAIConventionVersion::V1_30);
    assert_ne!(t27.response_text, t30.response_text);
    assert_eq!(t27.system, t30.system); // `gen_ai.system` stable across versions.
}

#[test]
fn genai_missing_system_returns_missing_attribute() {
    let raw = session(vec![make_span(
        "llm.call",
        vec![KeyValue::new("gen_ai.request.model", "m")],
    )]);
    let err = OtelGenAiSessionMapper::new(GenAIConventionVersion::V1_30)
        .map(&raw)
        .expect_err("system absent");
    assert!(matches!(err, MappingError::MissingAttribute { name } if name == "gen_ai.system"));
}

#[test]
fn genai_v1_30_maps_usage_and_tool_call() {
    let llm = make_span(
        "llm.call",
        vec![
            KeyValue::new("gen_ai.system", "anthropic"),
            KeyValue::new("gen_ai.request.model", "claude-3"),
            KeyValue::new("gen_ai.usage.input_tokens", 5_i64),
            KeyValue::new("gen_ai.usage.output_tokens", 6_i64),
        ],
    );
    let tool = make_span(
        "tool.call",
        vec![
            KeyValue::new("gen_ai.tool.name", "search"),
            KeyValue::new("gen_ai.tool.arguments", r#"{"q":"rust"}"#),
            KeyValue::new("gen_ai.tool.call.id", "tc_1"),
        ],
    );
    let inv = OtelGenAiSessionMapper::new(GenAIConventionVersion::V1_30)
        .map(&session(vec![llm, tool]))
        .unwrap();
    assert_eq!(inv.total_usage.input, 5);
    assert_eq!(inv.total_usage.output, 6);
    assert_eq!(inv.turns[0].tool_calls[0].name, "search");
}

#[test]
fn genai_experimental_tolerates_unknown_attributes() {
    let llm = make_span(
        "llm.call",
        vec![
            KeyValue::new("gen_ai.system", "openai"),
            KeyValue::new("gen_ai.request.model", "gpt-5"),
            KeyValue::new("gen_ai.wildcard.future_thing", "yes"),
        ],
    );
    let inv = OtelGenAiSessionMapper::new(GenAIConventionVersion::Experimental)
        .map(&session(vec![llm]))
        .expect("experimental ignores unknown gen_ai.* keys");
    assert_eq!(inv.model.provider, "openai");
}
