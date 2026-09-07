//! Shared protocol shell for the OpenAI **Responses** API.
//!
//! This is to `/v1/responses` what [`oai_transport`](crate::oai_transport)
//! is to `/v1/chat/completions`: one shell, thin per-provider wrappers.
//! The two protocols differ on the wire in every layer —
//!
//! | Layer | Chat Completions | Responses |
//! |---|---|---|
//! | history | `messages: [{role, content}]` | `input: [{type: "message" \| "function_call" \| "function_call_output", …}]` |
//! | system prompt | a `system` message | top-level `instructions` |
//! | tools | `{type: "function", function: {name, …}}` (nested) | `{type: "function", name, …}` (flat) |
//! | reasoning | — | `reasoning: {effort}` |
//! | stream | untyped `data:` chunks + `[DONE]` | typed `event:`/`data:` pairs ending in `response.completed` |
//! | usage | `prompt_tokens` / `completion_tokens` | `input_tokens` / `output_tokens` (+ cached / reasoning details) |
//!
//! — but the [`AssistantMessageEvent`] stream this shell produces is
//! indistinguishable from every other adapter's, so nothing downstream of
//! [`StreamFn`] can tell the protocols apart.
//!
//! `store: false` and a non-empty `instructions` are always sent: the Codex
//! subscription backend requires both and they are harmless elsewhere.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::{
    AgentContext, AgentTool, AssistantMessage, AssistantMessageEvent, ContentBlock, Cost,
    MessageConverter, ModelSpec, ResponseFormat, ServingOptionSupport, StopReason, StreamFn,
    StreamOptions, ThinkingLevel, ToolResultMessage, Usage, UserMessage, convert_messages,
    extract_tool_schemas,
};

use crate::base::AdapterBase;
use crate::block_accumulator::BlockAccumulator;
use crate::sse::{
    SSE_PROTOCOL_ERROR_EVENT, SSE_TRANSPORT_ERROR_EVENT, SseAction, SseEvent, sse_adapter_stream,
    sse_paired_events_with_callback,
};

/// Default `instructions` when the context carries no system prompt. The
/// Codex backend rejects an empty string, so this is never omitted.
const DEFAULT_INSTRUCTIONS: &str = "You are a helpful assistant.";

// ─── Request types ──────────────────────────────────────────────────────────

/// One item of the Responses `input` array.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputItem {
    Message {
        role: &'static str,
        content: Vec<ContentPart>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

/// A content part inside an `input` message. User turns carry
/// `input_text`; replayed assistant turns carry `output_text`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    InputText { text: String },
    OutputText { text: String },
}

/// Tool definition in the Responses shape: **flat**, no `function` nesting.
#[derive(Debug, Serialize)]
struct ResponsesTool {
    r#type: &'static str,
    name: String,
    description: String,
    parameters: Value,
    strict: bool,
}

#[derive(Debug, Serialize)]
struct ResponsesReasoning {
    effort: &'static str,
}

/// Full request body for `POST …/responses`.
#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    instructions: String,
    input: Vec<InputItem>,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ResponsesReasoning>,
    /// Structured output: `{"format": {"type": …}}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ResponsesTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    /// [`ServingOptions::extra`](swink_agent::ServingOptions) minus the
    /// typed keys above.
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

/// Marker type for Responses `input` conversion.
///
/// One harness message can expand into several input items (an assistant
/// turn with text *and* tool calls), so the converter yields a `Vec` per
/// message and the caller flattens.
struct ResponsesConverter;

impl MessageConverter for ResponsesConverter {
    type Message = Vec<InputItem>;

    // The system prompt travels as top-level `instructions`, not an item.
    fn system_message(_system_prompt: &str) -> Option<Self::Message> {
        None
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        vec![InputItem::Message {
            role: "user",
            content: vec![ContentPart::InputText {
                text: ContentBlock::extract_text(&user.content),
            }],
        }]
    }

