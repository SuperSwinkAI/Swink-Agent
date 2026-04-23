//! Google Gemini adapter.
//!
//! Implements [`StreamFn`] for the Google Generative Language API streaming
//! endpoint used by Gemini text/tool models.

use std::collections::HashMap;
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::ApiVersion;
use swink_agent::{
    AgentContext, AgentMessage, AssistantMessage as HarnessAssistantMessage, AssistantMessageEvent,
    ContentBlock, Cost, ImageSource, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions,
    ToolResultMessage, Usage,
};

use crate::convert::extract_tool_schemas;
use crate::sse::{SseAction, SseLine, sse_data_lines_with_callback};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<GeminiToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiThinkingConfig {
    include_thoughts: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolConfig {
    function_calling_config: GeminiFunctionCallingConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallingConfig {
    mode: String,
}

#[derive(Debug, Serialize)]
struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_data: Option<GeminiFileData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFileData {
    mime_type: String,
    file_uri: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponse {
    name: String,
    response: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiChunk {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiResponseContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponsePart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thought: Option<bool>,
    #[serde(default)]
    thought_signature: Option<String>,
    #[serde(default)]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    total_token_count: u64,
}

#[derive(Debug)]
struct GeminiToolCallState {
    /// Harness-side content index returned by [`BlockAccumulator::open_tool_call`].
    content_index: usize,
    /// Latest full arguments snapshot from Gemini.  Updated on every chunk but
    /// only emitted as a single [`AssistantMessageEvent::ToolCallDelta`] at
    /// finalization to avoid corrupted concatenation when Gemini rewrites
    /// (non-prefix-extends) the snapshot between chunks.
    arguments: String,
}

#[derive(Debug, Default)]
struct GeminiStreamState {
    /// Shared block lifecycle accumulator (index allocation, open/close, drain).
    blocks: crate::block_accumulator::BlockAccumulator,
    /// Provider-part-index → harness state.  Kept for argument diffing and
    /// delta event emission; block lifecycle is owned by `blocks`.
    tool_calls: HashMap<usize, GeminiToolCallState>,
    saw_tool_call: bool,
    usage: Usage,
    stop_reason: Option<StopReason>,
    /// Set to `true` when a terminal error (e.g. SAFETY finish reason) has
    /// already been emitted, so the later `[DONE]` sentinel does not produce
    /// a duplicate terminal event.
    terminated: bool,
}

pub struct GeminiStreamFn {
    base_url: String,
    api_key: String,
    api_version: ApiVersion,
    client: Client,
}

impl GeminiStreamFn {
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        api_version: ApiVersion,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            api_version,
            client: Client::new(),
        }
    }

    const fn api_version_path(&self) -> &'static str {
        match self.api_version {
            ApiVersion::V1 => "v1",
            ApiVersion::V1beta => "v1beta",
        }
    }
}

impl std::fmt::Debug for GeminiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiStreamFn")
            .field("base_url", &self.base_url)
            .field("api_version", &self.api_version)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for GeminiStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(gemini_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

fn gemini_stream<'a>(
    gemini: &'a GeminiStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match crate::base::race_pre_stream_cancellation(
            &cancellation_token,
            "Google request cancelled",
            send_request(gemini, model, context, options),
        )
        .await
        {
            Ok(response) => response,
            Err(event) => return stream::iter(crate::base::pre_stream_error(event)).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Google Gemini HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Google");
            return stream::iter(crate::base::pre_stream_error(event)).left_stream();
        }

        parse_sse_stream(response, cancellation_token, options.on_raw_payload.clone())
            .right_stream()
    })
    .flatten()
}

async fn send_request(
    gemini: &GeminiStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!(
        "{}/{}/models/{}:streamGenerateContent?alt=sse",
        gemini.base_url,
        gemini.api_version_path(),
        model.model_id
    );
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Google Gemini request"
    );

    let body = convert_request(context, options);
    let api_key = options.api_key.as_deref().unwrap_or(&gemini.api_key);

    gemini
        .client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            AssistantMessageEvent::error_network(format!("Google connection error: {error}"))
        })
}

