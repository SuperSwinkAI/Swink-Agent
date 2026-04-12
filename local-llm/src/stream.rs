//! [`StreamFn`] implementation for local model inference.
//!
//! [`LocalStreamFn`] wraps a [`LocalModel`] and produces
//! [`AssistantMessageEvent`] values by incrementally streaming responses
//! from the mistral.rs inference engine. Uses the `stream_chat_request`
//! API for true token-by-token delivery and mid-generation cancellation.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};
#[cfg(feature = "gemma4")]
use uuid::Uuid;

use swink_agent::stream_assembly::{BlockAccumulator, finalize_blocks};
use swink_agent::{
    AgentContext, AssistantMessageEvent, Cost, ModelSpec, StopReason, StreamFn, StreamOptions,
    Usage,
};

use crate::loader::LoaderState;
use crate::model::LocalModel;

// ─── LocalStreamFn ──────────────────────────────────────────────────────────

/// A [`StreamFn`] backed by a local GGUF model via mistral.rs.
///
/// On first call, lazily downloads and loads the model. Subsequent calls
/// reuse the loaded model. The underlying [`LocalModel`] is `Arc`-shared,
/// so multiple `LocalStreamFn` clones use the same loaded weights.
pub struct LocalStreamFn {
    model: Arc<LocalModel>,
}

impl LocalStreamFn {
    /// Create a new local stream function.
    #[must_use]
    pub const fn new(model: Arc<LocalModel>) -> Self {
        Self { model }
    }
}

impl std::fmt::Debug for LocalStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStreamFn")
            .field("model", &self.model)
            .finish()
    }
}

