//! [`StreamFn`] implementation for local model inference.
//!
//! [`LocalStreamFn`] wraps a [`LocalModel`] and produces
//! [`AssistantMessageEvent`] values by incrementally streaming responses
//! from the llama.cpp inference engine via the internal `LlamaRunner`.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use llama_cpp_2::model::LlamaChatMessage;
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
use crate::runner::{FinishReason, GenerateOptions, TokenEvent};

// ─── LocalStreamFn ──────────────────────────────────────────────────────────

/// A [`StreamFn`] backed by a local GGUF model via llama.cpp.
pub struct LocalStreamFn {
    model: Arc<LocalModel>,
}

impl LocalStreamFn {
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

mod delimiter {
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

mod think_tags {
    use super::delimiter::partial_prefix_at_end;

    const OPEN_DELIM: &str = "<think>";
    const CLOSE_DELIM: &str = "</think>";

    #[derive(Debug)]
    pub(super) struct ThinkTagParser {
        state: State,
        buffer: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        PartialOpen,
        InThinking,
        PartialClose,
    }

    impl ThinkTagParser {
        pub const fn new() -> Self {
            Self {
                state: State::Normal,
                buffer: String::new(),
            }
        }

        pub fn process(&mut self, content: &str) -> (Option<String>, Option<String>) {
            self.buffer.push_str(content);

            let mut thinking_out: Option<String> = None;
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
                            self.state = State::InThinking;
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
                                self.state = State::InThinking;
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
                            self.state = State::InThinking;
                            continue;
                        }
                        if CLOSE_DELIM.starts_with(&self.buffer) {
                            break;
                        }
                        self.state = State::InThinking;
                    }
                }
            }

            (thinking_out, text_out)
        }

        pub fn finish(&mut self) -> (Option<String>, Option<String>) {
            if self.buffer.is_empty() {
                return (None, None);
            }

            let buffered = std::mem::take(&mut self.buffer);
            let state = self.state;
            self.state = State::Normal;

            match state {
                State::Normal | State::PartialOpen => (None, Some(buffered)),
                State::InThinking | State::PartialClose => (Some(buffered), None),
            }
        }
    }

    fn append(target: &mut Option<String>, s: &str) {
        match target {
            Some(existing) => existing.push_str(s),
            None => *target = Some(s.to_string()),
        }
    }
}

#[cfg(feature = "gemma4")]
mod channel_thought {
    use super::delimiter::partial_prefix_at_end;

    const OPEN_DELIM: &str = "<|channel>thought\n";
    const CLOSE_DELIM: &str = "<channel|>";

    #[derive(Debug)]
    pub(super) struct ChannelThoughtParser {
        state: State,
        buffer: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        PartialOpen,
        InThinking,
        PartialClose,
    }

    impl ChannelThoughtParser {
        pub const fn new() -> Self {
            Self {
                state: State::Normal,
                buffer: String::new(),
            }
        }

        pub fn process(&mut self, content: &str) -> (Option<String>, Option<String>) {
            self.buffer.push_str(content);

            let mut thinking_out: Option<String> = None;
            let mut text_out: Option<String> = None;

            loop {
                match self.state {
                    State::Normal => {
                        if let Some(pos) = self.buffer.find("<|channel>thought\n") {
                            let before = &self.buffer[..pos];
                            if !before.is_empty() {
                                append(&mut text_out, before);
                            }
                            let rest = self.buffer[pos + OPEN_DELIM.len()..].to_string();
                            self.buffer = rest;
                            self.state = State::InThinking;
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
                                self.state = State::InThinking;
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
                            self.state = State::InThinking;
                            continue;
                        }
                        if CLOSE_DELIM.starts_with(&self.buffer) {
                            break;
                        }
                        self.state = State::InThinking;
                    }
                }
            }

            (thinking_out, text_out)
        }
    }

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
    use super::delimiter::partial_prefix_at_end;

    const OPEN_DELIM: &str = "<|tool_call>call:";
    const CLOSE_DELIM: &str = "<tool_call|>";

    pub(super) struct ParsedToolCall {
        pub name: String,
        pub args: String,
    }

    #[derive(Debug)]
    pub(super) struct ToolCallParser {
        state: State,
        buffer: String,
        name_buf: String,
        args_buf: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        PartialOpen,
        InName,
        InArgs,
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

struct StreamState {
    events: Vec<AssistantMessageEvent>,
    blocks: BlockAccumulator,
    prompt_tokens: u32,
    completion_tokens: u32,
    has_tool_calls: bool,
    finish_reason: FinishReason,
    saw_done: bool,
    think_parser: think_tags::ThinkTagParser,
    #[cfg(feature = "gemma4")]
    channel_parser: Option<channel_thought::ChannelThoughtParser>,
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
            prompt_tokens: 0,
            completion_tokens: 0,
            has_tool_calls: false,
            finish_reason: FinishReason::Stop,
            saw_done: false,
            think_parser: think_tags::ThinkTagParser::new(),
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