fn convert_request(context: &AgentContext, options: &StreamOptions) -> GeminiRequest {
    let system_instruction = (!context.system_prompt.is_empty()).then(|| GeminiContent {
        role: "user".to_string(),
        parts: vec![GeminiPart {
            text: Some(context.system_prompt.clone()),
            ..GeminiPart::default()
        }],
    });

    let contents = convert_messages(&context.messages);
    let tools = build_tools(&context.tools);
    let tool_config = (!tools.is_empty()).then(|| GeminiToolConfig {
        function_calling_config: GeminiFunctionCallingConfig {
            mode: "AUTO".to_string(),
        },
    });

    // Preserve reasoning signatures in follow-up turns and allow Gemini to emit
    // tool-compatible thinking metadata when the model supports it.
    let include_thoughts = context.messages.iter().any(contains_thinking)
        || context.messages.iter().any(contains_tool_call)
        || !context.tools.is_empty();

    let generation_config = Some(GeminiGenerationConfig {
        temperature: options.temperature,
        max_output_tokens: options.max_tokens,
        thinking_config: include_thoughts.then_some(GeminiThinkingConfig {
            include_thoughts: true,
        }),
    });

    GeminiRequest {
        system_instruction,
        contents,
        tools,
        tool_config,
        generation_config,
    }
}

fn build_tools(tools: &[std::sync::Arc<dyn swink_agent::AgentTool>]) -> Vec<GeminiTool> {
    let declarations: Vec<GeminiFunctionDeclaration> = extract_tool_schemas(tools)
        .into_iter()
        .map(|schema| GeminiFunctionDeclaration {
            name: schema.name,
            description: schema.description,
            parameters: schema.parameters,
        })
        .collect();

    if declarations.is_empty() {
        Vec::new()
    } else {
        vec![GeminiTool {
            function_declarations: declarations,
        }]
    }
}

fn contains_thinking(message: &AgentMessage) -> bool {
    let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = message else {
        return false;
    };
    assistant
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Thinking { .. }))
}

fn contains_tool_call(message: &AgentMessage) -> bool {
    let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = message else {
        return false;
    };
    assistant
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall { .. }))
}

fn convert_messages(messages: &[AgentMessage]) -> Vec<GeminiContent> {
    let mut result = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for message in messages {
        let AgentMessage::Llm(llm) = message else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                let parts = user_parts(&user.content);
                if !parts.is_empty() {
                    result.push(GeminiContent {
                        role: "user".to_string(),
                        parts,
                    });
                }
            }
            LlmMessage::Assistant(assistant) => {
                let parts = assistant_parts(assistant, &mut tool_names_by_id);
                if !parts.is_empty() {
                    result.push(GeminiContent {
                        role: "model".to_string(),
                        parts,
                    });
                }
            }
            LlmMessage::ToolResult(tool_result) => {
                let part = tool_result_part(tool_result, &tool_names_by_id);
                result.push(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![part],
                });
            }
        }
    }

    result
}

fn user_parts(blocks: &[ContentBlock]) -> Vec<GeminiPart> {
    let mut parts = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } if !text.is_empty() => parts.push(GeminiPart {
                text: Some(text.clone()),
                ..GeminiPart::default()
            }),
            ContentBlock::Image { source } => match source {
                ImageSource::Base64 { media_type, data } => parts.push(GeminiPart {
                    inline_data: Some(GeminiInlineData {
                        mime_type: media_type.clone(),
                        data: data.clone(),
                    }),
                    ..GeminiPart::default()
                }),
                ImageSource::Url { url, media_type } => parts.push(GeminiPart {
                    file_data: Some(GeminiFileData {
                        mime_type: media_type.clone(),
                        file_uri: url.clone(),
                    }),
                    ..GeminiPart::default()
                }),
                _ => {}
            },
            _ => {}
        }
    }

    parts
}

