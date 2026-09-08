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
    ServingOptionSupport, StopReason, StreamFn, StreamOptions, Usage,
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
    /// Model-native parameters the Converse API passes to the model verbatim
    /// (`swink_agent::ServingOptions::extra`, e.g. `top_k` for Anthropic
    /// models). `None` when `extra` is empty so default bodies are unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_model_request_fields: Option<serde_json::Map<String, Value>>,
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
    /// Whether `AssistantMessageEvent::Start` has been emitted.
    started: bool,
}

impl BedrockStreamState {
    fn new() -> Self {
        Self {
            blocks: BlockAccumulator::default(),
            provider_blocks: HashMap::new(),
            stop_reason: None,
            started: false,
        }
    }
}

impl StreamFinalize for BedrockStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        self.provider_blocks.clear();
        self.blocks.drain_open_blocks()
    }
}

fn unexpected_eof_events(state: &mut BedrockStreamState) -> Vec<AssistantMessageEvent> {
    let mut events = finalize::finalize_blocks(state);
    events.push(AssistantMessageEvent::error_network(
        "Bedrock stream ended unexpectedly",
    ));
    prefix_pre_start_terminal_error(events, &mut state.started)
}

fn prefix_pre_start_terminal_error(
    events: Vec<AssistantMessageEvent>,
    started: &mut bool,
) -> Vec<AssistantMessageEvent> {
    if *started || !matches!(events.as_slice(), [AssistantMessageEvent::Error { .. }]) {
        return events;
    }

    let event = events
        .into_iter()
        .next()
        .expect("matched single terminal error");
    crate::base::prefix_start_if_unstarted(event, started)
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
            client: crate::base::adapter_http_client(),
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
            client: crate::base::adapter_http_client(),
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
    // Only `extra` is merged into the request body; the typed serving
    // fields have no ConverseStream equivalent.
    fn supported_serving_options(&self) -> ServingOptionSupport {
        ServingOptionSupport::none().with_extra(true)
    }

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
        cancellation_token: &CancellationToken,
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
        crate::base::report_rate_limit(response.headers(), options.on_rate_limit.as_ref());

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = crate::base::read_error_body_or_cancelled(
                response,
                cancellation_token,
                "Bedrock request cancelled",
            )
            .await?;
            warn!(status = code, "Bedrock HTTP error");
            // Bedrock reports context overflow as HTTP 400 ValidationException
            // with a documented message (e.g. "Input is too long for requested
            // model.") — classify it structurally before status-based mapping.
            if code == 400 && crate::classify::is_context_overflow_message(&body) {
                return Err(AssistantMessageEvent::error_context_overflow(format!(
                    "Bedrock context window exceeded (HTTP {code}): {body}"
                )));
            }
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
                                vec![
                                    AssistantMessageEvent::Start,
                                    crate::base::cancelled_error("Bedrock request cancelled"),
                                ],
                                StreamUnfoldState::Done,
                            ));
                        }

                        match crate::base::race_pre_stream_cancellation(
                            &cancellation_token,
                            "Bedrock request cancelled",
                            self.send_converse_stream(
                                model,
                                context,
                                options,
                                &cancellation_token,
                            ),
                        )
                        .await
                        {
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
                                Some((vec![AssistantMessageEvent::Start, err_event], StreamUnfoldState::Done))
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
                                    let events =
                                        prefix_pre_start_terminal_error(events, &mut state.started);
                                    return Some((events, StreamUnfoldState::Done));
                                }
                            }

                            // Read more bytes from the network
                            tokio::select! {
                                biased;
                                () = cancellation_token.cancelled() => {
                                    let mut events = finalize::finalize_blocks(state.as_mut());
                                    events.push(crate::base::cancelled_error(
                                        "Bedrock stream cancelled",
                                    ));
                                    let events =
                                        prefix_pre_start_terminal_error(events, &mut state.started);
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
                                            let events = prefix_pre_start_terminal_error(
                                                events,
                                                &mut state.started,
                                            );
                                            return Some((events, StreamUnfoldState::Done));
                                        }
                                        None => {
                                            let events = unexpected_eof_events(state.as_mut());
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
        let error_event = classify_bedrock_exception(exc_type, payload_str);
        let mut events = finalize::finalize_blocks(state);
        events.push(error_event);
        if state.started {
            return events;
        }
        return prefix_pre_start_terminal_error(events, &mut state.started);
    }

    // Handle normal event frames
    event_type.map_or_else(Vec::new, |et| match parse_event_frame(&et, message.payload(), state) {
        Ok(Some(events)) => events,
        Ok(None) => Vec::new(),
        Err(error_text) => {
            warn!(event_type = %et, "Bedrock event deserialization failed for known event type");
            let mut events = finalize::finalize_blocks(state);
            events.push(AssistantMessageEvent::error(error_text));
            prefix_pre_start_terminal_error(events, &mut state.started)
        }
    })
}

