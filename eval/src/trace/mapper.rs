//! `SessionMapper` trait and built-in implementations.
//!
//! Session mappers translate a backend-neutral [`RawSession`] (produced by a
//! [`TraceProvider`](crate::trace::provider::TraceProvider)) into an internal
//! [`Invocation`]. Each mapper encodes one semantic convention:
//!
//! * [`OpenInferenceSessionMapper`] — OpenInference / Arize attribute vocabulary.
//! * [`LangChainSessionMapper`] — LangChain-OTel attribute vocabulary.
//! * [`OtelGenAiSessionMapper`] — OpenTelemetry GenAI semantic conventions,
//!   with per-version attribute tables (v1.27, v1.30, experimental) per
//!   research R-005 and FR-032.
//!
//! Missing-but-required attributes surface as
//! [`MappingError::MissingAttribute`] — mappers never panic on bad input
//! (spec 043 edge case under FR-031/FR-032).

use std::collections::HashMap;
use std::time::Duration;

use opentelemetry::Value;
use opentelemetry_sdk::trace::SpanData;
use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};
use thiserror::Error;

use crate::trace::provider::RawSession;
use crate::types::{Invocation, RecordedToolCall, TurnRecord};

// ─── Error model ────────────────────────────────────────────────────────────

/// Errors a [`SessionMapper`] can surface. No mapper variant panics on
/// missing data (spec 043 FR-031 edge case).
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum MappingError {
    /// A required attribute was absent on every span in the session.
    #[error("required attribute `{name}` not found on any span in session")]
    MissingAttribute {
        /// Fully-qualified attribute key per the backend's convention.
        name: String,
    },

    /// The raw session shape is not one this mapper knows how to handle.
    #[error("raw session shape is unsupported by this mapper: {reason}")]
    UnsupportedShape {
        /// Human-readable diagnostic.
        reason: String,
    },

    /// An attribute value was present but could not be parsed into the
    /// expected Rust type (e.g. a token count that wasn't an integer).
    #[error("attribute `{name}` has unexpected value: {reason}")]
    InvalidAttribute {
        /// Fully-qualified attribute key.
        name: String,
        /// Why the value failed to parse.
        reason: String,
    },
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// Translate an external-convention [`RawSession`] into an [`Invocation`].
///
/// Mappers SHOULD be stateless and cheap to clone; the runner instantiates
/// them once per backend configuration and shares them across cases.
pub trait SessionMapper: Send + Sync {
    /// Produce an `Invocation` from the raw payload. MUST return
    /// [`MappingError::MissingAttribute`] rather than panic when required
    /// attributes are absent.
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError>;
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

// `RawSession` is `#[non_exhaustive]`; keeping a `Result` return type lets
// future backend-specific variants surface `UnsupportedShape` without
// reshaping every caller.
#[allow(clippy::unnecessary_wraps)]
fn spans_of(raw: &RawSession) -> Result<&[SpanData], MappingError> {
    match raw {
        RawSession::OtelSpans { spans, .. } => Ok(spans),
    }
}

/// Find the first span carrying attribute `key` and return the attribute
/// value as a borrowed string slice.
fn first_string_attr(spans: &[SpanData], key: &str) -> Option<String> {
    for span in spans {
        for kv in &span.attributes {
            if kv.key.as_str() == key {
                return Some(kv.value.as_str().into_owned());
            }
        }
    }
    None
}

/// Sum a numeric attribute across every span that carries it.
fn sum_u64_attr(spans: &[SpanData], key: &str) -> Result<u64, MappingError> {
    let mut sum: u64 = 0;
    for span in spans {
        for kv in &span.attributes {
            if kv.key.as_str() == key {
                let parsed =
                    match &kv.value {
                        Value::I64(v) => {
                            u64::try_from(*v).map_err(|_| MappingError::InvalidAttribute {
                                name: key.to_string(),
                                reason: format!("negative token count: {v}"),
                            })?
                        }
                        Value::String(s) => s.as_str().parse::<u64>().map_err(|e| {
                            MappingError::InvalidAttribute {
                                name: key.to_string(),
                                reason: format!("not u64: {e}"),
                            }
                        })?,
                        other => {
                            return Err(MappingError::InvalidAttribute {
                                name: key.to_string(),
                                reason: format!("expected integer, got {other:?}"),
                            });
                        }
                    };
                sum = sum.saturating_add(parsed);
            }
        }
    }
    Ok(sum)
}

fn total_duration(spans: &[SpanData]) -> Duration {
    spans
        .iter()
        .filter_map(|s| s.end_time.duration_since(s.start_time).ok())
        .sum()
}

fn current_ts() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Build a minimal `AssistantMessage` from a provider/model pair and optional
/// final response text. Used by every mapper to produce a single terminal
/// turn that carries the model-identifying attributes.
fn assistant_message(
    provider: &str,
    model: &str,
    response_text: Option<String>,
    usage: Usage,
) -> AssistantMessage {
    let mut content: Vec<ContentBlock> = Vec::new();
    if let Some(text) = response_text
        && !text.is_empty()
    {
        content.push(ContentBlock::Text { text });
    }
    AssistantMessage {
        content,
        provider: provider.to_string(),
        model_id: model.to_string(),
        usage,
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: current_ts(),
        cache_hint: None,
    }
}

/// Extract `RecordedToolCall`s from tool-kind spans using the conventional
/// `<prefix>.name` / `<prefix>.arguments` attribute pair (e.g.
/// `tool.name` / `tool.arguments` under GenAI, or `llm.tool_call.*` under
/// OpenInference). The caller supplies the `prefix` and a span-name hint.
fn extract_tool_calls(
    spans: &[SpanData],
    span_name_hint: &str,
    name_attr: &str,
    args_attr: &str,
    id_attr: Option<&str>,
) -> Vec<RecordedToolCall> {
    let mut calls = Vec::new();
    for span in spans {
        if !span.name.contains(span_name_hint) {
            continue;
        }
        let name = span
            .attributes
            .iter()
            .find(|kv| kv.key.as_str() == name_attr)
            .map(|kv| kv.value.as_str().into_owned());
        let Some(name) = name else { continue };

        let arguments = span
            .attributes
            .iter()
            .find(|kv| kv.key.as_str() == args_attr)
            .and_then(|kv| {
                let raw = kv.value.as_str();
                serde_json::from_str::<serde_json::Value>(&raw).ok()
            })
            .unwrap_or(serde_json::Value::Null);

        let id = id_attr
            .and_then(|k| {
                span.attributes
                    .iter()
                    .find(|kv| kv.key.as_str() == k)
                    .map(|kv| kv.value.as_str().into_owned())
            })
            .unwrap_or_else(|| span.span_context.span_id().to_string());

        calls.push(RecordedToolCall {
            id,
            name,
            arguments,
        });
    }
    calls
}

/// Shared backbone: build an `Invocation` from the provided provider/model,
/// usage, response, and tool calls. All three built-in mappers converge on
/// this so they return consistent shapes.
fn build_invocation(
    spans: &[SpanData],
    provider: String,
    model: String,
    usage: Usage,
    response_text: Option<String>,
    tool_calls: Vec<RecordedToolCall>,
) -> Invocation {
    let total_duration = total_duration(spans);
    let assistant = assistant_message(&provider, &model, response_text.clone(), usage.clone());
    let turn = TurnRecord {
        turn_index: 0,
        assistant_message: assistant.clone(),
        tool_calls,
        tool_results: Vec::new(),
        duration: total_duration,
    };
    Invocation {
        turns: vec![turn],
        total_usage: usage,
        total_cost: Cost::default(),
        total_duration,
        final_response: response_text,
        stop_reason: assistant.stop_reason,
        model: ModelSpec::new(provider, model),
    }
}

// ─── OpenInferenceSessionMapper (T123) ──────────────────────────────────────

/// Mapper for the OpenInference / Arize attribute vocabulary.
///
/// Consumed attribute keys (subset we require):
/// * `llm.provider`, `llm.model_name` — required; absence yields
///   [`MappingError::MissingAttribute`].
/// * `llm.token_count.prompt`, `llm.token_count.completion` — optional usage.
/// * `output.value` — optional final response text.
/// * Tool spans: `tool.name`, `tool.parameters`, `tool.call_id`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenInferenceSessionMapper;

impl OpenInferenceSessionMapper {
    pub const PROVIDER_KEY: &'static str = "llm.provider";
    pub const MODEL_KEY: &'static str = "llm.model_name";
    pub const INPUT_TOKENS_KEY: &'static str = "llm.token_count.prompt";
    pub const OUTPUT_TOKENS_KEY: &'static str = "llm.token_count.completion";
    pub const RESPONSE_KEY: &'static str = "output.value";
    pub const TOOL_NAME_KEY: &'static str = "tool.name";
    pub const TOOL_PARAMS_KEY: &'static str = "tool.parameters";
    pub const TOOL_ID_KEY: &'static str = "tool.call_id";
}

impl SessionMapper for OpenInferenceSessionMapper {
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError> {
        let spans = spans_of(raw)?;

        let provider = first_string_attr(spans, Self::PROVIDER_KEY).ok_or_else(|| {
            MappingError::MissingAttribute {
                name: Self::PROVIDER_KEY.to_string(),
            }
        })?;
        let model = first_string_attr(spans, Self::MODEL_KEY).ok_or_else(|| {
            MappingError::MissingAttribute {
                name: Self::MODEL_KEY.to_string(),
            }
        })?;

        let input = sum_u64_attr(spans, Self::INPUT_TOKENS_KEY)?;
        let output = sum_u64_attr(spans, Self::OUTPUT_TOKENS_KEY)?;
        let usage = Usage {
            input,
            output,
            total: input.saturating_add(output),
            ..Usage::default()
        };
        let response = first_string_attr(spans, Self::RESPONSE_KEY);

        let tool_calls = extract_tool_calls(
            spans,
            "tool",
            Self::TOOL_NAME_KEY,
            Self::TOOL_PARAMS_KEY,
            Some(Self::TOOL_ID_KEY),
        );

        Ok(build_invocation(
            spans, provider, model, usage, response, tool_calls,
        ))
    }
}

// ─── LangChainSessionMapper (T124) ──────────────────────────────────────────

/// Mapper for LangChain-OTel traces.
///
/// Consumed attribute keys:
/// * `langchain.llm.provider`, `langchain.llm.model` — required.
/// * `langchain.llm.usage.prompt_tokens`, `langchain.llm.usage.completion_tokens`
///   — optional usage.
/// * `langchain.llm.output_text` — optional final response.
/// * Tool spans: `langchain.tool.name`, `langchain.tool.input`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LangChainSessionMapper;

impl LangChainSessionMapper {
    pub const PROVIDER_KEY: &'static str = "langchain.llm.provider";
    pub const MODEL_KEY: &'static str = "langchain.llm.model";
    pub const INPUT_TOKENS_KEY: &'static str = "langchain.llm.usage.prompt_tokens";
    pub const OUTPUT_TOKENS_KEY: &'static str = "langchain.llm.usage.completion_tokens";
    pub const RESPONSE_KEY: &'static str = "langchain.llm.output_text";
    pub const TOOL_NAME_KEY: &'static str = "langchain.tool.name";
    pub const TOOL_INPUT_KEY: &'static str = "langchain.tool.input";
    pub const TOOL_ID_KEY: &'static str = "langchain.tool.run_id";
}

impl SessionMapper for LangChainSessionMapper {
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError> {
        let spans = spans_of(raw)?;

        let provider = first_string_attr(spans, Self::PROVIDER_KEY).ok_or_else(|| {
            MappingError::MissingAttribute {
                name: Self::PROVIDER_KEY.to_string(),
            }
        })?;
        let model = first_string_attr(spans, Self::MODEL_KEY).ok_or_else(|| {
            MappingError::MissingAttribute {
                name: Self::MODEL_KEY.to_string(),
            }
        })?;

        let input = sum_u64_attr(spans, Self::INPUT_TOKENS_KEY)?;
        let output = sum_u64_attr(spans, Self::OUTPUT_TOKENS_KEY)?;
        let usage = Usage {
            input,
            output,
            total: input.saturating_add(output),
            ..Usage::default()
        };
        let response = first_string_attr(spans, Self::RESPONSE_KEY);

        let tool_calls = extract_tool_calls(
            spans,
            "tool",
            Self::TOOL_NAME_KEY,
            Self::TOOL_INPUT_KEY,
            Some(Self::TOOL_ID_KEY),
        );

        Ok(build_invocation(
            spans, provider, model, usage, response, tool_calls,
        ))
    }
}

// ─── OtelGenAiSessionMapper + version enum (T125) ───────────────────────────

/// OpenTelemetry GenAI semantic-convention versions supported by
/// [`OtelGenAiSessionMapper`] (research R-005, FR-032).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenAIConventionVersion {
    /// `gen_ai.*` vocabulary as of OTel semantic conventions v1.27.
    V1_27,
    /// v1.30 — introduces `gen_ai.operation.name` and renames some response
    /// attributes.
    V1_30,
    /// Forward-compatible bucket: the mapper consumes the v1.30 attribute
    /// set and tolerates unknown extras.
    Experimental,
}

/// Attribute-key lookup table for a single GenAI convention version.
///
/// Exposed (rather than hidden inside the mapper) so out-of-tree callers can
/// inspect exactly which keys are consumed per version.
#[derive(Debug, Clone)]
pub struct GenAIAttributeTable {
    pub system: &'static str,
    pub request_model: &'static str,
    pub response_model: &'static str,
    pub input_tokens: &'static str,
    pub output_tokens: &'static str,
    pub finish_reasons: &'static str,
    pub response_text: &'static str,
    pub tool_name: &'static str,
    pub tool_arguments: &'static str,
    pub tool_id: &'static str,
}

impl GenAIAttributeTable {
    /// Per-version attribute table (FR-032).
    #[must_use]
    pub const fn for_version(version: GenAIConventionVersion) -> Self {
        match version {
            GenAIConventionVersion::V1_27 => Self {
                system: "gen_ai.system",
                request_model: "gen_ai.request.model",
                response_model: "gen_ai.response.model",
                input_tokens: "gen_ai.usage.input_tokens",
                output_tokens: "gen_ai.usage.output_tokens",
                finish_reasons: "gen_ai.response.finish_reasons",
                response_text: "gen_ai.completion.content",
                tool_name: "gen_ai.tool.name",
                tool_arguments: "gen_ai.tool.call.arguments",
                tool_id: "gen_ai.tool.call.id",
            },
            GenAIConventionVersion::V1_30 | GenAIConventionVersion::Experimental => Self {
                system: "gen_ai.system",
                request_model: "gen_ai.request.model",
                response_model: "gen_ai.response.model",
                input_tokens: "gen_ai.usage.input_tokens",
                output_tokens: "gen_ai.usage.output_tokens",
                finish_reasons: "gen_ai.response.finish_reasons",
                response_text: "gen_ai.output.messages",
                tool_name: "gen_ai.tool.name",
                tool_arguments: "gen_ai.tool.arguments",
                tool_id: "gen_ai.tool.call.id",
            },
        }
    }
}

/// Mapper for OTel GenAI semantic conventions.
///
/// Construction pins a [`GenAIConventionVersion`]; swapping versions requires
/// no code change on the caller side — just re-construct with a different
/// enum variant (FR-032 independent-test criterion).
#[derive(Debug, Clone)]
pub struct OtelGenAiSessionMapper {
    /// Convention version this mapper targets.
    pub version: GenAIConventionVersion,
    table: GenAIAttributeTable,
}

impl OtelGenAiSessionMapper {
    /// Build a mapper pinned to `version`.
    #[must_use]
    pub fn new(version: GenAIConventionVersion) -> Self {
        Self {
            version,
            table: GenAIAttributeTable::for_version(version),
        }
    }

    /// Attribute table this mapper uses.
    #[must_use]
    pub fn table(&self) -> &GenAIAttributeTable {
        &self.table
    }
}

impl Default for OtelGenAiSessionMapper {
    fn default() -> Self {
        Self::new(GenAIConventionVersion::V1_30)
    }
}

impl SessionMapper for OtelGenAiSessionMapper {
    fn map(&self, raw: &RawSession) -> Result<Invocation, MappingError> {
        let spans = spans_of(raw)?;
        let tbl = &self.table;

        let provider =
            first_string_attr(spans, tbl.system).ok_or_else(|| MappingError::MissingAttribute {
                name: tbl.system.to_string(),
            })?;
        let model = first_string_attr(spans, tbl.response_model)
            .or_else(|| first_string_attr(spans, tbl.request_model))
            .ok_or_else(|| MappingError::MissingAttribute {
                name: tbl.request_model.to_string(),
            })?;

        let input = sum_u64_attr(spans, tbl.input_tokens)?;
        let output = sum_u64_attr(spans, tbl.output_tokens)?;
        let usage = Usage {
            input,
            output,
            total: input.saturating_add(output),
            ..Usage::default()
        };
        let response = first_string_attr(spans, tbl.response_text);

        let tool_calls = extract_tool_calls(
            spans,
            "tool",
            tbl.tool_name,
            tbl.tool_arguments,
            Some(tbl.tool_id),
        );

        // Unknown attributes are tolerated for Experimental. We surface them
        // as a debug log but never error out.
        if matches!(self.version, GenAIConventionVersion::Experimental) {
            let known: [&str; 10] = [
                tbl.system,
                tbl.request_model,
                tbl.response_model,
                tbl.input_tokens,
                tbl.output_tokens,
                tbl.finish_reasons,
                tbl.response_text,
                tbl.tool_name,
                tbl.tool_arguments,
                tbl.tool_id,
            ];
            let mut seen_unknown: HashMap<String, usize> = HashMap::new();
            for span in spans {
                for kv in &span.attributes {
                    let k = kv.key.as_str();
                    if k.starts_with("gen_ai.") && !known.contains(&k) {
                        *seen_unknown.entry(k.to_string()).or_default() += 1;
                    }
                }
            }
            if !seen_unknown.is_empty() {
                tracing::debug!(
                    target: "swink_agent_eval::trace::mapper",
                    unknown = ?seen_unknown,
                    "OtelGenAiSessionMapper (Experimental) tolerating unknown gen_ai.* attributes"
                );
            }
        }

        Ok(build_invocation(
            spans, provider, model, usage, response, tool_calls,
        ))
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