fn assistant_parts(
    assistant: &HarnessAssistantMessage,
    tool_names_by_id: &mut HashMap<String, String>,
) -> Vec<GeminiPart> {
    let mut parts = Vec::new();

    for block in &assistant.content {
        match block {
            ContentBlock::Text { text } if !text.is_empty() => parts.push(GeminiPart {
                text: Some(text.clone()),
                ..GeminiPart::default()
            }),
            ContentBlock::Thinking {
                thinking,
                signature,
            } if !thinking.is_empty() || signature.is_some() => parts.push(GeminiPart {
                text: (!thinking.is_empty()).then(|| thinking.clone()),
                thought: Some(true),
                thought_signature: signature.clone(),
                ..GeminiPart::default()
            }),
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                // Issue #619: loop-level scrub coerces incomplete tool-use blocks
                // into object-typed arguments before reaching here. Debug builds
                // assert the invariant to catch regressions.
                debug_assert!(
                    arguments.is_object(),
                    "google adapter: function_call args must be a JSON object (got {arguments:?}); loop-level sanitize_incomplete_tool_calls should have coerced this before dispatch"
                );
                tool_names_by_id.insert(id.clone(), name.clone());
                parts.push(GeminiPart {
                    function_call: Some(GeminiFunctionCall {
                        id: Some(id.clone()),
                        name: name.clone(),
                        args: arguments.clone(),
                    }),
                    ..GeminiPart::default()
                });
            }
            _ => {}
        }
    }

    parts
}

fn tool_result_part(
    result: &ToolResultMessage,
    tool_names_by_id: &HashMap<String, String>,
) -> GeminiPart {
    let name = tool_names_by_id
        .get(&result.tool_call_id)
        .cloned()
        .unwrap_or_else(|| result.tool_call_id.clone());
    let content = ContentBlock::extract_text(&result.content);
    GeminiPart {
        function_response: Some(GeminiFunctionResponse {
            name,
            response: json!({
                "toolCallId": result.tool_call_id,
                "content": content,
                "isError": result.is_error,
            }),
        }),
        ..GeminiPart::default()
    }
}

fn parse_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let line_stream = sse_data_lines_with_callback(response.bytes_stream(), on_raw_payload);

    // NOTE: Google's cancel behavior differs from Anthropic/OpenAI — it uses
    // error_network (retryable) rather than Aborted. We preserve this via a
    // custom cancel handler in on_item's None branch, and accept the generic
    // Aborted from sse_adapter_stream's cancel path. This is a semantic
    // simplification: cancellation is non-retryable regardless of error_kind.
    crate::sse::sse_adapter_stream(
        line_stream,
        cancellation_token,
        GeminiStreamState::default(),
        "Google request cancelled",
        |item, state| match item {
            None => {
                // If we already emitted a terminal event (e.g. SAFETY error),
                // don't produce a second one.
                if state.terminated {
                    return SseAction::Done(Vec::new());
                }
                if state.stop_reason.is_none() {
                    return SseAction::Done(
                        state.emit_terminal_network_error("Google stream ended unexpectedly", true),
                    );
                }
                let mut events = state.emit_final_tool_deltas();
                events.extend(crate::finalize::finalize_blocks(state));
                events.push(AssistantMessageEvent::Done {
                    stop_reason: state.stop_reason.unwrap_or(StopReason::Stop),
                    usage: state.usage.clone(),
                    cost: Cost::default(),
                });
                SseAction::Done(events)
            }
            Some(SseLine::Done) => {
                // If we already emitted a terminal event (e.g. SAFETY error),
                // don't produce a second one.
                if state.terminated {
                    return SseAction::Done(Vec::new());
                }
                let mut events = state.emit_final_tool_deltas();
                events.extend(crate::finalize::finalize_blocks(state));
                events.push(AssistantMessageEvent::Done {
                    stop_reason: state.stop_reason.unwrap_or({
                        if state.saw_tool_call {
                            StopReason::ToolUse
                        } else {
                            StopReason::Stop
                        }
                    }),
                    usage: state.usage.clone(),
                    cost: Cost::default(),
                });
                SseAction::Done(events)
            }
            Some(SseLine::Data(line)) => {
                // If we already emitted a terminal event, skip further data.
                if state.terminated {
                    return SseAction::Done(Vec::new());
                }
                let mut events = Vec::new();
                match serde_json::from_str::<GeminiChunk>(&line) {
                    Ok(chunk) => {
                        process_chunk(chunk, state, &mut events);
                        if state.terminated {
                            SseAction::Done(events)
                        } else {
                            SseAction::Continue(events)
                        }
                    }
                    Err(parse_error) => {
                        error!(error = %parse_error, "Google Gemini JSON parse error");
                        events.extend(state.emit_terminal_error(
                            AssistantMessageEvent::error(format!(
                                "Google JSON parse error: {parse_error}",
                            )),
                            true,
                        ));
                        SseAction::Done(events)
                    }
                }
            }
            Some(SseLine::TransportError(message)) => SseAction::Done(
                state.emit_terminal_network_error(format!("Google {message}"), true),
            ),
            Some(_) => SseAction::Skip,
        },
    )
}

