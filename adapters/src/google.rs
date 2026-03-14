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
use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AgentMessage, AssistantMessage as HarnessAssistantMessage, ContentBlock, Cost,
    ImageSource, LlmMessage, ModelSpec, StopReason, ToolResultMessage, Usage,
};

use crate::convert::extract_tool_schemas;
use crate::sse::{SseLine, sse_data_lines};

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
    id: String,
    name: String,
    content_index: usize,
    arguments: String,
}

#[derive(Debug, Default)]
struct GeminiStreamState {
    text_started: bool,
    text_content_index: Option<usize>,
    thinking_started: bool,
    thinking_content_index: Option<usize>,
    thinking_signature: Option<String>,
    next_content_index: usize,
    tool_calls: HashMap<usize, GeminiToolCallState>,
    saw_tool_call: bool,
    usage: Usage,
    stop_reason: Option<StopReason>,
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
        let response = match send_request(gemini, model, context, options).await {
            Ok(response) => response,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Google Gemini HTTP error");
            let event = match code {
                401 | 403 => AssistantMessageEvent::error_auth(format!("Google auth error (HTTP {code}): {body}")),
                429 => AssistantMessageEvent::error_throttled(format!("Google rate limit (HTTP 429): {body}")),
                500..=599 => {
                    AssistantMessageEvent::error_network(format!("Google server error (HTTP {code}): {body}"))
                }
                _ => AssistantMessageEvent::error(format!("Google HTTP {code}: {body}")),
            };
            return stream::iter(vec![event]).left_stream();
        }

        parse_sse_stream(response, cancellation_token).right_stream()
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
        .map_err(|error| AssistantMessageEvent::error_network(format!("Google connection error: {error}")))
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
                ImageSource::Url { url } => parts.push(GeminiPart {
                    file_data: Some(GeminiFileData {
                        mime_type: "image/*".to_string(),
                        file_uri: url.clone(),
                    }),
                    ..GeminiPart::default()
                }),
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
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let line_stream = sse_data_lines(response.bytes_stream());

    stream::unfold(
        (
            Box::pin(line_stream),
            cancellation_token,
            GeminiStreamState::default(),
            false,
            true,
        ),
        |(mut lines, token, mut state, mut done, first)| async move {
            if done {
                return None;
            }

            if first {
                return Some((
                    vec![AssistantMessageEvent::Start],
                    (lines, token, state, done, false),
                ));
            }

            tokio::select! {
                biased;
                () = token.cancelled() => {
                    let mut events = crate::finalize::finalize_blocks(&mut state);
                    events.push(AssistantMessageEvent::error_network("Google request cancelled"));
                    done = true;
                    Some((events, (lines, token, state, done, false)))
                }
                maybe_line = lines.next() => {
                    match maybe_line {
                        None => {
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            if state.stop_reason.is_none() {
                                events.push(AssistantMessageEvent::error("Google stream ended unexpectedly"));
                            } else {
                                events.push(AssistantMessageEvent::Done {
                                    stop_reason: state.stop_reason.unwrap_or(StopReason::Stop),
                                    usage: state.usage.clone(),
                                    cost: Cost::default(),
                                });
                            }
                            done = true;
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Done) => {
                            let mut events = crate::finalize::finalize_blocks(&mut state);
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
                            done = true;
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Data(line)) => {
                            let mut events = Vec::new();
                            match serde_json::from_str::<GeminiChunk>(&line) {
                                Ok(chunk) => {
                                    process_chunk(chunk, &mut state, &mut events);
                                }
                                Err(parse_error) => {
                                    error!(error = %parse_error, "Google Gemini JSON parse error");
                                    events.push(AssistantMessageEvent::error(format!(
                                        "Google JSON parse error: {parse_error}"
                                    )));
                                    done = true;
                                }
                            }
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(_) => Some((Vec::new(), (lines, token, state, done, false))),
                    }
                }
            }
        },
    )
    .flat_map(stream::iter)
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
                close_text_block(state, events);
                if !state.thinking_started {
                    let content_index = state.next_content_index;
                    state.next_content_index += 1;
                    state.thinking_started = true;
                    state.thinking_content_index = Some(content_index);
                    events.push(AssistantMessageEvent::ThinkingStart { content_index });
                }
                if let Some(text) = part.text
                    && !text.is_empty()
                {
                    events.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: state.thinking_content_index.expect("thinking index set"),
                        delta: text,
                    });
                }
                if let Some(signature) = part.thought_signature {
                    state.thinking_signature = Some(signature);
                }
                continue;
            }

            if let Some(function_call) = part.function_call {
                close_text_block(state, events);
                close_thinking_block(state, events);
                process_function_call(part_index, function_call, state, events);
                continue;
            }

            if let Some(text) = part.text
                && !text.is_empty()
            {
                close_thinking_block(state, events);
                if !state.text_started {
                    let content_index = state.next_content_index;
                    state.next_content_index += 1;
                    state.text_started = true;
                    state.text_content_index = Some(content_index);
                    events.push(AssistantMessageEvent::TextStart { content_index });
                }
                events.push(AssistantMessageEvent::TextDelta {
                    content_index: state.text_content_index.expect("text index set"),
                    delta: text,
                });
            }
        }
    }

    if let Some(finish_reason) = candidate.finish_reason {
        state.stop_reason = Some(map_finish_reason(&finish_reason, state.saw_tool_call));
    }
}