/// Classify a Bedrock exception frame into the correct error category.
///
/// Bedrock exception types map to four buckets:
/// - **`ContextWindowExceeded`** (recoverable via compaction):
///   `validationException` whose message matches a documented
///   context-overflow wording (e.g. "Input is too long for requested model.")
/// - **Throttled** (retryable): `throttlingException`, `tooManyRequestsException`
/// - **Auth** (non-retryable): `accessDeniedException`, `validationException`,
///   `resourceNotFoundException`
/// - **Network** (retryable): `internalServerException`, `modelStreamErrorException`,
///   `modelTimeoutException`, `serviceUnavailableException`
///
/// Unknown exception types fall through to a generic (unclassified) error so they
/// are not silently treated as retryable.
fn classify_bedrock_exception(exc_type: &str, payload: &str) -> AssistantMessageEvent {
    let exception_name = canonical_bedrock_exception_name(exc_type);

    if exception_name.as_str() == "validationexception"
        && crate::classify::is_context_overflow_message(payload)
    {
        return AssistantMessageEvent::error_context_overflow(format!(
            "Bedrock context window exceeded ({exc_type}): {payload}"
        ));
    }

    if matches!(
        exception_name.as_str(),
        "throttlingexception" | "toomanyrequestsexception"
    ) {
        AssistantMessageEvent::error_throttled(format!("Bedrock throttled: {payload}"))
    } else if matches!(
        exception_name.as_str(),
        "accessdeniedexception" | "validationexception" | "resourcenotfoundexception"
    ) {
        AssistantMessageEvent::error_auth(format!("Bedrock client error ({exc_type}): {payload}"))
    } else if matches!(
        exception_name.as_str(),
        "internalserverexception"
            | "modelstreamerrorexception"
            | "modeltimeoutexception"
            | "serviceunavailableexception"
    ) {
        AssistantMessageEvent::error_network(format!(
            "Bedrock server error ({exc_type}): {payload}"
        ))
    } else {
        // Unknown exception type — do not assume retryable.
        AssistantMessageEvent::error(format!("Bedrock exception ({exc_type}): {payload}"))
    }
}

fn canonical_bedrock_exception_name(exc_type: &str) -> String {
    exc_type
        .trim()
        .rsplit(['#', '.', '/'])
        .next()
        .unwrap_or(exc_type)
        .to_ascii_lowercase()
}

fn parse_metadata_frame(
    payload: &[u8],
    state: &mut BedrockStreamState,
) -> Result<Vec<AssistantMessageEvent>, String> {
    let event: MetadataEvent = serde_json::from_slice(payload)
        .map_err(|e| format!("Bedrock metadata parse error: {e}"))?;
    let usage = Usage::default()
        .with_input(event.usage.input_tokens)
        .with_output(event.usage.output_tokens)
        .with_total(if event.usage.total_tokens == 0 {
            event.usage.input_tokens + event.usage.output_tokens
        } else {
            event.usage.total_tokens
        });
    let stop_reason = map_stop_reason(state.stop_reason.as_deref());
    let mut events = finalize::finalize_blocks(state);
    match stop_reason {
        Ok(stop_reason) => {
            events.push(AssistantMessageEvent::Done {
                stop_reason,
                usage,
                cost: Cost::default(),
            });
        }
        Err(error_event) => {
            events.push(error_event);
        }
    }
    Ok(events)
}