fn process_chunk(
    chunk: GeminiChunk,
    state: &mut GeminiStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    if let Some(usage) = chunk.usage_metadata {
        state.usage = Usage {
            input: usage.prompt_token_count,
            output: usage.candidates_token_count,
            total: usage.total_token_count,
            ..Usage::default()
        };
    }

    let Some(candidate) = chunk.candidates.into_iter().next() else {
        return;
    };

    if let Some(content) = candidate.content {
        for (part_index, part) in content.parts.into_iter().enumerate() {
            if part.thought.unwrap_or(false) {
                events.extend(state.blocks.close_text());
                if let Some(ev) = state.blocks.ensure_thinking_open() {
                    events.push(ev);
                }
                if let Some(text) = part.text
                    && !text.is_empty()
                    && let Some(ev) = state.blocks.thinking_delta(text)
                {
                    events.push(ev);
                }
                if let Some(signature) = part.thought_signature {
                    state.blocks.set_thinking_signature(signature);
                }
                continue;
            }

            if let Some(function_call) = part.function_call {
                events.extend(state.blocks.close_text());
                events.extend(state.blocks.close_thinking(None));
                process_function_call(part_index, function_call, state, events);
                continue;
            }

            if let Some(text) = part.text
                && !text.is_empty()
            {
                events.extend(state.blocks.close_thinking(None));
                if let Some(ev) = state.blocks.ensure_text_open() {
                    events.push(ev);
                }
                if let Some(ev) = state.blocks.text_delta(text) {
                    events.push(ev);
                }
            }
        }
    }

    if let Some(ref finish_reason) = candidate.finish_reason {
        if finish_reason == "SAFETY" {
            warn!("Google Gemini response blocked by safety filter");
            events.extend(state.emit_final_tool_deltas());
            events.extend(state.blocks.close_text());
            events.extend(state.blocks.close_thinking(None));
            events.extend(crate::finalize::finalize_blocks(state));
            events.push(AssistantMessageEvent::error_content_filtered(
                "Google Gemini: response blocked by safety filter",
            ));
            state.terminated = true;
        } else {
            state.stop_reason = Some(map_finish_reason(finish_reason, state.saw_tool_call));
        }
    }
}

fn process_function_call(
    part_index: usize,
    function_call: GeminiFunctionCall,
    state: &mut GeminiStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    // Open a new tool-call block the first time we see this part_index.
    if !state.tool_calls.contains_key(&part_index) {
        state.saw_tool_call = true;
        let id = function_call
            .id
            .clone()
            .unwrap_or_else(|| format!("gemini-tool-{part_index}"));
        let (content_index, start_ev) = state.blocks.open_tool_call(id, function_call.name.clone());
        events.push(start_ev);
        state.tool_calls.insert(
            part_index,
            GeminiToolCallState {
                content_index,
                arguments: String::new(),
            },
        );
    }

    let entry = state
        .tool_calls
        .get_mut(&part_index)
        .expect("just inserted or already present");

    // Gemini sends full argument snapshots, not deltas.  We buffer the latest
    // snapshot and defer delta emission to finalization.  Emitting deltas
    // inline corrupts `accumulate_message` when Gemini rewrites the snapshot
    // (non-prefix change) between chunks, because `partial_json` is
    // append-only.  See issue #271.
    let serialized_args = match function_call.args {
        Value::Null => String::new(),
        value => value.to_string(),
    };
    entry.arguments = serialized_args;
}