    fn assistant_message(assistant: &AssistantMessage) -> Self::Message {
        let mut items = Vec::new();
        let mut text = String::new();
        for block in &assistant.content {
            match block {
                ContentBlock::Text { text: t } => text.push_str(t),
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    debug_assert!(
                        arguments.is_object(),
                        "responses adapter: function_call.arguments must stringify from a JSON object (got {arguments:?})"
                    );
                    items.push(InputItem::FunctionCall {
                        call_id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.to_string(),
                    });
                }
                _ => {}
            }
        }
        if !text.is_empty() {
            items.insert(
                0,
                InputItem::Message {
                    role: "assistant",
                    content: vec![ContentPart::OutputText { text }],
                },
            );
        }
        items
    }

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        vec![InputItem::FunctionCallOutput {
            call_id: result.tool_call_id.clone(),
            output: ContentBlock::extract_text(&result.content),
        }]
    }
}

fn build_tools(tools: &[Arc<dyn AgentTool>]) -> (Vec<ResponsesTool>, Option<&'static str>) {
    let tools: Vec<ResponsesTool> = extract_tool_schemas(tools)
        .into_iter()
        .map(|s| ResponsesTool {
            r#type: "function",
            name: s.name,
            description: s.description,
            parameters: s.parameters,
            strict: false,
        })
        .collect();
    let choice = (!tools.is_empty()).then_some("auto");
    (tools, choice)
}

/// Map [`ThinkingLevel`] onto `reasoning.effort`. `Off` sends nothing.
const fn reasoning_effort(level: ThinkingLevel) -> Option<&'static str> {
    match level {
        ThinkingLevel::Minimal => Some("minimal"),
        ThinkingLevel::Low => Some("low"),
        ThinkingLevel::Medium => Some("medium"),
        ThinkingLevel::High => Some("high"),
        ThinkingLevel::ExtraHigh => Some("xhigh"),
        // `Off`, and any future variant with no known wire value: omit.
        _ => None,
    }
}

/// Map [`ResponseFormat`] onto `text.format`.
fn text_format(options: &StreamOptions) -> Option<Value> {
    options.serving.format.as_ref().and_then(|f| match f {
        ResponseFormat::Json => Some(serde_json::json!({ "format": { "type": "json_object" } })),
        ResponseFormat::Schema(schema) => Some(serde_json::json!({
            "format": {
                "type": "json_schema",
                "name": "response",
                "strict": true,
                "schema": schema,
            }
        })),
        _ => None,
    })
}

fn build_request(
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> ResponsesRequest {
    const TYPED_KEYS: &[&str] = &[
        "model",
        "instructions",
        "input",
        "stream",
        "store",
        "temperature",
        "max_output_tokens",
        "top_p",
        "reasoning",
        "text",
        "tools",
        "tool_choice",
    ];
    let mut extra = serde_json::Map::new();
    crate::base::merge_extra(&mut extra, &options.serving.extra, TYPED_KEYS);

    let instructions = if context.system_prompt.trim().is_empty() {
        DEFAULT_INSTRUCTIONS.to_owned()
    } else {
        context.system_prompt.clone()
    };
    let input = convert_messages::<ResponsesConverter>(&context.messages, &context.system_prompt)
        .into_iter()
        .flatten()
        .collect();
    let (tools, tool_choice) = build_tools(&context.tools);

    ResponsesRequest {
        model: model.model_id.clone(),
        instructions,
        input,
        stream: true,
        store: false,
        temperature: options.temperature,
        max_output_tokens: options.max_tokens,
        top_p: options.serving.top_p,
        reasoning: reasoning_effort(model.thinking_level)
            .map(|effort| ResponsesReasoning { effort }),
        text: text_format(options),
        tools,
        tool_choice,
        extra,
    }
}

// ─── Streaming event types ──────────────────────────────────────────────────

/// Loose envelope for every Responses SSE payload. The `event:` label decides
/// which fields matter; everything is optional so an unknown event never
/// fails deserialization.
#[derive(Deserialize)]
struct StreamPayload {
    #[serde(default)]
    output_index: Option<usize>,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    item: Option<OutputItem>,
    #[serde(default)]
    response: Option<ResponseObject>,
    // `error` events carry these at the top level …
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    // … and some servers nest them.
    #[serde(default)]
    error: Option<ErrorObject>,
}

#[derive(Deserialize)]
struct OutputItem {
    r#type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ResponseObject {
    #[serde(default)]
    usage: Option<ResponsesUsage>,
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
    #[serde(default)]
    error: Option<ErrorObject>,
}

#[derive(Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct ErrorObject {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: Option<u64>,
    #[serde(default)]
    input_tokens_details: HashMap<String, Value>,
    #[serde(default)]
    output_tokens_details: HashMap<String, Value>,
}