    fn process_token(&mut self, content: &str) {
        // Step 1: Extract thinking blocks.
        #[cfg(feature = "gemma4")]
        let (thinking_part, text_part) = self.channel_parser.as_mut().map_or_else(
            || self.think_parser.process(content),
            |parser| parser.process(content),
        );
        #[cfg(not(feature = "gemma4"))]
        let (thinking_part, text_part) = self.think_parser.process(content);

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

        if let Some(text) = final_text
            && !text.is_empty()
        {
            self.events.extend(self.blocks.close_thinking(None));
            self.events.extend(self.blocks.ensure_text_open());
            self.events.extend(self.blocks.text_delta(text));
        }
    }

    #[cfg(feature = "gemma4")]
    fn emit_gemma4_tool_call(&mut self, call: tool_call::ParsedToolCall) {
        self.events.extend(self.blocks.close_text());
        self.events.extend(self.blocks.close_thinking(None));

        let id = Uuid::new_v4().to_string();
        self.has_tool_calls = true;
        let (tc_content_index, start_ev) = self.blocks.open_tool_call(id.clone(), call.name);
        self.events.push(start_ev);

        if !call.args.is_empty() {
            self.events.push(BlockAccumulator::tool_call_delta(
                tc_content_index,
                call.args,
            ));
        }
    }

    fn flush_pending_non_gemma_thinking(&mut self) {
        let (thinking_part, text_part) = self.think_parser.finish();

        if let Some(think) = thinking_part
            && !think.is_empty()
        {
            self.events.extend(self.blocks.ensure_thinking_open());
            self.events.extend(self.blocks.thinking_delta(think));
        }

        if let Some(text) = text_part
            && !text.is_empty()
        {
            self.events.extend(self.blocks.close_thinking(None));
            self.events.extend(self.blocks.ensure_text_open());
            self.events.extend(self.blocks.text_delta(text));
        }
    }

    fn finalize(mut self) -> Vec<AssistantMessageEvent> {
        self.flush_pending_non_gemma_thinking();
        self.events.extend(finalize_blocks(&mut self.blocks));

        let stop_reason = match self.finish_reason {
            FinishReason::Length => StopReason::Length,
            FinishReason::Stop if self.has_tool_calls => StopReason::ToolUse,
            FinishReason::Stop => StopReason::Stop,
        };

        self.events.push(AssistantMessageEvent::Done {
            stop_reason,
            usage: Usage {
                input: u64::from(self.prompt_tokens),
                output: u64::from(self.completion_tokens),
                total: u64::from(self.prompt_tokens + self.completion_tokens),
                ..Default::default()
            },
            cost: Cost::default(),
        });

        self.events
    }

    fn finalize_error(mut self, message: impl Into<String>) -> Vec<AssistantMessageEvent> {
        self.flush_pending_non_gemma_thinking();
        self.events.extend(finalize_blocks(&mut self.blocks));
        self.events
            .push(AssistantMessageEvent::error(message.into()));
        self.events
    }

    fn finalize_cancelled(self) -> Vec<AssistantMessageEvent> {
        self.finalize_error("local inference cancelled")
    }