fn map_finish_reason(finish_reason: &str, saw_tool_call: bool) -> StopReason {
    match finish_reason {
        "MAX_TOKENS" => StopReason::Length,
        _ if saw_tool_call => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

impl GeminiStreamState {
    fn emit_terminal_error(
        &mut self,
        event: AssistantMessageEvent,
        emit_final_tool_deltas: bool,
    ) -> Vec<AssistantMessageEvent> {
        let mut events = if emit_final_tool_deltas {
            self.emit_final_tool_deltas()
        } else {
            Vec::new()
        };
        events.extend(crate::finalize::finalize_blocks(self));
        events.push(event);
        events
    }

    fn emit_terminal_network_error(
        &mut self,
        message: impl Into<String>,
        emit_final_tool_deltas: bool,
    ) -> Vec<AssistantMessageEvent> {
        self.emit_terminal_error(
            AssistantMessageEvent::error_network(message.into()),
            emit_final_tool_deltas,
        )
    }

    /// Emit a single [`AssistantMessageEvent::ToolCallDelta`] per buffered tool
    /// call carrying the complete final arguments snapshot.  Must be called
    /// **before** [`finalize_blocks`](crate::finalize::finalize_blocks) (which
    /// drains and clears the tool-call map).
    fn emit_final_tool_deltas(&self) -> Vec<AssistantMessageEvent> {
        let mut ordered_tool_calls: Vec<_> = self
            .tool_calls
            .values()
            .filter(|tc| !tc.arguments.is_empty())
            .collect();
        ordered_tool_calls.sort_by_key(|tc| tc.content_index);

        ordered_tool_calls
            .into_iter()
            .map(|tc| AssistantMessageEvent::ToolCallDelta {
                content_index: tc.content_index,
                delta: tc.arguments.clone(),
            })
            .collect()
    }
}

impl crate::finalize::StreamFinalize for GeminiStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        // Clear per-chunk bookkeeping; block lifecycle is owned by `blocks`.
        self.tool_calls.clear();
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GeminiStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for #619: after the loop-level scrub runs, an assistant
    /// message that originally carried `arguments: Null` with `partial_json`
    /// set must serialize with `args: {}` so the Gemini API accepts the
    /// replayed history on the next turn.
    #[test]
    fn convert_messages_sanitized_tool_use_becomes_empty_object_args() {
        let mut assistant = HarnessAssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: Value::Null,
                partial_json: Some(r#"{"path": "/tm"#.into()),
            }],
            provider: "google".into(),
            model_id: "gemini-2.0-flash".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Length,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        };

        swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

        let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(assistant))];
        let converted = convert_messages(&messages);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "model");
        let json = serde_json::to_value(&converted[0]).unwrap();
        let part = &json["parts"][0];
        let args = &part["functionCall"]["args"];
        assert!(
            args.is_object(),
            "functionCall.args must be a JSON object, got {args:?}"
        );
        assert_eq!(args.as_object().unwrap().len(), 0);
    }

    #[test]
    fn terminal_parse_error_flushes_final_tool_delta_before_generic_error() {
        let mut state = GeminiStreamState::default();
        let (content_index, _) = state
            .blocks
            .open_tool_call("call_1".into(), "read_file".into());
        state.tool_calls.insert(
            0,
            GeminiToolCallState {
                content_index,
                arguments: r#"{"path":"foo.rs"}"#.into(),
            },
        );

        let events = state.emit_terminal_error(
            AssistantMessageEvent::error("Google JSON parse error: bad payload"),
            true,
        );
        let delta_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
            .expect("final tool delta");
        let end_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
            .expect("tool call end");
        let error_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
            .expect("terminal error");

        assert!(
            delta_index < end_index && end_index < error_index,
            "pending tool-call state must flush before the terminal error: {events:?}"
        );
        match &events[error_index] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert!(
                    error_kind.is_none(),
                    "JSON parse errors must be non-retryable protocol errors"
                );
            }
            event => panic!("expected terminal error, got {event:?}"),
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
            "terminal error path must not emit Done"
        );
    }

    #[tokio::test]
    async fn pre_cancelled_stream_aborts_before_request_send() {
        let gemini = GeminiStreamFn::new("http://127.0.0.1:1", "api-key", ApiVersion::V1beta);
        let model = ModelSpec::new("google", "gemini-2.0-flash");
        let context = AgentContext {
            system_prompt: String::new(),
            messages: vec![],
            tools: vec![],
        };
        let options = StreamOptions::default();
        let token = CancellationToken::new();
        token.cancel();

        let events: Vec<_> = gemini
            .stream(&model, &context, &options, token)
            .collect()
            .await;

        assert_eq!(events.len(), 2, "expected Start + Error: {events:?}");
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        match &events[1] {
            AssistantMessageEvent::Error {
                stop_reason,
                error_message,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::Aborted);
                assert!(
                    error_message.contains("cancelled"),
                    "unexpected cancellation message: {error_message}"
                );
            }
            other => panic!("expected aborted terminal event, got {other:?}"),
        }
    }
}