impl ResponsesUsage {
    /// `input_tokens` *includes* cached tokens on this protocol; they are
    /// split out so cache reads are priced at the cached rate and not
    /// double-counted as fresh input. Every numeric detail is also kept in
    /// `extra` under its dotted path (e.g.
    /// `output_tokens_details.reasoning_tokens`).
    fn to_usage(&self) -> Usage {
        let cached = self
            .input_tokens_details
            .get("cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(self.input_tokens);
        let mut extra = HashMap::new();
        for (prefix, details) in [
            ("input_tokens_details", &self.input_tokens_details),
            ("output_tokens_details", &self.output_tokens_details),
        ] {
            for (key, value) in details {
                if let Some(n) = value.as_u64() {
                    extra.insert(format!("{prefix}.{key}"), n);
                }
            }
        }
        let mut usage = Usage::default()
            .with_input(self.input_tokens - cached)
            .with_output(self.output_tokens)
            .with_total(
                self.total_tokens
                    .unwrap_or(self.input_tokens + self.output_tokens),
            )
            .with_extra(extra);
        usage.cache_read = cached;
        usage
    }
}

// ─── Stream state machine ───────────────────────────────────────────────────

struct ToolEntry {
    content_index: usize,
    /// Bytes of arguments already streamed as deltas, so a `…arguments.done`
    /// for a call that never streamed deltas can emit the full payload once.
    streamed: usize,
}

#[derive(Default)]
struct ResponsesState {
    blocks: BlockAccumulator,
    /// Provider `output_index` → harness bookkeeping.
    tools: HashMap<usize, ToolEntry>,
    saw_tool_call: bool,
    usage: Option<Usage>,
    /// Set once a terminal event (`response.completed` & co.) was handled,
    /// so end-of-stream is not reported as a truncation.
    finished: bool,
}

impl crate::finalize::StreamFinalize for ResponsesState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        self.tools.clear();
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

impl ResponsesState {
    fn terminal(&mut self, event: AssistantMessageEvent) -> Vec<AssistantMessageEvent> {
        self.finished = true;
        let mut events = crate::finalize::finalize_blocks(self);
        events.push(event);
        events
    }

    fn done(&mut self, stop_reason: StopReason) -> Vec<AssistantMessageEvent> {
        let usage = self.usage.take().unwrap_or_default();
        self.terminal(AssistantMessageEvent::Done {
            stop_reason,
            usage,
            cost: Cost::default(),
        })
    }

    fn open_tool_call(
        &mut self,
        output_index: usize,
        item: &OutputItem,
        events: &mut Vec<AssistantMessageEvent>,
    ) {
        if self.tools.contains_key(&output_index) {
            return;
        }
        if let Some(ev) = self.blocks.close_thinking(None) {
            events.push(ev);
        }
        if let Some(ev) = self.blocks.close_text() {
            events.push(ev);
        }
        let id = item
            .call_id
            .clone()
            .or_else(|| item.id.clone())
            .unwrap_or_else(|| format!("call_{output_index}"));
        let (content_index, start) = self
            .blocks
            .open_tool_call(id, item.name.clone().unwrap_or_default());
        events.push(start);
        self.saw_tool_call = true;
        self.tools.insert(
            output_index,
            ToolEntry {
                content_index,
                streamed: 0,
            },
        );
    }

