//! AWS Bedrock adapter.
//!
//! Uses the Bedrock `ConverseStream` API and maps event-stream frames into
//! the harness event protocol. Responses arrive as binary event-stream frames
//! decoded by `aws-smithy-eventstream`.

use std::collections::HashMap;
use std::{pin::Pin, time::SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::{
    http_request::{
        self, PayloadChecksumKind, SessionTokenMode, SignableBody, SignableRequest, SigningSettings,
    },
    sign::v4,
};
use aws_smithy_eventstream::frame::{DecodedFrame, MessageFrameDecoder};
use aws_smithy_runtime_api::client::identity::Identity;
use aws_smithy_types::event_stream::HeaderValue;
use bytes::BytesMut;
use futures::stream::{self, Stream, StreamExt as _};
use reqwest::{
    Client,
    header::{CONTENT_TYPE, HOST, HeaderName, HeaderValue as HttpHeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use swink_agent::{
    AgentContext, AgentMessage, AssistantMessageEvent, ContentBlock, Cost, LlmMessage, ModelSpec,
    StopReason, StreamFn, StreamOptions, Usage,
};

use crate::block_accumulator::BlockAccumulator;
use crate::convert::extract_tool_schemas;
use crate::finalize::{self, StreamFinalize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockRequest {
    messages: Vec<BedrockMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<BedrockSystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_config: Option<BedrockInferenceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<BedrockToolConfig>,
}

#[derive(Debug, Serialize)]
struct BedrockSystemBlock {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockInferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolConfig {
    tools: Vec<BedrockTool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockTool {
    tool_spec: BedrockToolSpec,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolSpec {
    name: String,
    description: String,
    input_schema: BedrockInputSchema,
}

#[derive(Debug, Serialize)]
struct BedrockInputSchema {
    json: Value,
}

#[derive(Debug, Serialize)]
struct BedrockMessage {
    role: String,
    content: Vec<BedrockContentBlock>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockContentBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use: Option<BedrockToolUse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_result: Option<BedrockToolResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolUse {
    tool_use_id: String,
    name: String,
    input: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolResult {
    tool_use_id: String,
    content: Vec<BedrockToolResultContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct BedrockToolResultContent {
    text: String,
}

// --- Streaming event deserialization types ---

#[derive(Debug, Deserialize)]
struct MessageStartEvent {
    #[allow(dead_code)]
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockStartEvent {
    content_block_index: usize,
    start: StartBlock,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum StartBlock {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "toolUse")]
    ToolUse {
        #[serde(rename = "toolUseId")]
        tool_use_id: String,
        name: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockDeltaEvent {
    content_block_index: usize,
    delta: DeltaBlock,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DeltaBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "toolUse")]
    ToolUse { input: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockStopEvent {
    content_block_index: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageStopEvent {
    stop_reason: String,
}

#[derive(Debug, Deserialize)]
struct MetadataEvent {
    usage: BedrockStreamUsage,
    // Deserialized for completeness but not currently used.
    #[serde(default)]
    #[allow(dead_code)]
    metrics: Option<BedrockMetrics>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct BedrockStreamUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct BedrockMetrics {
    #[serde(default)]
    latency_ms: u64,
}

// --- Streaming state ---

/// The type of content block currently active at a given provider index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    ToolUse,
}

/// State machine tracking Bedrock streaming progress.
///
/// Block lifecycle (index allocation, open/close, drain) is delegated to
/// [`BlockAccumulator`].  The `provider_blocks` map translates Bedrock's
/// provider-side block indices to `(BlockType, harness content_index)` so
/// that `contentBlockDelta` and `contentBlockStop` events can be routed to
/// the correct accumulator method.
#[derive(Debug)]
struct BedrockStreamState {
    /// Shared block lifecycle accumulator.
    blocks: BlockAccumulator,
    /// Bedrock block index → `(BlockType, harness content_index)`.
    provider_blocks: HashMap<usize, (BlockType, usize)>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
}

impl BedrockStreamState {
    fn new() -> Self {
        Self {
            blocks: BlockAccumulator::default(),
            provider_blocks: HashMap::new(),
            stop_reason: None,
            usage: None,
        }
    }
}

impl StreamFinalize for BedrockStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        self.provider_blocks.clear();
        self.blocks.drain_open_blocks()
    }
}

pub struct BedrockStreamFn {
    base_url: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    client: Client,
}

impl BedrockStreamFn {
    #[must_use]
    pub fn new(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        let region = region.into();
        Self {
            base_url: format!("https://bedrock-runtime.{region}.amazonaws.com"),
            region,
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
            client: Client::new(),
        }
    }

    #[must_use]
    pub fn new_with_base_url(
        base_url: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for BedrockStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockStreamFn")
            .field("base_url", &self.base_url)
            .field("region", &self.region)
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

impl StreamFn for BedrockStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.converse_stream(model, context, options, cancellation_token)
    }
}

impl BedrockStreamFn {
    /// Build and sign a `ConverseStream` request, returning the streaming response.
    async fn send_converse_stream(
        &self,
        model: &ModelSpec,
        context: &AgentContext,
        options: &StreamOptions,
    ) -> Result<reqwest::Response, AssistantMessageEvent> {
        let body = build_request(context, options);
        let body_json = serde_json::to_vec(&body).map_err(|e| {
            AssistantMessageEvent::error_network(format!("Bedrock JSON error: {e}"))
        })?;
        let path = format!("/model/{}/converse-stream", model.model_id);
        let url = format!("{}{}", self.base_url, path);
        debug!(%url, model = %model.model_id, "sending Bedrock converse-stream request");

        let request = self.client.post(&url).body(body_json.clone());
        let host = request
            .try_clone()
            .and_then(|builder| builder.build().ok())
            .and_then(|request| request_host_header(request.url()))
            .unwrap_or_else(|| "bedrock-runtime.amazonaws.com".to_string());
        let mut request = request
            .header(CONTENT_TYPE, "application/json")
            .header(HOST, &host)
            .build()
            .map_err(|e| {
                AssistantMessageEvent::error_network(format!("Bedrock request build error: {e}"))
            })?;
        self.sign_request(&mut request, &body_json)?;

        let response = self.client.execute(request).await.map_err(|e| {
            AssistantMessageEvent::error_network(format!("Bedrock connection error: {e}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Bedrock HTTP error");
            return Err(crate::classify::error_event_from_status(
                code, &body, "Bedrock",
            ));
        }

        Ok(response)
    }

    /// Stream events from the Bedrock `ConverseStream` API.
    ///
    /// Reads the response as a binary event-stream, decodes frames with
    /// `MessageFrameDecoder`, and maps each frame through `parse_event_frame`.
    #[allow(clippy::too_many_lines)]
    fn converse_stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(stream::unfold(
            StreamUnfoldState::Init {
                model,
                context,
                options,
                cancellation_token,
            },
            move |unfold_state| async move {
                match unfold_state {
                    StreamUnfoldState::Init {
                        model,
                        context,
                        options,
                        cancellation_token,
                    } => {
                        if cancellation_token.is_cancelled() {
                            return Some((
                                vec![AssistantMessageEvent::error_network(
                                    "Bedrock request cancelled",
                                )],
                                StreamUnfoldState::Done,
                            ));
                        }

                        match self.send_converse_stream(model, context, options).await {
                            Ok(response) => {
                                let byte_stream = response.bytes_stream();
                                Some((
                                    vec![],
                                    StreamUnfoldState::Streaming {
                                        byte_stream: Box::pin(byte_stream),
                                        decoder: MessageFrameDecoder::new(),
                                        buffer: BytesMut::new(),
                                        state: Box::new(BedrockStreamState::new()),
                                        cancellation_token,
                                    },
                                ))
                            }
                            Err(err_event) => {
                                Some((vec![err_event], StreamUnfoldState::Done))
                            }
                        }
                    }
                    StreamUnfoldState::Streaming {
                        mut byte_stream,
                        mut decoder,
                        mut buffer,
                        mut state,
                        cancellation_token,
                    } => {
                        loop {
                            // Try to decode a frame from the buffer first
                            match decoder.decode_frame(&mut buffer) {
                                Ok(DecodedFrame::Complete(message)) => {
                                    let events = process_smithy_message(
                                        &message, &mut state,
                                    );
                                    if !events.is_empty() {
                                        // Check if this was the final Done event
                                        let is_done = events.iter().any(|e| {
                                            matches!(
                                                e,
                                                AssistantMessageEvent::Done { .. }
                                                    | AssistantMessageEvent::Error { .. }
                                            )
                                        });
                                        if is_done {
                                            return Some((
                                                events,
                                                StreamUnfoldState::Done,
                                            ));
                                        }
                                        return Some((
                                            events,
                                            StreamUnfoldState::Streaming {
                                                byte_stream,
                                                decoder,
                                                buffer,
                                                state,
                                                cancellation_token,
                                            },
                                        ));
                                    }
                                    // No events from this frame (e.g. messageStop),
                                    // continue decoding
                                    continue;
                                }
                                Ok(DecodedFrame::Incomplete) => {
                                    // Need more data — fall through to read from stream
                                }
                                Err(e) => {
                                    warn!(error = %e, "Bedrock event-stream decode error");
                                    let mut events = finalize::finalize_blocks(state.as_mut());
                                    events.push(AssistantMessageEvent::error_network(format!(
                                        "Bedrock event-stream decode error: {e}"
                                    )));
                                    return Some((events, StreamUnfoldState::Done));
                                }
                            }

                            // Read more bytes from the network
                            tokio::select! {
                                biased;
                                () = cancellation_token.cancelled() => {
                                    let mut events = finalize::finalize_blocks(state.as_mut());
                                    events.push(AssistantMessageEvent::error_network(
                                        "Bedrock stream cancelled",
                                    ));
                                    return Some((events, StreamUnfoldState::Done));
                                }
                                chunk = byte_stream.next() => {
                                    match chunk {
                                        Some(Ok(bytes)) => {
                                            buffer.extend_from_slice(&bytes);
                                            // Loop back to try decoding
                                        }
                                        Some(Err(e)) => {
                                            let mut events = finalize::finalize_blocks(state.as_mut());
                                            events.push(AssistantMessageEvent::error_network(
                                                format!("Bedrock stream read error: {e}"),
                                            ));
                                            return Some((events, StreamUnfoldState::Done));
                                        }
                                        None => {
                                            // Stream ended — emit Done if we haven't yet
                                            let mut events = finalize::finalize_blocks(state.as_mut());
                                            if !events.is_empty() || state.stop_reason.is_some() {
                                                let usage = state.usage.take().unwrap_or_default();
                                                let stop_reason = map_stop_reason(
                                                    state.stop_reason.as_deref(),
                                                );
                                                match stop_reason {
                                                    Ok(sr) => events.push(
                                                        AssistantMessageEvent::Done {
                                                            stop_reason: sr,
                                                            usage,
                                                            cost: Cost::default(),
                                                        },
                                                    ),
                                                    Err(err) => events.push(err),
                                                }
                                            }
                                            if events.is_empty() {
                                                // Stream ended without any terminal event
                                                events.push(AssistantMessageEvent::error_network(
                                                    "Bedrock stream ended unexpectedly",
                                                ));
                                            }
                                            return Some((events, StreamUnfoldState::Done));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    StreamUnfoldState::Done => None,
                }
            },
        )
        .flat_map(stream::iter))
    }
}

/// Internal state machine for the `stream::unfold` streaming loop.
enum StreamUnfoldState<'a> {
    Init {
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    },
    Streaming {
        byte_stream: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'a>>,
        decoder: MessageFrameDecoder,
        buffer: BytesMut,
        state: Box<BedrockStreamState>,
        cancellation_token: CancellationToken,
    },
    Done,
}

/// Extract event-type and message-type headers from a smithy `Message` and
/// dispatch to `parse_event_frame`.
fn process_smithy_message(
    message: &aws_smithy_types::event_stream::Message,
    state: &mut BedrockStreamState,
) -> Vec<AssistantMessageEvent> {
    let mut event_type = None;
    let mut message_type = None;
    let mut exception_type = None;

    for header in message.headers() {
        let name = header.name().as_str();
        match name {
            ":event-type" => {
                if let HeaderValue::String(val) = header.value() {
                    event_type = Some(val.as_str().to_string());
                }
            }
            ":message-type" => {
                if let HeaderValue::String(val) = header.value() {
                    message_type = Some(val.as_str().to_string());
                }
            }
            ":exception-type" => {
                if let HeaderValue::String(val) = header.value() {
                    exception_type = Some(val.as_str().to_string());
                }
            }
            _ => {}
        }
    }

    // Handle exception frames
    if message_type.as_deref() == Some("exception") {
        let exc_type = exception_type.as_deref().unwrap_or("unknown");
        let payload_str = std::str::from_utf8(message.payload()).unwrap_or("(binary)");
        warn!(exception_type = exc_type, "Bedrock exception frame");
        return vec![classify_bedrock_exception(exc_type, payload_str)];
    }

    // Handle normal event frames
    event_type.map_or_else(Vec::new, |et| {
        parse_event_frame(&et, message.payload(), state).unwrap_or_else(|| {
            // parse_event_frame returns None for unknown events (benign) or
            // JSON deserialization failures (potentially problematic).
            if matches!(
                et.as_str(),
                "messageStart"
                    | "contentBlockStart"
                    | "contentBlockDelta"
                    | "contentBlockStop"
                    | "metadata"
            ) {
                warn!(
                    event_type = %et,
                    "Bedrock event deserialization failed for known event type"
                );
            }
            vec![]
        })
    })
}

/// Classify a Bedrock exception frame into the correct error category.
///
/// Bedrock exception types map to three buckets:
/// - **Throttled** (retryable): `throttlingException`, `tooManyRequestsException`
/// - **Auth** (non-retryable): `accessDeniedException`, `validationException`,
///   `resourceNotFoundException`
/// - **Network** (retryable): `internalServerException`, `modelStreamErrorException`,
///   `modelTimeoutException`, `serviceUnavailableException`
///
/// Unknown exception types fall through to a generic (unclassified) error so they
/// are not silently treated as retryable.
fn classify_bedrock_exception(exc_type: &str, payload: &str) -> AssistantMessageEvent {
    let lower = exc_type.to_lowercase();

    if lower.contains("throttl") || lower.contains("toomanyrequests") {
        AssistantMessageEvent::error_throttled(format!("Bedrock throttled: {payload}"))
    } else if lower.contains("accessdenied")
        || lower.contains("validation")
        || lower.contains("resourcenotfound")
    {
        AssistantMessageEvent::error_auth(format!(
            "Bedrock client error ({exc_type}): {payload}"
        ))
    } else if lower.contains("internalserver")
        || lower.contains("modelstreamerror")
        || lower.contains("modeltimeout")
        || lower.contains("serviceunavailable")
    {
        AssistantMessageEvent::error_network(format!(
            "Bedrock server error ({exc_type}): {payload}"
        ))
    } else {
        // Unknown exception type — do not assume retryable.
        AssistantMessageEvent::error(format!(
            "Bedrock exception ({exc_type}): {payload}"
        ))
    }
}

fn parse_event_frame(
    event_type: &str,
    payload: &[u8],
    state: &mut BedrockStreamState,
) -> Option<Vec<AssistantMessageEvent>> {
    match event_type {
        "messageStart" => {
            let _event: MessageStartEvent = serde_json::from_slice(payload).ok()?;
            Some(vec![AssistantMessageEvent::Start])
        }
        "contentBlockStart" => {
            let event: ContentBlockStartEvent = serde_json::from_slice(payload).ok()?;
            let provider_idx = event.content_block_index;
            match event.start {
                StartBlock::Text => {
                    let mut events = Vec::new();
                    events.extend(state.blocks.ensure_text_open());
                    if let Some(content_index) = state.blocks.text_index() {
                        state
                            .provider_blocks
                            .insert(provider_idx, (BlockType::Text, content_index));
                    }
                    Some(events)
                }
                StartBlock::ToolUse { tool_use_id, name } => {
                    let (content_index, start_ev) =
                        state.blocks.open_tool_call(tool_use_id, name);
                    state
                        .provider_blocks
                        .insert(provider_idx, (BlockType::ToolUse, content_index));
                    Some(vec![start_ev])
                }
            }
        }
        "contentBlockDelta" => {
            let event: ContentBlockDeltaEvent = serde_json::from_slice(payload).ok()?;
            let (_, content_index) = state.provider_blocks.get(&event.content_block_index)?;
            let content_index = *content_index;
            match event.delta {
                DeltaBlock::Text { text } => state.blocks.text_delta(text).map(|e| vec![e]),
                DeltaBlock::ToolUse { input } => {
                    Some(vec![BlockAccumulator::tool_call_delta(
                        content_index,
                        input,
                    )])
                }
            }
        }
        "contentBlockStop" => {
            let event: ContentBlockStopEvent = serde_json::from_slice(payload).ok()?;
            let (block_type, content_index) =
                state.provider_blocks.remove(&event.content_block_index)?;
            let evt = match block_type {
                BlockType::Text => state.blocks.close_text(),
                BlockType::ToolUse => state.blocks.close_tool_call(content_index),
            };
            evt.map(|e| vec![e])
        }
        "messageStop" => {
            let event: MessageStopEvent = serde_json::from_slice(payload).ok()?;
            state.stop_reason = Some(event.stop_reason);
            None
        }
        "metadata" => {
            let event: MetadataEvent = serde_json::from_slice(payload).ok()?;
            let usage = Usage {
                input: event.usage.input_tokens,
                output: event.usage.output_tokens,
                total: if event.usage.total_tokens == 0 {
                    event.usage.input_tokens + event.usage.output_tokens
                } else {
                    event.usage.total_tokens
                },
                ..Usage::default()
            };
            let stop_reason = map_stop_reason(state.stop_reason.as_deref());
            match stop_reason {
                Ok(stop_reason) => Some(vec![AssistantMessageEvent::Done {
                    stop_reason,
                    usage,
                    cost: Cost::default(),
                }]),
                Err(error_event) => Some(vec![error_event]),
            }
        }
        _ => {
            debug!(event_type, "unknown Bedrock event type, skipping");
            None
        }
    }
}

#[allow(clippy::result_large_err)]
fn map_stop_reason(reason: Option<&str>) -> Result<StopReason, AssistantMessageEvent> {
    match reason {
        Some("tool_use") => Ok(StopReason::ToolUse),
        Some("max_tokens") => Ok(StopReason::Length),
        Some("guardrail_intervened") => Err(AssistantMessageEvent::error_content_filtered(
            "Bedrock content filter: guardrail intervened",
        )),
        // end_turn, stop_sequence, None, and any unknown reason all map to Stop
        _ => Ok(StopReason::Stop),
    }
}

fn build_request(context: &AgentContext, options: &StreamOptions) -> BedrockRequest {
    let mut messages = convert_messages(&context.messages);
    let inference_config = Some(BedrockInferenceConfig {
        temperature: options.temperature,
        max_tokens: options.max_tokens,
    });
    let tools = extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|tool| BedrockTool {
            tool_spec: BedrockToolSpec {
                name: tool.name,
                description: tool.description,
                input_schema: BedrockInputSchema {
                    json: tool.parameters,
                },
            },
        })
        .collect::<Vec<_>>();
    let tool_config = (!tools.is_empty()).then_some(BedrockToolConfig { tools });

    let system = if context.system_prompt.is_empty() {
        None
    } else {
        Some(vec![BedrockSystemBlock {
            text: context.system_prompt.clone(),
        }])
    };

    if messages.is_empty() {
        messages.push(BedrockMessage {
            role: "user".to_string(),
            content: vec![BedrockContentBlock {
                text: Some(String::new()),
                ..BedrockContentBlock::default()
            }],
        });
    }

    BedrockRequest {
        messages,
        system,
        inference_config,
        tool_config,
    }
}

fn convert_messages(messages: &[AgentMessage]) -> Vec<BedrockMessage> {
    let mut result = Vec::new();
    for message in messages {
        let AgentMessage::Llm(llm) = message else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                let text = ContentBlock::extract_text(&user.content);
                if !text.is_empty() {
                    result.push(BedrockMessage {
                        role: "user".to_string(),
                        content: vec![BedrockContentBlock {
                            text: Some(text),
                            ..BedrockContentBlock::default()
                        }],
                    });
                }
            }
            LlmMessage::Assistant(assistant) => {
                let mut content = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            content.push(BedrockContentBlock {
                                text: Some(text.clone()),
                                ..BedrockContentBlock::default()
                            });
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => content.push(BedrockContentBlock {
                            tool_use: Some(BedrockToolUse {
                                tool_use_id: id.clone(),
                                name: name.clone(),
                                input: arguments.clone(),
                            }),
                            ..BedrockContentBlock::default()
                        }),
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    result.push(BedrockMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            LlmMessage::ToolResult(tool_result) => {
                result.push(BedrockMessage {
                    role: "user".to_string(),
                    content: vec![BedrockContentBlock {
                        tool_result: Some(BedrockToolResult {
                            tool_use_id: tool_result.tool_call_id.clone(),
                            content: vec![BedrockToolResultContent {
                                text: ContentBlock::extract_text(&tool_result.content),
                            }],
                            status: tool_result.is_error.then_some("error".to_string()),
                        }),
                        ..BedrockContentBlock::default()
                    }],
                });
            }
        }
    }
    result
}

impl BedrockStreamFn {
    #[allow(clippy::result_large_err)]
    fn sign_request(
        &self,
        request: &mut reqwest::Request,
        body: &[u8],
    ) -> Result<(), AssistantMessageEvent> {
        let credentials = Credentials::new(
            self.access_key_id.clone(),
            self.secret_access_key.clone(),
            self.session_token.clone(),
            None,
            "swink-agent-bedrock",
        );
        let identity: Identity = credentials.into();
        let mut signing_settings = SigningSettings::default();
        signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        signing_settings.session_token_mode = SessionTokenMode::Include;
        let signing_params = http_request::SigningParams::V4(
            v4::SigningParams::builder()
                .identity(&identity)
                .region(&self.region)
                .name("bedrock")
                .time(SystemTime::now())
                .settings(signing_settings)
                .build()
                .map_err(|e| {
                    AssistantMessageEvent::error_network(format!(
                        "Bedrock signing parameter error: {e}"
                    ))
                })?,
        );
        let header_pairs = request
            .headers()
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.as_str(), value))
                    .map_err(|e| {
                        AssistantMessageEvent::error_network(format!(
                            "Bedrock header encoding error: {e}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signable_request = SignableRequest::new(
            request.method().as_str(),
            request.url().as_str(),
            header_pairs.into_iter(),
            SignableBody::Bytes(body),
        )
        .map_err(|e| {
            AssistantMessageEvent::error_network(format!("Bedrock signing request error: {e}"))
        })?;
        let (signing_instructions, _) = http_request::sign(signable_request, &signing_params)
            .map_err(|e| {
                AssistantMessageEvent::error_network(format!("Bedrock signing error: {e}"))
            })?
            .into_parts();
        let (signed_headers, _) = signing_instructions.into_parts();
        for header in signed_headers {
            let name = HeaderName::from_bytes(header.name().as_bytes()).map_err(|e| {
                AssistantMessageEvent::error_network(format!(
                    "Bedrock signed header name error: {e}"
                ))
            })?;
            let value = HttpHeaderValue::from_str(header.value()).map_err(|e| {
                AssistantMessageEvent::error_network(format!(
                    "Bedrock signed header value error: {e}"
                ))
            })?;
            request.headers_mut().insert(name, value);
        }
        Ok(())
    }
}

fn request_host_header(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    match (url.scheme(), url.port()) {
        ("https", Some(443)) | ("http", Some(80)) | (_, None) => Some(host.to_string()),
        (_, Some(port)) => Some(format!("{host}:{port}")),
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BedrockStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{CONTENT_TYPE, HOST};

    #[test]
    fn sigv4_signs_request_with_expected_headers() {
        let signer = BedrockStreamFn::new("us-east-1", "AKIDEXAMPLE", "secret", None);
        let body = b"{}";
        let mut request = signer
            .client
            .post(
                "https://bedrock-runtime.us-east-1.amazonaws.com/model/test-model/converse-stream",
            )
            .body(body.to_vec())
            .build()
            .unwrap();
        request.headers_mut().insert(
            CONTENT_TYPE,
            HttpHeaderValue::from_static("application/json"),
        );
        request.headers_mut().insert(
            HOST,
            HttpHeaderValue::from_static("bedrock-runtime.us-east-1.amazonaws.com"),
        );

        signer.sign_request(&mut request, body).unwrap();

        assert!(request.headers().contains_key("authorization"));
        assert!(request.headers().contains_key("x-amz-date"));
        assert!(request.headers().contains_key("x-amz-content-sha256"));
        let authorization = request.headers()["authorization"].to_str().unwrap();
        assert!(authorization.contains("AWS4-HMAC-SHA256"));
        assert!(authorization.contains("Credential=AKIDEXAMPLE/"));
        assert!(authorization.contains("/us-east-1/bedrock/aws4_request"));
    }

    #[test]
    fn sigv4_signing_includes_session_token() {
        let signer = BedrockStreamFn::new(
            "us-east-1",
            "AKIDEXAMPLE",
            "secret",
            Some("session-token".to_string()),
        );
        let body = b"{}";
        let mut request = signer
            .client
            .post(
                "https://bedrock-runtime.us-east-1.amazonaws.com/model/test-model/converse-stream",
            )
            .body(body.to_vec())
            .build()
            .unwrap();
        request.headers_mut().insert(
            CONTENT_TYPE,
            HttpHeaderValue::from_static("application/json"),
        );
        request.headers_mut().insert(
            HOST,
            HttpHeaderValue::from_static("bedrock-runtime.us-east-1.amazonaws.com"),
        );

        signer.sign_request(&mut request, body).unwrap();

        assert_eq!(request.headers()["x-amz-security-token"], "session-token");
        let authorization = request.headers()["authorization"].to_str().unwrap();
        assert!(authorization.contains("x-amz-security-token"));
    }

    #[test]
    fn parse_message_start_event() {
        let mut state = BedrockStreamState::new();
        let payload = br#"{"role":"assistant"}"#;
        let events = parse_event_frame("messageStart", payload, &mut state);
        assert!(matches!(
            events.as_deref(),
            Some([AssistantMessageEvent::Start])
        ));
    }

    #[test]
    fn parse_text_content_block_events() {
        let mut state = BedrockStreamState::new();

        // contentBlockStart with text
        let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
        let events = parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
        assert!(state.blocks.text_open());

        // contentBlockDelta with text
        let payload = br#"{"contentBlockIndex":0,"delta":{"type":"text","text":"Hello"}}"#;
        let events = parse_event_frame("contentBlockDelta", payload, &mut state).unwrap();
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello"
        ));

        // contentBlockStop
        let payload = br#"{"contentBlockIndex":0}"#;
        let events = parse_event_frame("contentBlockStop", payload, &mut state).unwrap();
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        assert!(!state.blocks.text_open());
    }

    #[test]
    fn parse_tool_use_content_block_events() {
        let mut state = BedrockStreamState::new();

        // contentBlockStart with toolUse — provider index 1, but harness index
        // is 0 because no prior blocks were opened through the accumulator.
        let payload = br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_123","name":"get_weather"}}"#;
        let events = parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallStart { content_index: 0, id, name }
                if id == "tc_123" && name == "get_weather"
        ));
        assert!(state.provider_blocks.contains_key(&1));

        // contentBlockDelta with toolUse input
        let payload =
            br#"{"contentBlockIndex":1,"delta":{"type":"toolUse","input":"{\"city\":\"SF\"}"}}"#;
        let events = parse_event_frame("contentBlockDelta", payload, &mut state).unwrap();
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
                if delta == r#"{"city":"SF"}"#
        ));

        // contentBlockStop
        let payload = br#"{"contentBlockIndex":1}"#;
        let events = parse_event_frame("contentBlockStop", payload, &mut state).unwrap();
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolCallEnd { content_index: 0 }
        ));
    }

    #[test]
    fn parse_message_stop_and_metadata() {
        let mut state = BedrockStreamState::new();

        // messageStop captures stop_reason
        let payload = br#"{"stopReason":"end_turn"}"#;
        let events = parse_event_frame("messageStop", payload, &mut state);
        assert!(events.is_none());
        assert_eq!(state.stop_reason.as_deref(), Some("end_turn"));

        // metadata emits Done
        let payload = br#"{"usage":{"inputTokens":10,"outputTokens":20,"totalTokens":30},"metrics":{"latencyMs":150}}"#;
        let events = parse_event_frame("metadata", payload, &mut state).unwrap();
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::Done { stop_reason: StopReason::Stop, usage, .. }
                if usage.input == 10 && usage.output == 20 && usage.total == 30
        ));
    }

    #[test]
    fn map_stop_reason_variants() {
        assert_eq!(map_stop_reason(Some("end_turn")).unwrap(), StopReason::Stop);
        assert_eq!(
            map_stop_reason(Some("stop_sequence")).unwrap(),
            StopReason::Stop
        );
        assert_eq!(
            map_stop_reason(Some("tool_use")).unwrap(),
            StopReason::ToolUse
        );
        assert_eq!(
            map_stop_reason(Some("max_tokens")).unwrap(),
            StopReason::Length
        );
        assert_eq!(map_stop_reason(None).unwrap(), StopReason::Stop);
        assert!(map_stop_reason(Some("guardrail_intervened")).is_err());
    }

    #[test]
    fn build_request_uses_system_field() {
        let context = AgentContext {
            system_prompt: "You are a helpful assistant.".to_string(),
            messages: vec![AgentMessage::Llm(LlmMessage::User(
                swink_agent::UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "Hello".to_string(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                },
            ))],
            tools: vec![],
        };
        let options = StreamOptions::default();
        let request = build_request(&context, &options);
        assert!(request.system.is_some());
        assert_eq!(
            request.system.unwrap()[0].text,
            "You are a helpful assistant."
        );
        // Should NOT have system prompt as first user message
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
    }

    #[test]
    fn parse_unknown_event_returns_none() {
        let mut state = BedrockStreamState::new();
        let events = parse_event_frame("someUnknownEvent", b"{}", &mut state);
        assert!(events.is_none());
    }

    #[test]
    fn guardrail_intervened_emits_error() {
        let mut state = BedrockStreamState::new();
        state.stop_reason = Some("guardrail_intervened".to_string());

        let payload = br#"{"usage":{"inputTokens":5,"outputTokens":0,"totalTokens":5}}"#;
        let events = parse_event_frame("metadata", payload, &mut state).unwrap();
        assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
    }

    /// Build a smithy event-stream `Message` with the given event-type header
    /// and JSON payload. Used to simulate Bedrock ConverseStream frames.
    fn make_event_message(
        event_type: &str,
        payload: &[u8],
    ) -> aws_smithy_types::event_stream::Message {
        use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
        Message::new_from_parts(
            vec![
                Header::new(
                    ":message-type",
                    HeaderValue::String(String::from("event").into()),
                ),
                Header::new(
                    ":event-type",
                    HeaderValue::String(String::from(event_type).into()),
                ),
            ],
            bytes::Bytes::from(payload.to_vec()),
        )
    }

    /// Build a smithy exception `Message`.
    fn make_exception_message(
        exception_type: &str,
        payload: &[u8],
    ) -> aws_smithy_types::event_stream::Message {
        use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
        Message::new_from_parts(
            vec![
                Header::new(
                    ":message-type",
                    HeaderValue::String(String::from("exception").into()),
                ),
                Header::new(
                    ":exception-type",
                    HeaderValue::String(String::from(exception_type).into()),
                ),
            ],
            bytes::Bytes::from(payload.to_vec()),
        )
    }

    #[test]
    fn text_event_stream_parsing() {
        let mut state = BedrockStreamState::new();

        // 1. messageStart
        let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AssistantMessageEvent::Start));

        // 2. contentBlockStart (text)
        let msg = make_event_message(
            "contentBlockStart",
            br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));

        // 3. contentBlockDelta (text)
        let msg = make_event_message(
            "contentBlockDelta",
            br#"{"contentBlockIndex":0,"delta":{"type":"text","text":"Hello, world!"}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::TextDelta { content_index: 0, delta }
                if delta == "Hello, world!"
        ));

        // 4. contentBlockStop
        let msg = make_event_message("contentBlockStop", br#"{"contentBlockIndex":0}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));

        // 5. messageStop (no events emitted, captures stop_reason)
        let msg = make_event_message("messageStop", br#"{"stopReason":"end_turn"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert!(events.is_empty());
        assert_eq!(state.stop_reason.as_deref(), Some("end_turn"));

        // 6. metadata → Done
        let msg = make_event_message(
            "metadata",
            br#"{"usage":{"inputTokens":15,"outputTokens":25,"totalTokens":40}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage,
                ..
            } if usage.input == 15 && usage.output == 25 && usage.total == 40
        ));
    }

    #[test]
    fn exception_frame_throttling_is_throttled() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message("throttlingException", br#"{"message":"Rate exceeded"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Throttled));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_too_many_requests_is_throttled() {
        let mut state = BedrockStreamState::new();
        let msg =
            make_exception_message("tooManyRequestsException", br#"{"message":"Slow down"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Throttled));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_access_denied_is_auth() {
        let mut state = BedrockStreamState::new();
        let msg =
            make_exception_message("accessDeniedException", br#"{"message":"Not authorized"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_validation_is_auth() {
        let mut state = BedrockStreamState::new();
        let msg =
            make_exception_message("validationException", br#"{"message":"Invalid request"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_resource_not_found_is_auth() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message(
            "resourceNotFoundException",
            br#"{"message":"Model not found"}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_internal_server_is_network() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message(
            "internalServerException",
            br#"{"message":"Internal error"}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_model_stream_error_is_network() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message(
            "modelStreamErrorException",
            br#"{"message":"Stream failed"}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_model_timeout_is_network() {
        let mut state = BedrockStreamState::new();
        let msg =
            make_exception_message("modelTimeoutException", br#"{"message":"Timeout"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_service_unavailable_is_network() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message(
            "serviceUnavailableException",
            br#"{"message":"Service down"}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn exception_frame_unknown_type_is_unclassified() {
        let mut state = BedrockStreamState::new();
        let msg = make_exception_message(
            "someFutureException",
            br#"{"message":"Something new"}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, None, "unknown exceptions should not be classified as retryable");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn guardrail_intervened_maps_to_content_filtered() {
        let mut state = BedrockStreamState::new();

        // messageStop with guardrail_intervened
        let msg = make_event_message("messageStop", br#"{"stopReason":"guardrail_intervened"}"#);
        let _ = process_smithy_message(&msg, &mut state);
        assert_eq!(state.stop_reason.as_deref(), Some("guardrail_intervened"));

        // metadata should emit error, not Done
        let msg = make_event_message(
            "metadata",
            br#"{"usage":{"inputTokens":5,"outputTokens":0,"totalTokens":5}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
    }

    #[test]
    fn stream_finalize_closes_open_text_block() {
        use crate::finalize::OpenBlock;
        let mut state = BedrockStreamState::new();
        // Simulate opening a text block through the accumulator
        let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
        parse_event_frame("contentBlockStart", payload, &mut state);
        let blocks = state.drain_open_blocks();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], OpenBlock::Text { content_index: 0 }));
    }

    #[test]
    fn stream_finalize_closes_open_tool_block() {
        use crate::finalize::OpenBlock;
        let mut state = BedrockStreamState::new();
        // Simulate opening a tool-call block through the accumulator
        let payload = br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_1","name":"tool"}}"#;
        parse_event_frame("contentBlockStart", payload, &mut state);
        let blocks = state.drain_open_blocks();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(
            blocks[0],
            OpenBlock::ToolCall { content_index: 0 }
        ));
    }

    #[test]
    fn stream_finalize_empty_when_no_open_blocks() {
        let mut state = BedrockStreamState::new();
        let blocks = state.drain_open_blocks();
        assert!(blocks.is_empty());
    }

    #[test]
    fn stream_finalize_drains_multiple_open_blocks() {
        use crate::finalize::OpenBlock;
        let mut state = BedrockStreamState::new();
        // Open text block (provider index 0)
        let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
        parse_event_frame("contentBlockStart", payload, &mut state);
        // Close text block
        let payload = br#"{"contentBlockIndex":0}"#;
        parse_event_frame("contentBlockStop", payload, &mut state);
        // Open two tool-call blocks (provider indices 1 and 2)
        let payload = br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_1","name":"a"}}"#;
        parse_event_frame("contentBlockStart", payload, &mut state);
        let payload = br#"{"contentBlockIndex":2,"start":{"type":"toolUse","toolUseId":"tc_2","name":"b"}}"#;
        parse_event_frame("contentBlockStart", payload, &mut state);
        // Drain without closing — both tool calls should be drained
        let blocks = state.drain_open_blocks();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            blocks[0],
            OpenBlock::ToolCall { content_index: 1 }
        ));
        assert!(matches!(
            blocks[1],
            OpenBlock::ToolCall { content_index: 2 }
        ));
    }

    #[test]
    fn content_indices_are_sequential_across_block_types() {
        let mut state = BedrockStreamState::new();
        // Open text (harness index 0)
        let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
        let events = parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
        // Close text
        let payload = br#"{"contentBlockIndex":0}"#;
        parse_event_frame("contentBlockStop", payload, &mut state);
        // Open tool (harness index 1)
        let payload = br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_1","name":"t"}}"#;
        let events = parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                ..
            }
        ));
    }

    #[test]
    fn tool_call_event_stream_parsing() {
        let mut state = BedrockStreamState::new();

        // 1. messageStart
        let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AssistantMessageEvent::Start));

        // 2. contentBlockStart (toolUse)
        let msg = make_event_message(
            "contentBlockStart",
            br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_abc123","name":"get_weather"}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallStart { content_index: 0, id, name }
                if id == "tc_abc123" && name == "get_weather"
        ));

        // 3. contentBlockDelta (toolUse) — partial JSON
        let msg = make_event_message(
            "contentBlockDelta",
            br#"{"contentBlockIndex":0,"delta":{"type":"toolUse","input":"{\"city\""}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
                if delta == r#"{"city""#
        ));

        // 4. contentBlockDelta (toolUse) — rest of JSON
        let msg = make_event_message(
            "contentBlockDelta",
            br#"{"contentBlockIndex":0,"delta":{"type":"toolUse","input":": \"Paris\"}"}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
                if delta == r#": "Paris"}"#
        ));

        // 5. contentBlockStop
        let msg = make_event_message("contentBlockStop", br#"{"contentBlockIndex":0}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolCallEnd { content_index: 0 }
        ));

        // 6. messageStop (tool_use stop reason)
        let msg = make_event_message("messageStop", br#"{"stopReason":"tool_use"}"#);
        let events = process_smithy_message(&msg, &mut state);
        assert!(events.is_empty());
        assert_eq!(state.stop_reason.as_deref(), Some("tool_use"));

        // 7. metadata → Done with ToolUse stop reason
        let msg = make_event_message(
            "metadata",
            br#"{"usage":{"inputTokens":50,"outputTokens":30,"totalTokens":80}}"#,
        );
        let events = process_smithy_message(&msg, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage,
                ..
            } if usage.input == 50 && usage.output == 30 && usage.total == 80
        ));
    }
}