fn process_function_call(
    part_index: usize,
    function_call: GeminiFunctionCall,
    state: &mut GeminiStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    let entry = state.tool_calls.entry(part_index).or_insert_with(|| {
        state.saw_tool_call = true;
        let content_index = state.next_content_index;
        state.next_content_index += 1;
        GeminiToolCallState {
            id: function_call
                .id
                .clone()
                .unwrap_or_else(|| format!("gemini-tool-{part_index}")),
            name: function_call.name.clone(),
            content_index,
            arguments: String::new(),
        }
    });

    if entry.arguments.is_empty() {
        events.push(AssistantMessageEvent::ToolCallStart {
            content_index: entry.content_index,
            id: entry.id.clone(),
            name: entry.name.clone(),
        });
    }

    let serialized_args = match function_call.args {
        Value::Null => String::new(),
        value => value.to_string(),
    };
    if serialized_args.len() > entry.arguments.len()
        && serialized_args.starts_with(&entry.arguments)
    {
        let delta = serialized_args[entry.arguments.len()..].to_string();
        if !delta.is_empty() {
            events.push(AssistantMessageEvent::ToolCallDelta {
                content_index: entry.content_index,
                delta: delta.clone(),
            });
            entry.arguments.push_str(&delta);
        }
    } else if !serialized_args.is_empty() && serialized_args != entry.arguments {
        events.push(AssistantMessageEvent::ToolCallDelta {
            content_index: entry.content_index,
            delta: serialized_args.clone(),
        });
        entry.arguments = serialized_args;
    }
}

fn map_finish_reason(finish_reason: &str, saw_tool_call: bool) -> StopReason {
    if saw_tool_call {
        return StopReason::ToolUse;
    }

    match finish_reason {
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Stop,
    }
}

fn close_text_block(state: &mut GeminiStreamState, events: &mut Vec<AssistantMessageEvent>) {
    if let Some(content_index) = state.text_content_index.take() {
        events.push(AssistantMessageEvent::TextEnd { content_index });
        state.text_started = false;
    }
}

fn close_thinking_block(state: &mut GeminiStreamState, events: &mut Vec<AssistantMessageEvent>) {
    if let Some(content_index) = state.thinking_content_index.take() {
        events.push(AssistantMessageEvent::ThinkingEnd {
            content_index,
            signature: state.thinking_signature.take(),
        });
        state.thinking_started = false;
    }
}

impl crate::finalize::StreamFinalize for GeminiStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        let mut blocks = Vec::new();

        if let Some(content_index) = self.text_content_index.take() {
            blocks.push(crate::finalize::OpenBlock::Text { content_index });
            self.text_started = false;
        }

        if let Some(content_index) = self.thinking_content_index.take() {
            blocks.push(crate::finalize::OpenBlock::Thinking {
                content_index,
                signature: self.thinking_signature.take(),
            });
            self.thinking_started = false;
        }

        let mut tool_calls: Vec<_> = self.tool_calls.values().collect();
        tool_calls.sort_by_key(|tool_call| tool_call.content_index);
        for tool_call in tool_calls {
            blocks.push(crate::finalize::OpenBlock::ToolCall {
                content_index: tool_call.content_index,
            });
        }
        self.tool_calls.clear();

        blocks
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GeminiStreamFn>();
};