    fn finalize_eof_without_done(self) -> Vec<AssistantMessageEvent> {
        self.finalize_error("local inference stream ended before completion")
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn local_stream<'a>(
    local_model: &'a LocalModel,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
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

        #[cfg(feature = "gemma4")]
        let is_gemma4 = local_model.config().is_gemma4();
        #[cfg(not(feature = "gemma4"))]
        let is_gemma4 = false;

        let thinking_enabled = model
            .capabilities
            .as_ref()
            .is_some_and(|c| c.supports_thinking);

        let local_messages = crate::convert::convert_context_messages(
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

        // Build prompt string from messages.
        // Gemma 4: format manually (GGUF Jinja template is too complex for llama.cpp's engine).
        // Other models: use the model's built-in chat template via apply_chat_template.
        #[cfg(feature = "gemma4")]
        let use_manual_format = is_gemma4;
        #[cfg(not(feature = "gemma4"))]
        let use_manual_format = false;

        let prompt = if use_manual_format {
            #[cfg(feature = "gemma4")]
            {
                let p = crate::convert::format_gemma4_prompt(&local_messages);
                debug!(prompt_len = p.len(), "gemma4 prompt formatted manually");
                p
            }
            #[cfg(not(feature = "gemma4"))]
            unreachable!()
        } else {
            let chat_messages: Vec<LlamaChatMessage> = local_messages
                .into_iter()
                .filter_map(|m| {
                    match LlamaChatMessage::new(m.role.clone(), m.content) {
                        Ok(msg) => Some(msg),
                        Err(e) => {
                            warn!(role = %m.role, error = %e, "failed to create chat message, skipping");
                            None
                        }
                    }
                })
                .collect();

            debug!(chat_message_count = chat_messages.len(), "built chat messages");

            match runner.apply_chat_template(&chat_messages, true) {
                Ok(p) => {
                    debug!(prompt_len = p.len(), "chat template applied");
                    p
                }
                Err(e) => {
                    error!(error = %e, "chat template application failed");
                    return stream::iter(vec![
                        AssistantMessageEvent::Start,
                        AssistantMessageEvent::error(format!("chat template error: {e}")),
                    ]);
                }
            }
        };

        let tokens = match runner.tokenize(&prompt) {
            Ok(t) => t,
            Err(e) => {
                error!(error = %e, "tokenization failed");
                return stream::iter(vec![
                    AssistantMessageEvent::Start,
                    AssistantMessageEvent::error(format!("tokenization error: {e}")),
                ]);
            }
        };

        debug!(token_count = tokens.len(), "prompt tokenized");

        let mut rx = runner.generate_stream(
            tokens,
            generation_options_from_stream_options(options),
            cancellation_token.clone(),
        );

        // Release the state guard before consuming the stream — the runner
        // Arc keeps the model alive independently.
        drop(state_guard);

        let mut state = StreamState::new(is_gemma4);
        while let Some(event) = rx.recv().await {
            if cancellation_token.is_cancelled() {
                return stream::iter(state.finalize_cancelled());
            }
            match event {
                TokenEvent::Token(text) => state.process_token(&text),
                TokenEvent::Done {
                    prompt_tokens,
                    completion_tokens,
                    finish_reason,
                } => {
                    state.prompt_tokens = prompt_tokens;
                    state.completion_tokens = completion_tokens;
                    state.finish_reason = finish_reason;
                    state.saw_done = true;
                    break;
                }
                TokenEvent::Error(msg) => {
                    error!(error = %msg, "error during local streaming");
                    return stream::iter(
                        state.finalize_error(format!("local inference error: {msg}"))
                    );
                }
            }
        }

        if state.saw_done {
            stream::iter(state.finalize())
        } else {
            warn!("local stream ended without Done; emitting terminal error");
            stream::iter(state.finalize_eof_without_done())
        }
    })
    .flatten()
}

// ─── Helpers ────────────────────────────────────────────────────────────────

#[allow(clippy::cast_possible_truncation)]
fn generation_options_from_stream_options(options: &StreamOptions) -> GenerateOptions {
    GenerateOptions {
        temperature: options.temperature.map(|value| value as f32),
        max_tokens: options
            .max_tokens
            .map(|value| u32::try_from(value).unwrap_or(u32::MAX)),
    }
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LocalStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::delimiter::partial_prefix_at_end;
    use super::think_tags::ThinkTagParser;
    use super::*;

    #[test]
    fn think_tag_single_chunk() {
        let mut parser = ThinkTagParser::new();
        let (thinking, text) =
            parser.process("<think>I need to reason about this.</think>The answer is 42.");
        assert_eq!(thinking.as_deref(), Some("I need to reason about this."));
        assert_eq!(text.as_deref(), Some("The answer is 42."));
    }

    #[test]
    fn think_tag_no_tags() {
        let mut parser = ThinkTagParser::new();
        let (thinking, text) = parser.process("Hello, world!");
        assert!(thinking.is_none());
        assert_eq!(text.as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn think_tag_empty_tags() {
        let mut parser = ThinkTagParser::new();
        let (thinking, text) = parser.process("<think></think>Just text.");
        assert!(thinking.is_none());
        assert_eq!(text.as_deref(), Some("Just text."));
    }

    #[test]
    fn think_tag_with_content_before() {
        let mut parser = ThinkTagParser::new();
        let (thinking, text) = parser.process("Before <think>reasoning</think> after");
        assert_eq!(thinking.as_deref(), Some("reasoning"));
        assert_eq!(text.as_deref(), Some("Before  after"));
    }

    #[test]
    fn think_tag_cross_chunk_open_and_close() {
        let mut parser = ThinkTagParser::new();
        let (t1, txt1) = parser.process("<th");
        assert!(t1.is_none());
        assert!(txt1.is_none());

        let (t2, txt2) = parser.process("ink>reason");
        assert_eq!(t2.as_deref(), Some("reason"));
        assert!(txt2.is_none());

        let (t3, txt3) = parser.process("ing</th");
        assert_eq!(t3.as_deref(), Some("ing"));
        assert!(txt3.is_none());

        let (t4, txt4) = parser.process("ink> after");
        assert!(t4.is_none());
        assert_eq!(txt4.as_deref(), Some(" after"));
    }

    #[test]
    fn think_tag_cross_chunk_with_text_before_open() {
        let mut parser = ThinkTagParser::new();
        let (t1, txt1) = parser.process("Before <thi");
        assert!(t1.is_none());
        assert_eq!(txt1.as_deref(), Some("Before "));

        let (t2, txt2) = parser.process("nk>reasoning</think> after");
        assert_eq!(t2.as_deref(), Some("reasoning"));
        assert_eq!(txt2.as_deref(), Some(" after"));
    }

    #[test]
    fn think_tag_partial_match_is_utf8_safe() {
        let haystack = "alpha🙂<thi";
        assert_eq!(partial_prefix_at_end(haystack, "<think>"), Some(4));

        let mut parser = ThinkTagParser::new();
        let (t1, txt1) = parser.process("alpha🙂<thi");
        assert!(t1.is_none());
        assert_eq!(txt1.as_deref(), Some("alpha🙂"));

        let (t2, txt2) = parser.process("nk>reasoning</think>");
        assert_eq!(t2.as_deref(), Some("reasoning"));
        assert!(txt2.is_none());
    }

    #[test]
    fn finalize_cancelled_emits_error_terminal() {
        let mut state = StreamState::new(false);
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
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. }))
        );
    }