    /// Handle one paired SSE event. Returns `Some` when the stream is over.
    #[allow(clippy::too_many_lines)]
    fn on_event(&mut self, event_type: &str, payload: &StreamPayload, provider: &str) -> SseAction {
        let mut events = Vec::new();
        match event_type {
            "response.output_item.added" => {
                if let Some(item) = &payload.item
                    && item.r#type == "function_call"
                {
                    self.open_tool_call(payload.output_index.unwrap_or(0), item, &mut events);
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = payload.delta.as_deref().filter(|d| !d.is_empty()) {
                    if let Some(ev) = self.blocks.close_thinking(None) {
                        events.push(ev);
                    }
                    if let Some(ev) = self.blocks.ensure_text_open() {
                        events.push(ev);
                    }
                    if let Some(ev) = self.blocks.text_delta(delta.to_owned()) {
                        events.push(ev);
                    }
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = payload.delta.as_deref().filter(|d| !d.is_empty()) {
                    if let Some(ev) = self.blocks.ensure_thinking_open() {
                        events.push(ev);
                    }
                    if let Some(ev) = self.blocks.thinking_delta(delta.to_owned()) {
                        events.push(ev);
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if let (Some(index), Some(delta)) = (payload.output_index, payload.delta.as_deref())
                    && let Some(entry) = self.tools.get_mut(&index)
                    && !delta.is_empty()
                {
                    entry.streamed += delta.len();
                    events.push(BlockAccumulator::tool_call_delta(
                        entry.content_index,
                        delta.to_owned(),
                    ));
                }
            }
            "response.function_call_arguments.done" => {
                // Some servers skip deltas and send the whole payload here.
                if let (Some(index), Some(arguments)) =
                    (payload.output_index, payload.arguments.as_deref())
                    && let Some(entry) = self.tools.get_mut(&index)
                    && entry.streamed == 0
                    && !arguments.is_empty()
                {
                    entry.streamed = arguments.len();
                    events.push(BlockAccumulator::tool_call_delta(
                        entry.content_index,
                        arguments.to_owned(),
                    ));
                }
            }
            "response.output_item.done" => {
                if let Some(item) = &payload.item {
                    match item.r#type.as_str() {
                        "function_call" => {
                            let index = payload.output_index.unwrap_or(0);
                            // A call that arrived only in `done` (no `added`).
                            if !self.tools.contains_key(&index) {
                                self.open_tool_call(index, item, &mut events);
                                if let (Some(entry), Some(arguments)) =
                                    (self.tools.get_mut(&index), item.arguments.as_deref())
                                    && !arguments.is_empty()
                                {
                                    entry.streamed = arguments.len();
                                    events.push(BlockAccumulator::tool_call_delta(
                                        entry.content_index,
                                        arguments.to_owned(),
                                    ));
                                }
                            }
                            if let Some(entry) = self.tools.remove(&index)
                                && let Some(ev) = self.blocks.close_tool_call(entry.content_index)
                            {
                                events.push(ev);
                            }
                        }
                        "message" => {
                            if let Some(ev) = self.blocks.close_text() {
                                events.push(ev);
                            }
                        }
                        "reasoning" => {
                            if let Some(ev) = self.blocks.close_thinking(None) {
                                events.push(ev);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "response.completed" => {
                self.usage = payload
                    .response
                    .as_ref()
                    .and_then(|r| r.usage.as_ref())
                    .map(ResponsesUsage::to_usage);
                let stop = if self.saw_tool_call {
                    StopReason::ToolUse
                } else {
                    StopReason::Stop
                };
                return SseAction::Done(self.done(stop));
            }
            "response.incomplete" => {
                let response = payload.response.as_ref();
                self.usage = response
                    .and_then(|r| r.usage.as_ref())
                    .map(ResponsesUsage::to_usage);
                let reason = response
                    .and_then(|r| r.incomplete_details.as_ref())
                    .and_then(|d| d.reason.as_deref())
                    .unwrap_or("unknown");
                return match reason {
                    "max_output_tokens" => SseAction::Done(self.done(StopReason::Length)),
                    "content_filter" => SseAction::Done(self.terminal(
                        AssistantMessageEvent::error_content_filtered(format!(
                            "{provider} response stopped by content filter"
                        )),
                    )),
                    _ => SseAction::Done(self.done(StopReason::Stop)),
                };
            }
            "response.failed" => {
                let detail = payload
                    .response
                    .as_ref()
                    .and_then(|r| r.error.as_ref())
                    .map_or_else(
                        || "response failed".to_owned(),
                        |e| error_detail(e.code.as_deref(), e.message.as_deref()),
                    );
                return SseAction::Done(
                    self.terminal(AssistantMessageEvent::error(format!("{provider} {detail}"))),
                );
            }
            "error" => {
                let (code, message) = match &payload.error {
                    Some(e) => (e.code.as_deref(), e.message.as_deref()),
                    None => (payload.code.as_deref(), payload.message.as_deref()),
                };
                let detail = error_detail(code, message);
                let event = if code == Some("rate_limit_exceeded") {
                    AssistantMessageEvent::error_throttled(format!("{provider} {detail}"))
                } else {
                    AssistantMessageEvent::error(format!("{provider} {detail}"))
                };
                return SseAction::Done(self.terminal(event));
            }
            _ => {}
        }
        if events.is_empty() {
            SseAction::Skip
        } else {
            SseAction::Continue(events)
        }
    }
}

fn error_detail(code: Option<&str>, message: Option<&str>) -> String {
    match (code, message) {
        (Some(code), Some(message)) => format!("error {code}: {message}"),
        (Some(code), None) => format!("error {code}"),
        (None, Some(message)) => format!("error: {message}"),
        (None, None) => "error".to_owned(),
    }
}

/// Parse a Responses SSE stream into `AssistantMessageEvent`s.
fn parse_responses_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    provider: &'static str,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>> {
    let events = sse_paired_events_with_callback(response.bytes_stream(), on_raw_payload);

    sse_adapter_stream(
        events,
        cancellation_token,
        ResponsesState::default(),
        "operation cancelled",
        move |item: Option<SseEvent>, state: &mut ResponsesState| match item {
            None => {
                if state.finished {
                    return SseAction::Done(Vec::new());
                }
                SseAction::Done(state.terminal(AssistantMessageEvent::error_network(format!(
                    "{provider} stream ended unexpectedly"
                ))))
            }
            Some(event) if event.event_type == SSE_TRANSPORT_ERROR_EVENT => {
                SseAction::Done(state.terminal(AssistantMessageEvent::error_network(format!(
                    "{provider} {}",
                    event.data
                ))))
            }
            Some(event) if event.event_type == SSE_PROTOCOL_ERROR_EVENT => {
                SseAction::Done(state.terminal(AssistantMessageEvent::error(format!(
                    "{provider} {}",
                    event.data
                ))))
            }
            Some(event) => {
                let payload: StreamPayload = match serde_json::from_str(&event.data) {
                    Ok(p) => p,
                    Err(e) => {
                        error!(error = %e, event = %event.event_type, "{provider} JSON parse error");
                        return SseAction::Done(state.terminal(AssistantMessageEvent::error(
                            format!("{provider} JSON parse error: {e}"),
                        )));
                    }
                };
                state.on_event(&event.event_type, &payload, provider)
            }
        },
    )
}

// ─── Shell ──────────────────────────────────────────────────────────────────

/// Shared transport for Responses-speaking providers.
/// HTTP 4xx body classifier: returns a structured event or `None` to fall
/// through to status-based classification.
pub(crate) type ErrorClassifier = fn(u16, &str, &str) -> Option<AssistantMessageEvent>;

pub(crate) struct ResponsesAdapterShell {
    provider: &'static str,
    base: AdapterBase,
    responses_path: &'static str,
    classify: ErrorClassifier,
}

impl ResponsesAdapterShell {
    pub(crate) fn new(
        provider: &'static str,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        responses_path: &'static str,
    ) -> Self {
        Self {
            provider,
            base: AdapterBase::new(base_url, api_key),
            responses_path,
            classify: crate::oai_transport::classify_oai_error_body,
        }
    }

    pub(crate) fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.base = self.base.with_header(name, value);
        self
    }

    pub(crate) fn with_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.base = self.base.with_headers(headers);
        self
    }

    #[cfg(test)]
    pub(crate) fn base_url(&self) -> &str {
        &self.base.base_url
    }

    fn url(&self) -> String {
        format!("{}{}", self.base.base_url, self.responses_path)
    }

    fn api_key<'a>(&'a self, options: &'a StreamOptions) -> &'a str {
        options.api_key.as_deref().unwrap_or(&self.base.api_key)
    }

    /// Bearer auth unless the static header map overrides `Authorization`,
    /// then the static headers — same contract as the Chat Completions shell.
    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        options: &StreamOptions,
        per_request: &reqwest::header::HeaderMap,
    ) -> reqwest::RequestBuilder {
        let mut request = request;
        let overrides_auth = self
            .base
            .headers
            .contains_key(reqwest::header::AUTHORIZATION)
            || per_request.contains_key(reqwest::header::AUTHORIZATION);
        if !overrides_auth {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key(options)),
            );
        }
        for (name, value) in self.base.headers.iter().chain(per_request.iter()) {
            request = request.header(name, value);
        }
        request
    }

    pub(crate) fn fmt_debug(
        &self,
        name: &'static str,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.debug_struct(name)
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }

    pub(crate) fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.stream_with_headers(
            model,
            context,
            options,
            cancellation_token,
            &reqwest::header::HeaderMap::new(),
        )
    }

    /// [`stream`](Self::stream) plus headers that exist only for this one
    /// request (a per-request session id, a routing id derived from the
    /// credential). Applied after the static map, so they win.
    ///
    /// Borrows are only needed while the request is built, so the returned
    /// stream is `'static`: a caller can construct `options` locally (e.g.
    /// after resolving a credential) and still hand the stream out.
    pub(crate) fn stream_with_headers(
        &self,
        model: &ModelSpec,
        context: &AgentContext,
        options: &StreamOptions,
        cancellation_token: CancellationToken,
        per_request: &reqwest::header::HeaderMap,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'static>> {
        let url = self.url();
        debug!(
            provider = self.provider,
            %url,
            model = %model.model_id,
            messages = context.messages.len(),
            "sending Responses request"
        );
        let body = build_request(model, context, options);
        let request = self.authorize(
            self.base.client.post(&url).json(&body),
            options,
            per_request,
        );
        let provider = self.provider;
        let classify = self.classify;
        let on_raw_payload = options.on_raw_payload.clone();
        let on_rate_limit = options.on_rate_limit.clone();

        Box::pin(
            stream::once(async move {
                let response = match crate::base::race_pre_stream_cancellation(
                    &cancellation_token,
                    "operation cancelled",
                    async {
                        request.send().await.map_err(|e| {
                            AssistantMessageEvent::error_network(format!(
                                "{provider} connection error: {e}"
                            ))
                        })
                    },
                )
                .await
                {
                    Ok(response) => response,
                    Err(event) => {
                        return stream::iter(crate::base::pre_stream_error(event)).left_stream();
                    }
                };
                crate::base::report_rate_limit(response.headers(), on_rate_limit.as_ref());

                let status = response.status();
                if !status.is_success() {
                    let code = status.as_u16();
                    let body = match crate::base::read_error_body_or_cancelled(
                        response,
                        &cancellation_token,
                        "operation cancelled",
                    )
                    .await
                    {
                        Ok(body) => body,
                        Err(event) => {
                            return stream::iter(crate::base::pre_stream_error(event))
                                .left_stream();
                        }
                    };
                    warn!(status = code, "{provider} HTTP error");
                    let event = classify(code, &body, provider).unwrap_or_else(|| {
                        crate::classify::error_event_from_status(code, &body, provider)
                    });
                    return stream::iter([AssistantMessageEvent::Start, event]).left_stream();
                }

                parse_responses_sse_stream(response, cancellation_token, provider, on_raw_payload)
                    .right_stream()
            })
            .flatten(),
        )
    }
}

// ─── Public adapter ─────────────────────────────────────────────────────────

/// A [`StreamFn`] for any endpoint that speaks the OpenAI Responses API.
///
/// Defaults to `/v1/responses` under `base_url`; override with
/// [`with_responses_path`](Self::with_responses_path) for backends that mount
/// it elsewhere. Static headers via [`with_header`](Self::with_header) reach
/// every request; supplying `Authorization` replaces the default `Bearer`.
pub struct ResponsesStreamFn {
    pub(crate) shell: ResponsesAdapterShell,
}

impl ResponsesStreamFn {
    /// Create an adapter against `base_url` authenticating with `api_key`.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            shell: ResponsesAdapterShell::new("Responses", base_url, api_key, "/v1/responses"),
        }
    }