impl StreamFn for LocalStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(local_stream(
            &self.model,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── ChannelThoughtParser (Gemma 4) ────────────────────────────────────────

#[cfg(feature = "gemma4")]
mod gemma4 {
    /// Find the longest suffix of `haystack` that is also a prefix of `needle`.
    ///
    /// The returned length is a byte length into `haystack`, but matching is
    /// only attempted at valid UTF-8 character boundaries so callers can safely
    /// slice the original `&str`.
    pub(super) fn partial_prefix_at_end(haystack: &str, needle: &str) -> Option<usize> {
        if needle.len() <= 1 || haystack.is_empty() {
            return None;
        }

        let min_start = haystack
            .len()
            .saturating_sub(needle.len().saturating_sub(1));

        for (start, _) in haystack.char_indices() {
            if start < min_start {
                continue;
            }

            let suffix = &haystack[start..];
            if needle.starts_with(suffix) {
                return Some(haystack.len() - start);
            }
        }

        if needle.starts_with(haystack) && haystack.len() < needle.len() {
            Some(haystack.len())
        } else {
            None
        }
    }
}

#[cfg(feature = "gemma4")]
mod channel_thought {
    use super::gemma4::partial_prefix_at_end;

    /// Opening delimiter for Gemma 4 thinking blocks.
    const OPEN_DELIM: &str = "<|channel>thought\n";
    /// Closing delimiter for Gemma 4 thinking blocks.
    const CLOSE_DELIM: &str = "<channel|>";

    /// Parser state for Gemma 4's `<|channel>thought\n...<channel|>` format.
    ///
    /// Handles cross-chunk boundary splitting of both open and close delimiters.
    #[derive(Debug)]
    pub(super) struct ChannelThoughtParser {
        state: State,
        /// Buffer for accumulating partial delimiter matches.
        buffer: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        /// Normal text output.
        Normal,
        /// Seen a partial open delimiter prefix — buffering until match or mismatch.
        PartialOpen,
        /// Inside a thinking block.
        InThinking,
        /// Seen a partial close delimiter prefix while inside thinking.
        PartialClose,
    }

    impl ChannelThoughtParser {
        pub const fn new() -> Self {
            Self {
                state: State::Normal,
                buffer: String::new(),
            }
        }

        /// Process a chunk of content, returning thinking and text parts.
        ///
        /// Returns `(Option<thinking_content>, Option<text_content>)`.
        pub fn process(&mut self, content: &str) -> (Option<String>, Option<String>) {
            self.buffer.push_str(content);

            let mut thinking_out: Option<String> = None;
            let mut text_out: Option<String> = None;

            // Process buffer until stable (no more transitions possible).
            loop {
                match self.state {
                    State::Normal => {
                        if let Some(pos) = self.buffer.find("<|channel>thought\n") {
                            // Flush text before the delimiter.
                            let before = &self.buffer[..pos];
                            if !before.is_empty() {
                                append(&mut text_out, before);
                            }
                            // Consume the open delimiter.
                            let rest = self.buffer[pos + OPEN_DELIM.len()..].to_string();
                            self.buffer = rest;
                            self.state = State::InThinking;
                            continue;
                        }
                        // Check for partial open delimiter at end of buffer.
                        if let Some(partial_len) = partial_prefix_at_end(&self.buffer, OPEN_DELIM) {
                            let flush_end = self.buffer.len() - partial_len;
                            let flush = &self.buffer[..flush_end];
                            if !flush.is_empty() {
                                append(&mut text_out, flush);
                            }
                            let rest = self.buffer[flush_end..].to_string();
                            self.buffer = rest;
                            self.state = State::PartialOpen;
                            break;
                        }
                        // No delimiter found — flush everything as text.
                        if !self.buffer.is_empty() {
                            append(&mut text_out, &self.buffer.clone());
                            self.buffer.clear();
                        }
                        break;
                    }
                    State::PartialOpen => {
                        if self.buffer.len() >= OPEN_DELIM.len() {
                            if self.buffer.starts_with(OPEN_DELIM) {
                                let rest = self.buffer[OPEN_DELIM.len()..].to_string();
                                self.buffer = rest;
                                self.state = State::InThinking;
                                continue;
                            }
                            // Mismatch — flush buffer as text.
                            self.state = State::Normal;
                            continue;
                        }
                        // Still partial — check if it's still a valid prefix.
                        if OPEN_DELIM.starts_with(&self.buffer) {
                            break; // Wait for more data.
                        }
                        // Not a valid prefix — flush as text.
                        self.state = State::Normal;
                    }
                    State::InThinking => {
                        if let Some(pos) = self.buffer.find(CLOSE_DELIM) {
                            let thinking = &self.buffer[..pos];
                            if !thinking.is_empty() {
                                append(&mut thinking_out, thinking);
                            }
                            let rest = self.buffer[pos + CLOSE_DELIM.len()..].to_string();
                            self.buffer = rest;
                            self.state = State::Normal;
                            continue;
                        }
                        // Check for partial close delimiter at end.
                        if let Some(partial_len) = partial_prefix_at_end(&self.buffer, CLOSE_DELIM)
                        {
                            let flush_end = self.buffer.len() - partial_len;
                            let flush = &self.buffer[..flush_end];
                            if !flush.is_empty() {
                                append(&mut thinking_out, flush);
                            }
                            let rest = self.buffer[flush_end..].to_string();
                            self.buffer = rest;
                            self.state = State::PartialClose;
                            break;
                        }
                        // No delimiter — flush all as thinking.
                        if !self.buffer.is_empty() {
                            append(&mut thinking_out, &self.buffer.clone());
                            self.buffer.clear();
                        }
                        break;
                    }
                    State::PartialClose => {
                        if self.buffer.len() >= CLOSE_DELIM.len() {
                            if self.buffer.starts_with(CLOSE_DELIM) {
                                let rest = self.buffer[CLOSE_DELIM.len()..].to_string();
                                self.buffer = rest;
                                self.state = State::Normal;
                                continue;
                            }
                            // Mismatch — flush buffer as thinking.
                            self.state = State::InThinking;
                            continue;
                        }
                        // Still partial — check if it's still a valid prefix.
                        if CLOSE_DELIM.starts_with(&self.buffer) {
                            break; // Wait for more data.
                        }
                        // Not a valid prefix — flush as thinking.
                        self.state = State::InThinking;
                    }
                }
            }

            (thinking_out, text_out)
        }
    }

    /// Append `s` to an `Option<String>`, creating it if `None`.
    fn append(target: &mut Option<String>, s: &str) {
        match target {
            Some(existing) => existing.push_str(s),
            None => *target = Some(s.to_string()),
        }
    }
}

// ─── ToolCallParser (Gemma 4) ─────────────────────────────────────────────

#[cfg(feature = "gemma4")]
mod tool_call {
    use super::gemma4::partial_prefix_at_end;

    /// Opening delimiter for Gemma 4 tool calls.
    const OPEN_DELIM: &str = "<|tool_call>call:";
    /// Closing delimiter for Gemma 4 tool calls.
    const CLOSE_DELIM: &str = "<tool_call|>";

    /// A tool call extracted from Gemma 4 streaming output.
    pub(super) struct ParsedToolCall {
        pub name: String,
        pub args: String,
    }

    /// Parser state for Gemma 4's `<|tool_call>call:{name}{args}<tool_call|>` format.
    ///
    /// Handles cross-chunk boundary splitting of both open and close delimiters.
    #[derive(Debug)]
    pub(super) struct ToolCallParser {
        state: State,
        buffer: String,
        name_buf: String,
        args_buf: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        /// Normal text output.
        Normal,
        /// Seen a partial open delimiter prefix — buffering until match or mismatch.
        PartialOpen,
        /// Inside the function name (before the first `{`).
        InName,
        /// Inside the JSON arguments (after `{`, up to `<tool_call|>`).
        InArgs,
        /// Seen a partial close delimiter prefix while inside arguments.
        PartialClose,
    }

    impl ToolCallParser {
        pub const fn new() -> Self {
            Self {
                state: State::Normal,
                buffer: String::new(),
                name_buf: String::new(),
                args_buf: String::new(),
            }
        }

        /// Process a chunk of content, returning completed tool calls and remaining text.
        ///
        /// Returns `(completed_calls, Option<text_content>)`.
        #[allow(clippy::too_many_lines)]
        pub fn process(&mut self, content: &str) -> (Vec<ParsedToolCall>, Option<String>) {
            self.buffer.push_str(content);

            let mut calls: Vec<ParsedToolCall> = Vec::new();
            let mut text_out: Option<String> = None;

            loop {
                match self.state {
                    State::Normal => {
                        if let Some(pos) = self.buffer.find(OPEN_DELIM) {
                            let before = &self.buffer[..pos];
                            if !before.is_empty() {
                                append(&mut text_out, before);
                            }
                            let rest = self.buffer[pos + OPEN_DELIM.len()..].to_string();
                            self.buffer = rest;
                            self.name_buf.clear();
                            self.args_buf.clear();
                            self.state = State::InName;
                            continue;
                        }
                        if let Some(partial_len) = partial_prefix_at_end(&self.buffer, OPEN_DELIM) {
                            let flush_end = self.buffer.len() - partial_len;
                            let flush = &self.buffer[..flush_end];
                            if !flush.is_empty() {
                                append(&mut text_out, flush);
                            }
                            let rest = self.buffer[flush_end..].to_string();
                            self.buffer = rest;
                            self.state = State::PartialOpen;
                            break;
                        }
                        if !self.buffer.is_empty() {
                            append(&mut text_out, &self.buffer.clone());
                            self.buffer.clear();
                        }
                        break;
                    }
                    State::PartialOpen => {
                        if self.buffer.len() >= OPEN_DELIM.len() {
                            if self.buffer.starts_with(OPEN_DELIM) {
                                let rest = self.buffer[OPEN_DELIM.len()..].to_string();
                                self.buffer = rest;
                                self.name_buf.clear();
                                self.args_buf.clear();
                                self.state = State::InName;
                                continue;
                            }
                            self.state = State::Normal;
                            continue;
                        }
                        if OPEN_DELIM.starts_with(&self.buffer) {
                            break;
                        }
                        self.state = State::Normal;
                    }
                    State::InName => {
                        if let Some(pos) = self.buffer.find('{') {
                            self.name_buf.push_str(&self.buffer[..pos]);
                            let rest = self.buffer[pos..].to_string();
                            self.buffer = rest;
                            self.state = State::InArgs;
                            continue;
                        }
                        self.name_buf.push_str(&self.buffer);
                        self.buffer.clear();
                        break;
                    }
                    State::InArgs => {
                        if let Some(pos) = self.buffer.find(CLOSE_DELIM) {
                            self.args_buf.push_str(&self.buffer[..pos]);
                            let rest = self.buffer[pos + CLOSE_DELIM.len()..].to_string();
                            self.buffer = rest;
                            calls.push(ParsedToolCall {
                                name: self.name_buf.trim().to_string(),
                                args: self.args_buf.clone(),
                            });
                            self.name_buf.clear();
                            self.args_buf.clear();
                            self.state = State::Normal;
                            continue;
                        }
                        if let Some(partial_len) = partial_prefix_at_end(&self.buffer, CLOSE_DELIM)
                        {
                            let flush_end = self.buffer.len() - partial_len;
                            self.args_buf.push_str(&self.buffer[..flush_end]);
                            let rest = self.buffer[flush_end..].to_string();
                            self.buffer = rest;
                            self.state = State::PartialClose;
                            break;
                        }
                        self.args_buf.push_str(&self.buffer);
                        self.buffer.clear();
                        break;
                    }
                    State::PartialClose => {
                        if self.buffer.len() >= CLOSE_DELIM.len() {
                            if self.buffer.starts_with(CLOSE_DELIM) {
                                let rest = self.buffer[CLOSE_DELIM.len()..].to_string();
                                self.buffer = rest;
                                calls.push(ParsedToolCall {
                                    name: self.name_buf.trim().to_string(),
                                    args: self.args_buf.clone(),
                                });
                                self.name_buf.clear();
                                self.args_buf.clear();
                                self.state = State::Normal;
                                continue;
                            }
                            self.state = State::InArgs;
                            continue;
                        }
                        if CLOSE_DELIM.starts_with(&self.buffer) {
                            break;
                        }
                        self.state = State::InArgs;
                    }
                }
            }

            (calls, text_out)
        }
    }

    fn append(target: &mut Option<String>, s: &str) {
        match target {
            Some(existing) => existing.push_str(s),
            None => *target = Some(s.to_string()),
        }
    }
}

// ─── Streaming state ────────────────────────────────────────────────────────

/// Mutable state accumulated across streaming chunks.
struct StreamState {
    events: Vec<AssistantMessageEvent>,
    blocks: BlockAccumulator,
    accumulated_usage: Option<mistralrs::Usage>,
    finish_reason: Option<String>,
    has_tool_calls: bool,
    /// Map from tool call index to (`tool_id`, `content_index` at start).
    active_tool_calls: Vec<(String, usize)>,
    /// Gemma 4 channel-thought parser (only present for Gemma 4 models).
    #[cfg(feature = "gemma4")]
    channel_parser: Option<channel_thought::ChannelThoughtParser>,
    /// Gemma 4 native tool call parser (only present for Gemma 4 models).
    #[cfg(feature = "gemma4")]
    tool_call_parser: Option<tool_call::ToolCallParser>,
}

impl StreamState {
    fn new(is_gemma4: bool) -> Self {
        #[cfg(not(feature = "gemma4"))]
        let _ = is_gemma4;

        Self {
            events: vec![AssistantMessageEvent::Start],
            blocks: BlockAccumulator::new(),
            accumulated_usage: None,
            finish_reason: None,
            has_tool_calls: false,
            active_tool_calls: Vec::new(),
            #[cfg(feature = "gemma4")]
            channel_parser: if is_gemma4 {
                Some(channel_thought::ChannelThoughtParser::new())
            } else {
                None
            },
            #[cfg(feature = "gemma4")]
            tool_call_parser: if is_gemma4 {
                Some(tool_call::ToolCallParser::new())
            } else {
                None
            },
        }
    }

    /// Process a streaming chunk, appending events to the buffer.
    fn process_chunk(&mut self, chunk: &mistralrs::ChatCompletionChunkResponse) {
        if let Some(usage) = &chunk.usage {
            self.accumulated_usage = Some(usage.clone());
        }

        let Some(choice) = chunk.choices.first() else {
            return;
        };

        if let Some(reason) = &choice.finish_reason {
            self.finish_reason = Some(reason.clone());
        }

        self.process_reasoning_delta(choice);
        self.process_content_delta(choice);
        self.process_tool_call_delta(choice);
    }

    /// Process a final Done response, extracting usage and metadata.
    fn handle_done(&mut self, resp: &mistralrs::ChatCompletionResponse) {
        if self.accumulated_usage.is_none() {
            self.accumulated_usage = Some(resp.usage.clone());
        }
        if let Some(choice) = resp.choices.first() {
            if self.finish_reason.is_none() {
                self.finish_reason = Some(choice.finish_reason.clone());
            }
            if choice
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty())
            {
                self.has_tool_calls = true;
            }
        }
    }

    fn process_reasoning_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            self.events.extend(self.blocks.ensure_thinking_open());
            self.events
                .extend(self.blocks.thinking_delta(reasoning.clone()));
        }
    }

    fn process_content_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        let Some(content) = &choice.delta.content else {
            return;
        };

        // Step 1: Extract thinking blocks (Gemma 4 channel-thought or SmolLM3 <think>).
        #[cfg(feature = "gemma4")]
        let (thinking_part, text_part) = self.channel_parser.as_mut().map_or_else(
            || {
                let (t, txt) = extract_thinking_delta(content);
                (t, if txt.is_empty() { None } else { Some(txt) })
            },
            |parser| parser.process(content),
        );
        #[cfg(not(feature = "gemma4"))]
        let (thinking_part, text_part) = {
            let (t, txt) = extract_thinking_delta(content);
            (t, if txt.is_empty() { None } else { Some(txt) })
        };

        // Emit thinking events.
        if let Some(think) = thinking_part
            && !think.is_empty()
        {
            self.events.extend(self.blocks.ensure_thinking_open());
            self.events.extend(self.blocks.thinking_delta(think));
        }

        // Step 2: Pass text through Gemma 4 tool call parser or emit directly.
        #[cfg(feature = "gemma4")]
        let final_text = if let Some(text) = text_part {
            if let Some(parser) = self.tool_call_parser.as_mut() {
                let (calls, remaining) = parser.process(&text);
                for call in calls {
                    self.emit_gemma4_tool_call(call);
                }
                remaining
            } else {
                Some(text)
            }
        } else {
            None
        };
        #[cfg(not(feature = "gemma4"))]
        let final_text = text_part;

        // Emit text events.
        if let Some(text) = final_text
            && !text.is_empty()
        {
            self.events.extend(self.blocks.close_thinking(None));
            self.events.extend(self.blocks.ensure_text_open());
            self.events.extend(self.blocks.text_delta(text));
        }
    }

    /// Emit `ToolCallStart` + optional `ToolCallDelta` for a Gemma 4 native tool call.
    ///
    /// `ToolCallEnd` is deferred to [`finalize`] via `active_tool_calls`, matching the
    /// pattern used for mistralrs-native tool calls.
    #[cfg(feature = "gemma4")]
    fn emit_gemma4_tool_call(&mut self, call: tool_call::ParsedToolCall) {
        self.events.extend(self.blocks.close_text());
        self.events.extend(self.blocks.close_thinking(None));

        let id = Uuid::new_v4().to_string();
        self.has_tool_calls = true;
        let (tc_content_index, start_ev) = self.blocks.open_tool_call(id.clone(), call.name);
        self.active_tool_calls.push((id, tc_content_index));
        self.events.push(start_ev);

        if !call.args.is_empty() {
            self.events.push(BlockAccumulator::tool_call_delta(
                tc_content_index,
                call.args,
            ));
        }
        // ToolCallEnd emitted in finalize() via BlockAccumulator::drain_open_blocks.
    }

    fn process_tool_call_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        let Some(tool_calls) = &choice.delta.tool_calls else {
            return;
        };

        self.events.extend(self.blocks.close_text());
        self.events.extend(self.blocks.close_thinking(None));

        for tc in tool_calls {
            self.has_tool_calls = true;
            let tool_idx = tc.index;
            while self.active_tool_calls.len() <= tool_idx {
                self.active_tool_calls.push((String::new(), 0));
            }
            if self.active_tool_calls[tool_idx].0.is_empty() {
                let (content_index, start_ev) = self
                    .blocks
                    .open_tool_call(tc.id.clone(), tc.function.name.clone());
                self.active_tool_calls[tool_idx] = (tc.id.clone(), content_index);
                self.events.push(start_ev);
            }
            let tc_content_index = self.active_tool_calls[tool_idx].1;
            if !tc.function.arguments.is_empty() {
                self.events.push(BlockAccumulator::tool_call_delta(
                    tc_content_index,
                    tc.function.arguments.clone(),
                ));
            }
        }
    }

    fn finalize(mut self) -> Vec<AssistantMessageEvent> {
        self.events.extend(finalize_blocks(&mut self.blocks));

        let stop_reason = match self.finish_reason.as_deref() {
            Some("length") => StopReason::Length,
            _ if self.has_tool_calls => StopReason::ToolUse,
            _ => StopReason::Stop,
        };

        self.events.push(AssistantMessageEvent::Done {
            stop_reason,
            usage: build_usage(self.accumulated_usage.as_ref()),
            cost: Cost::default(),
        });

        self.events
    }

    fn finalize_cancelled(mut self) -> Vec<AssistantMessageEvent> {
        self.events.extend(finalize_blocks(&mut self.blocks));
        self.events
            .push(AssistantMessageEvent::error("local inference cancelled"));
        self.events
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

// The `mistralrs::model::Stream` type is not re-exported, so the streaming
// loop must live in this function — it cannot be extracted into a helper
// that names the stream type in its signature. This slightly exceeds the
// 100-line limit but keeps the code correct.
#[allow(clippy::too_many_lines)]
fn local_stream<'a>(
    local_model: &'a LocalModel,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    _options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        if let Err(e) = local_model.ensure_ready().await {
            return stream::iter(vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::error(format!("local model not ready: {e}")),
            ]);
        }
        if cancellation_token.is_cancelled() {
            return stream::iter(vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::error("local inference cancelled"),
            ]);
        }

        // Determine model family and thinking state for Gemma 4 support.
        #[cfg(feature = "gemma4")]
        let is_gemma4 = local_model.config().is_gemma4();
        #[cfg(not(feature = "gemma4"))]
        let is_gemma4 = false;

        let thinking_enabled = model
            .capabilities
            .as_ref()
            .is_some_and(|c| c.supports_thinking);

        let messages = crate::convert::convert_context_messages(
            context,
            local_model.config(),
            thinking_enabled,
        );
        debug!(
            provider = %model.provider,
            model_id = %model.model_id,
            message_count = context.messages.len(),
            "sending local inference request (streaming)"
        );

        let state_guard = match local_model.runner().await {
            Ok(guard) => guard,
            Err(e) => {
                return stream::iter(vec![
                    AssistantMessageEvent::Start,
                    AssistantMessageEvent::error(format!("model runner unavailable: {e}")),
                ]);
            }
        };
        let LoaderState::Ready { runner } = &*state_guard else {
            return stream::iter(vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::error("model in unexpected state"),
            ]);
        };
        let mut mistral_stream = match runner.stream_chat_request(messages).await {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "local streaming request failed");
                return stream::iter(vec![
                    AssistantMessageEvent::Start,
                    AssistantMessageEvent::error(format!("local inference error: {e}")),
                ]);
            }
        };

        let mut state = StreamState::new(is_gemma4);
        while let Some(response) = mistral_stream.next().await {
            if cancellation_token.is_cancelled() {
                return stream::iter(state.finalize_cancelled());
            }
            match response {
                mistralrs::Response::Chunk(chunk) => state.process_chunk(&chunk),
                mistralrs::Response::Done(done) => {
                    state.handle_done(&done);
                    break;
                }
                mistralrs::Response::InternalError(e) => {
                    error!(error = %e, "internal error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local inference error: {e}"
                    )));
                    return stream::iter(state.events);
                }
                mistralrs::Response::ValidationError(e) => {
                    error!(error = %e, "validation error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local validation error: {e}"
                    )));
                    return stream::iter(state.events);
                }
                mistralrs::Response::ModelError(msg, _) => {
                    error!(error = %msg, "model error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local model error: {msg}"
                    )));
                    return stream::iter(state.events);
                }
                _ => warn!("unexpected response variant during streaming"),
            }
        }

        // Keep state_guard alive until stream is fully consumed.
        drop(state_guard);
        stream::iter(state.finalize())
    })
    .flatten()
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn build_usage(usage: Option<&mistralrs::Usage>) -> Usage {
    usage.map_or_else(Usage::default, |u| Usage {
        input: u64::try_from(u.prompt_tokens).unwrap_or(0),
        output: u64::try_from(u.completion_tokens).unwrap_or(0),
        total: u64::try_from(u.total_tokens).unwrap_or(0),
        ..Default::default()
    })
}