fn parse_event_frame(
    event_type: &str,
    payload: &[u8],
    state: &mut BedrockStreamState,
) -> Result<Option<Vec<AssistantMessageEvent>>, String> {
    match event_type {
        "messageStart" => {
            let _event: MessageStartEvent = serde_json::from_slice(payload)
                .map_err(|e| format!("Bedrock messageStart parse error: {e}"))?;
            state.started = true;
            Ok(Some(vec![AssistantMessageEvent::Start]))
        }
        "contentBlockStart" => {
            let event: ContentBlockStartEvent = serde_json::from_slice(payload)
                .map_err(|e| format!("Bedrock contentBlockStart parse error: {e}"))?;
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
                    Ok(Some(events))
                }
                StartBlock::ToolUse { tool_use_id, name } => {
                    let (content_index, start_ev) = state.blocks.open_tool_call(tool_use_id, name);
                    state
                        .provider_blocks
                        .insert(provider_idx, (BlockType::ToolUse, content_index));
                    Ok(Some(vec![start_ev]))
                }
            }
        }
        "contentBlockDelta" => {
            let event: ContentBlockDeltaEvent = serde_json::from_slice(payload)
                .map_err(|e| format!("Bedrock contentBlockDelta parse error: {e}"))?;
            let Some((_, content_index)) = state.provider_blocks.get(&event.content_block_index)
            else {
                return Ok(None);
            };
            let content_index = *content_index;
            match event.delta {
                DeltaBlock::Text { text } => Ok(state.blocks.text_delta(text).map(|e| vec![e])),
                DeltaBlock::ToolUse { input } => Ok(Some(vec![BlockAccumulator::tool_call_delta(
                    content_index,
                    input,
                )])),
            }
        }
        "contentBlockStop" => {
            let event: ContentBlockStopEvent = serde_json::from_slice(payload)
                .map_err(|e| format!("Bedrock contentBlockStop parse error: {e}"))?;
            let Some((block_type, content_index)) =
                state.provider_blocks.remove(&event.content_block_index)
            else {
                return Ok(None);
            };
            let evt = match block_type {
                BlockType::Text => state.blocks.close_text(),
                BlockType::ToolUse => state.blocks.close_tool_call(content_index),
            };
            Ok(evt.map(|e| vec![e]))
        }
        "messageStop" => {
            let event: MessageStopEvent = serde_json::from_slice(payload)
                .map_err(|e| format!("Bedrock messageStop parse error: {e}"))?;
            state.stop_reason = Some(event.stop_reason);
            Ok(None)
        }
        "metadata" => parse_metadata_frame(payload, state).map(Some),
        _ => {
            debug!(event_type, "unknown Bedrock event type, skipping");
            Ok(None)
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

    // `ServingOptions::extra` maps onto `additionalModelRequestFields` — the
    // Converse API's own verbatim pass-through for model-native parameters
    // beyond the base `inferenceConfig` set. The base parameters that already
    // have typed fields (`temperature`, `maxTokens`) are filtered so typed
    // fields win; Converse rejects base parameters in this namespace anyway.
    let mut additional = serde_json::Map::new();
    crate::base::merge_extra(
        &mut additional,
        &options.serving.extra,
        &["temperature", "maxTokens"],
    );
    let additional_model_request_fields = (!additional.is_empty()).then_some(additional);

    BedrockRequest {
        messages,
        system,
        inference_config,
        tool_config,
        additional_model_request_fields,
    }
}

/// Convert harness messages to Bedrock message format.
///
/// This function uses a bespoke conversion instead of the shared
/// [`MessageConverter`](super::convert::MessageConverter) trait because
/// the Bedrock Converse API requires the system prompt as a separate
/// top-level field rather than as a message (handled by the caller in
/// [`build_request`]) and represents tool use/results as typed content
/// blocks (`toolUse`/`toolResult`) distinct from Anthropic's or OpenAI's
/// wire formats.
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
                        } => {
                            // Issue #619: loop-level scrub coerces incomplete tool-use
                            // blocks into object-typed arguments before reaching here.
                            // Debug builds assert the invariant to catch regressions.
                            debug_assert!(
                                arguments.is_object(),
                                "bedrock adapter: toolUse input must be a JSON object (got {arguments:?}); loop-level sanitize_incomplete_tool_calls should have coerced this before dispatch"
                            );
                            content.push(BedrockContentBlock {
                                tool_use: Some(BedrockToolUse {
                                    tool_use_id: id.clone(),
                                    name: name.clone(),
                                    input: arguments.clone(),
                                }),
                                ..BedrockContentBlock::default()
                            });
                        }
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
            // Unknown future LlmMessage variant: nothing sensible to send to
            // Bedrock, so drop it — same as messages skipped elsewhere in
            // this loop (e.g. non-LLM AgentMessage variants).
            &_ => {}
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
#[path = "bedrock_tests.rs"]
mod tests;