    /// Change the path appended to `base_url` (default `/v1/responses`).
    #[must_use]
    pub const fn with_responses_path(mut self, path: &'static str) -> Self {
        self.shell.responses_path = path;
        self
    }

    /// Label used in error messages and logs (default `"Responses"`).
    #[must_use]
    pub const fn with_provider_label(mut self, label: &'static str) -> Self {
        self.shell.provider = label;
        self
    }

    /// Replace the HTTP 4xx body classifier. Takes `(status, body,
    /// provider_label)` and returns a structured event, or `None` to fall
    /// through to status-based classification. Defaults to the shared
    /// OpenAI error-envelope classifier.
    #[must_use]
    pub const fn with_error_classifier(
        mut self,
        classify: fn(u16, &str, &str) -> Option<AssistantMessageEvent>,
    ) -> Self {
        self.shell.classify = classify;
        self
    }

    /// Add one static header to every request.
    #[must_use]
    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.shell = self.shell.with_header(name, value);
        self
    }

    /// Merge a header map into every request, replacing colliding names.
    #[must_use]
    pub fn with_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.shell = self.shell.with_headers(headers);
        self
    }
}

impl std::fmt::Debug for ResponsesStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shell.fmt_debug("ResponsesStreamFn", f)
    }
}

impl StreamFn for ResponsesStreamFn {
    // `top_p`, `format` (as `text.format`) and `extra` reach the body;
    // `context_length` / `keep_alive` have no equivalent.
    fn supported_serving_options(&self) -> ServingOptionSupport {
        ServingOptionSupport::none()
            .with_top_p(true)
            .with_format(true)
            .with_extra(true)
    }

    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.shell
            .stream(model, context, options, cancellation_token)
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ResponsesStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{AgentMessage, LlmMessage};