    #[test]
    fn finalize_error_closes_open_blocks_before_terminal_error() {
        let mut state = StreamState::new(false);
        state.events.extend(state.blocks.ensure_text_open());
        state
            .events
            .extend(state.blocks.text_delta("partial".to_string()));

        let events = state.finalize_error("local inference error: runner crashed");
        let terminal_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
            .expect("terminal error event");
        let text_end_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::TextEnd { .. }))
            .expect("text end event");

        assert!(
            text_end_index < terminal_index,
            "open blocks must be finalized before the terminal error: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
            "terminal error path must not emit Done"
        );
    }

    #[test]
    fn finalize_flushes_pending_partial_open_as_text() {
        let mut state = StreamState::new(false);
        state.process_token("Before <thi");

        let events = state.finalize();
        let text_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["Before ", "<thi"]);
    }

    #[test]
    fn finalize_flushes_unclosed_thinking_buffer() {
        let mut state = StreamState::new(false);
        state.process_token("<think>reasoning");

        let events = state.finalize();
        let thinking_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thinking_deltas, vec!["reasoning"]);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::ThinkingEnd { .. }))
        );
    }

    #[test]
    fn finalize_keeps_tool_use_stop_reason() {
        let mut state = StreamState::new(false);
        state.has_tool_calls = true;
        state.finish_reason = FinishReason::Stop;

        let events = state.finalize();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[test]
    fn finalize_preserves_length_stop_reason_over_tool_use() {
        let mut state = StreamState::new(false);
        state.has_tool_calls = true;
        state.finish_reason = FinishReason::Length;

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
    fn finalize_eof_without_done_emits_error_terminal() {
        let mut state = StreamState::new(false);
        let start = state.blocks.ensure_text_open();
        state.events.extend(start);
        state
            .events
            .extend(state.blocks.text_delta("partial".to_string()));

        assert!(!state.saw_done);

        let events = state.finalize_eof_without_done();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(
                    error_message.contains("ended before completion"),
                    "expected EOF error message, got: {error_message}"
                );
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
        );
    }

    #[test]
    fn usage_tracking() {
        let mut state = StreamState::new(false);
        state.prompt_tokens = 42;
        state.completion_tokens = 13;
        state.saw_done = true;

        let events = state.finalize();
        let terminal = events.last().expect("at least one event");
        match terminal {
            AssistantMessageEvent::Done { usage, .. } => {
                assert_eq!(usage.input, 42);
                assert_eq!(usage.output, 13);
                assert_eq!(usage.total, 55);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[test]
    fn stream_options_forward_generation_overrides() {
        let options = StreamOptions {
            temperature: Some(0.8),
            max_tokens: Some(256),
            ..StreamOptions::default()
        };

        assert_eq!(
            generation_options_from_stream_options(&options),
            GenerateOptions {
                temperature: Some(0.8_f32),
                max_tokens: Some(256),
            }
        );
    }

    #[test]
    fn stream_options_max_tokens_saturates_to_u32() {
        let options = StreamOptions {
            max_tokens: Some(u64::MAX),
            ..StreamOptions::default()
        };

        assert_eq!(
            generation_options_from_stream_options(&options).max_tokens,
            Some(u32::MAX)
        );
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::super::channel_thought::ChannelThoughtParser;
        use super::super::delimiter::partial_prefix_at_end;

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
            let (t1, txt1) = parser.process("<|channel>");
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
            let mut parser = ChannelThoughtParser::new();
            let (thinking, text) = parser.process("The format is <|channel>thought end");
            assert!(thinking.is_none());
            assert!(text.is_some());
        }
    }
}