/// Extract `<think>...</think>` content from a streaming delta.
///
/// In streaming mode, `SmolLM3` may emit `<think>` tags across multiple
/// deltas. This handles the simple case where the full tag appears in one
/// delta; cross-delta tag boundaries are handled by the accumulator seeing
/// partial tags as text (acceptable degradation for streaming).
fn extract_thinking_delta(content: &str) -> (Option<String>, String) {
    let think_start = "<think>";
    let think_end = "</think>";

    if let Some(start_idx) = content.find(think_start)
        && let Some(end_idx) = content.find(think_end)
    {
        let thinking = content[start_idx + think_start.len()..end_idx]
            .trim()
            .to_string();
        let before = &content[..start_idx];
        let after = &content[end_idx + think_end.len()..];
        let text = format!("{before}{after}").trim().to_string();
        return (
            if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            text,
        );
    }

    (None, content.to_string())
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LocalStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_thinking_no_tags() {
        let (thinking, text) = extract_thinking_delta("Hello, world!");
        assert!(thinking.is_none());
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn extract_thinking_with_tags() {
        let input = "<think>I need to reason about this.</think>The answer is 42.";
        let (thinking, text) = extract_thinking_delta(input);
        assert_eq!(thinking.as_deref(), Some("I need to reason about this."));
        assert_eq!(text, "The answer is 42.");
    }

    #[test]
    fn extract_thinking_empty_tags() {
        let input = "<think></think>Just text.";
        let (thinking, text) = extract_thinking_delta(input);
        assert!(thinking.is_none());
        assert_eq!(text, "Just text.");
    }

    #[test]
    fn extract_thinking_with_content_before() {
        let input = "Before <think>reasoning</think> after";
        let (thinking, text) = extract_thinking_delta(input);
        assert_eq!(thinking.as_deref(), Some("reasoning"));
        assert_eq!(text, "Before  after");
    }

    #[test]
    fn extract_thinking_unclosed_tag() {
        let input = "<think>unclosed thinking";
        let (thinking, text) = extract_thinking_delta(input);
        assert!(thinking.is_none());
        assert_eq!(text, "<think>unclosed thinking");
    }

    #[test]
    fn finalize_cancelled_emits_error_terminal() {
        let mut state = StreamState::new(false);
        // Simulate having started a text block via BlockAccumulator.
        let start = state.blocks.ensure_text_open();
        state.events.extend(start);

        let events = state.finalize_cancelled();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(
                    error_message.contains("cancelled"),
                    "expected cancellation message, got: {error_message}"
                );
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
        // Open text block must be closed before the terminal error.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. }))
        );
    }

    #[test]
    fn build_usage_none() {
        let usage = build_usage(None);
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.total, 0);
    }

    #[test]
    fn build_usage_from_mistral() {
        let mistral_usage = mistralrs::Usage {
            prompt_tokens: 42,
            completion_tokens: 13,
            total_tokens: 55,
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        };
        let usage = build_usage(Some(&mistral_usage));
        assert_eq!(usage.input, 42);
        assert_eq!(usage.output, 13);
        assert_eq!(usage.total, 55);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
    }

    #[test]
    fn finalize_preserves_length_stop_reason_after_tool_calls() {
        let mut state = StreamState::new(false);
        state.has_tool_calls = true;
        state.finish_reason = Some("length".to_string());

        let events = state.finalize();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Length);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[test]
    fn finalize_keeps_tool_use_for_non_length_tool_call_turns() {
        let mut state = StreamState::new(false);
        state.has_tool_calls = true;

        let events = state.finalize();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::super::channel_thought::ChannelThoughtParser;
        use super::super::gemma4::partial_prefix_at_end;

        #[test]
        fn channel_thought_single_chunk() {
            let mut parser = ChannelThoughtParser::new();
            let (thinking, text) = parser.process("<|channel>thought\nreasoning here<channel|>");
            assert_eq!(thinking.as_deref(), Some("reasoning here"));
            assert!(text.is_none());
        }

        #[test]
        fn channel_thought_cross_chunk_open() {
            let mut parser = ChannelThoughtParser::new();
            // Split the opening delimiter across two chunks.
            let (t1, txt1) = parser.process("<|channel>");
            // Partial open — nothing emitted yet.
            assert!(t1.is_none());
            assert!(txt1.is_none());

            let (t2, txt2) = parser.process("thought\nthinking content<channel|>");
            assert_eq!(t2.as_deref(), Some("thinking content"));
            assert!(txt2.is_none());
        }

        #[test]
        fn channel_thought_cross_chunk_close() {
            let mut parser = ChannelThoughtParser::new();
            let (t1, txt1) = parser.process("<|channel>thought\nsome reasoning<chan");
            // Thinking content flushed, partial close buffered.
            assert_eq!(t1.as_deref(), Some("some reasoning"));
            assert!(txt1.is_none());

            let (t2, txt2) = parser.process("nel|>after text");
            assert!(t2.is_none());
            assert_eq!(txt2.as_deref(), Some("after text"));
        }

        #[test]
        fn channel_thought_partial_match_is_utf8_safe() {
            let haystack = "alpha🙂<|chan";
            assert_eq!(
                partial_prefix_at_end(haystack, "<|channel>thought\n"),
                Some(6)
            );

            let mut parser = ChannelThoughtParser::new();
            let (t1, txt1) = parser.process("alpha🙂<|chan");
            assert!(t1.is_none());
            assert_eq!(txt1.as_deref(), Some("alpha🙂"));

            let (t2, txt2) = parser.process("nel>thought\nreasoning<channel|>");
            assert_eq!(t2.as_deref(), Some("reasoning"));
            assert!(txt2.is_none());
        }

        #[test]
        fn channel_thought_no_delimiters() {
            let mut parser = ChannelThoughtParser::new();
            let (thinking, text) = parser.process("Hello, world!");
            assert!(thinking.is_none());
            assert_eq!(text.as_deref(), Some("Hello, world!"));
        }

        #[test]
        fn channel_thought_multiple_blocks() {
            let mut parser = ChannelThoughtParser::new();
            let input = "<|channel>thought\nfirst<channel|><|channel>thought\nsecond<channel|>";
            let (thinking, text) = parser.process(input);
            // Both thinking blocks merged into one output.
            assert_eq!(thinking.as_deref(), Some("firstsecond"));
            assert!(text.is_none());
        }

        #[test]
        fn channel_thought_mixed_text_and_thinking() {
            let mut parser = ChannelThoughtParser::new();

            let (t1, txt1) = parser.process("before ");
            assert!(t1.is_none());
            assert_eq!(txt1.as_deref(), Some("before "));

            let (t2, txt2) = parser.process("<|channel>thought\nreasoning<channel|>");
            assert_eq!(t2.as_deref(), Some("reasoning"));
            assert!(txt2.is_none());

            let (t3, txt3) = parser.process(" after");
            assert!(t3.is_none());
            assert_eq!(txt3.as_deref(), Some(" after"));
        }

        // ── T047-T050: ToolCallParser tests ──────────────────────────────────

        #[test]
        fn tool_call_single_chunk() {
            use super::super::tool_call::ToolCallParser;
            let mut parser = ToolCallParser::new();
            let (calls, text) =
                parser.process(r#"<|tool_call>call:read_file{"path":"foo.rs"}<tool_call|>"#);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "read_file");
            assert_eq!(calls[0].args, r#"{"path":"foo.rs"}"#);
            assert!(text.is_none());
        }

        #[test]
        fn tool_call_cross_chunk() {
            use super::super::tool_call::ToolCallParser;
            let mut parser = ToolCallParser::new();
            // Split at the close delimiter boundary.
            let (calls1, text1) =
                parser.process(r#"<|tool_call>call:read_file{"path":"foo.rs"}<tool_call"#);
            assert!(calls1.is_empty());
            assert!(text1.is_none());

            let (calls2, text2) = parser.process("|>");
            assert_eq!(calls2.len(), 1);
            assert_eq!(calls2[0].name, "read_file");
            assert_eq!(calls2[0].args, r#"{"path":"foo.rs"}"#);
            assert!(text2.is_none());
        }

        #[test]
        fn tool_call_partial_match_is_utf8_safe() {
            use super::super::tool_call::ToolCallParser;

            let haystack = r#"prefix🙂<tool_cal"#;
            assert_eq!(partial_prefix_at_end(haystack, "<tool_call|>"), Some(9));

            let mut parser = ToolCallParser::new();
            let (calls1, text1) =
                parser.process(r#"prefix🙂<|tool_call>call:read_file{"path":"foo.rs"}<tool_cal"#);
            assert!(calls1.is_empty());
            assert_eq!(text1.as_deref(), Some("prefix🙂"));

            let (calls2, text2) = parser.process("l|>");
            assert_eq!(calls2.len(), 1);
            assert_eq!(calls2[0].name, "read_file");
            assert_eq!(calls2[0].args, r#"{"path":"foo.rs"}"#);
            assert!(text2.is_none());
        }

        #[test]
        fn tool_call_no_delimiters() {
            use super::super::tool_call::ToolCallParser;
            let mut parser = ToolCallParser::new();
            let (calls, text) = parser.process("Hello, world!");
            assert!(calls.is_empty());
            assert_eq!(text.as_deref(), Some("Hello, world!"));
        }

        #[test]
        fn tool_call_with_thinking() {
            // Simulate the full pipeline: ChannelThoughtParser → ToolCallParser.
            let mut think_parser = ChannelThoughtParser::new();
            let input = r#"<|channel>thought
reasoning<channel|><|tool_call>call:read_file{"path":"foo.rs"}<tool_call|>"#;
            let (thinking, text_opt) = think_parser.process(input);
            assert_eq!(thinking.as_deref(), Some("reasoning"));

            let text = text_opt.expect("tool call text must follow thinking block");
            use super::super::tool_call::ToolCallParser;
            let mut tool_parser = ToolCallParser::new();
            let (calls, remaining) = tool_parser.process(&text);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "read_file");
            assert!(remaining.filter(|s| !s.is_empty()).is_none());
        }

        #[test]
        fn channel_thought_delimiter_in_text() {
            // Quoted delimiter text without the trailing newline should NOT trigger.
            // `<|channel>thought` without `\n` is not a valid open delimiter.
            let mut parser = ChannelThoughtParser::new();
            let (thinking, text) = parser.process("The format is <|channel>thought end");
            assert!(thinking.is_none());
            // The parser sees a partial open at `<|channel>thought` but then
            // gets ` end` which doesn't continue the delimiter — flushes as text.
            // We just need it to NOT produce thinking events.
            assert!(text.is_some());
        }
    }
}