    fn spec(level: ThinkingLevel) -> ModelSpec {
        ModelSpec::new("openai", "gpt-5.6-luna").with_thinking_level(level)
    }

    #[test]
    fn request_always_sends_store_false_and_non_empty_instructions() {
        let context = AgentContext::new("", Vec::new(), Vec::new());
        let body = serde_json::to_value(build_request(
            &spec(ThinkingLevel::Off),
            &context,
            &StreamOptions::default(),
        ))
        .unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], DEFAULT_INSTRUCTIONS);
        assert!(
            body.get("reasoning").is_none(),
            "Off must not send reasoning"
        );

        let context = AgentContext::new("Be terse.", Vec::new(), Vec::new());
        let body = serde_json::to_value(build_request(
            &spec(ThinkingLevel::High),
            &context,
            &StreamOptions::default(),
        ))
        .unwrap();
        assert_eq!(body["instructions"], "Be terse.");
        assert_eq!(body["reasoning"]["effort"], "high");
        // The system prompt is `instructions`, never an input item.
        assert!(body["input"].as_array().unwrap().is_empty());
    }

    #[test]
    fn reasoning_effort_covers_every_level() {
        assert_eq!(reasoning_effort(ThinkingLevel::Minimal), Some("minimal"));
        assert_eq!(reasoning_effort(ThinkingLevel::Low), Some("low"));
        assert_eq!(reasoning_effort(ThinkingLevel::Medium), Some("medium"));
        assert_eq!(reasoning_effort(ThinkingLevel::High), Some("high"));
        assert_eq!(reasoning_effort(ThinkingLevel::ExtraHigh), Some("xhigh"));
        assert_eq!(reasoning_effort(ThinkingLevel::Off), None);
    }

    #[test]
    fn tool_round_trip_converts_to_function_call_and_output_items() {
        let assistant = AssistantMessage::new(
            vec![
                ContentBlock::Text {
                    text: "Checking.".to_owned(),
                },
                ContentBlock::ToolCall {
                    id: "call_1".to_owned(),
                    name: "weather".to_owned(),
                    arguments: serde_json::json!({"city": "Oslo"}),
                    partial_json: None,
                },
            ],
            "openai",
            "gpt-5.6-luna",
        );
        let result = ToolResultMessage::new(
            "call_1",
            vec![ContentBlock::Text {
                text: "12C".to_owned(),
            }],
        );
        let user = UserMessage::new(vec![ContentBlock::Text {
            text: "What next?".to_owned(),
        }]);
        let messages = vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage::new(vec![
                ContentBlock::Text {
                    text: "Weather in Oslo?".to_owned(),
                },
            ]))),
            AgentMessage::Llm(LlmMessage::Assistant(assistant)),
            AgentMessage::Llm(LlmMessage::ToolResult(result)),
            AgentMessage::Llm(LlmMessage::User(user)),
        ];
        let context = AgentContext::new("sys", messages, Vec::new());
        let body = serde_json::to_value(build_request(
            &spec(ThinkingLevel::Off),
            &context,
            &StreamOptions::default(),
        ))
        .unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 5, "{input:#?}");
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[1]["content"][0]["text"], "Checking.");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["name"], "weather");
        assert_eq!(input[2]["arguments"], r#"{"city":"Oslo"}"#);
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[3]["output"], "12C");
        assert_eq!(input[4]["role"], "user");
    }

    #[test]
    fn text_format_maps_json_and_schema() {
        let json = StreamOptions::default()
            .with_serving(swink_agent::ServingOptions::default().with_format(ResponseFormat::Json));
        assert_eq!(text_format(&json).unwrap()["format"]["type"], "json_object");
        let schema = StreamOptions::default().with_serving(
            swink_agent::ServingOptions::default().with_format(ResponseFormat::Schema(
                serde_json::json!({"type": "object"}),
            )),
        );
        let value = text_format(&schema).unwrap();
        assert_eq!(value["format"]["type"], "json_schema");
        assert_eq!(value["format"]["strict"], true);
        assert_eq!(value["format"]["schema"]["type"], "object");
    }

    #[test]
    fn usage_splits_cached_tokens_out_of_input() {
        let usage: ResponsesUsage = serde_json::from_value(serde_json::json!({
            "input_tokens": 1000,
            "output_tokens": 50,
            "total_tokens": 1050,
            "input_tokens_details": {"cached_tokens": 600},
            "output_tokens_details": {"reasoning_tokens": 20}
        }))
        .unwrap();
        let usage = usage.to_usage();
        assert_eq!(usage.input, 400);
        assert_eq!(usage.cache_read, 600);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.total, 1050);
        assert_eq!(usage.extra["output_tokens_details.reasoning_tokens"], 20);
        assert_eq!(usage.extra["input_tokens_details.cached_tokens"], 600);
    }

    #[test]
    fn trailing_slash_stripped() {
        let f = ResponsesStreamFn::new("https://api.openai.com/", "k");
        assert_eq!(f.shell.base_url(), "https://api.openai.com");
        assert_eq!(f.shell.url(), "https://api.openai.com/v1/responses");
        let f = f.with_responses_path("/responses");
        assert_eq!(f.shell.url(), "https://api.openai.com/responses");
    }
}
