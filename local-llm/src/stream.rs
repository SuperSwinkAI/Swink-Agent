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
use uuid::Uuid;

use swink_agent::stream_assembly::{BlockAccumulator, finalize_blocks};
use swink_agent::{
    AgentContext, AssistantMessageEvent, Cost, ModelSpec, StopReason, StreamFn, StreamOptions,
    ThinkingLevel, Usage,
};

use crate::error::LocalModelError;
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

    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum ToolCallFinish {
        Text(String),
        PartialToolCall { name: String, args: String },
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

        pub fn finish(&mut self, preserve_partial_tool_call: bool) -> Option<ToolCallFinish> {
            if self.buffer.is_empty() && self.name_buf.is_empty() && self.args_buf.is_empty() {
                self.state = State::Normal;
                return None;
            }

            if preserve_partial_tool_call
                && matches!(self.state, State::InArgs | State::PartialClose)
                && !self.name_buf.trim().is_empty()
            {
                let result = ToolCallFinish::PartialToolCall {
                    name: self.name_buf.trim().to_string(),
                    args: self.args_buf.clone(),
                };
                self.reset();
                return Some(result);
            }

            let mut text = String::new();
            match self.state {
                State::Normal | State::PartialOpen => text.push_str(&self.buffer),
                State::InName => {
                    text.push_str(OPEN_DELIM);
                    text.push_str(&self.name_buf);
                    text.push_str(&self.buffer);
                }
                State::InArgs | State::PartialClose => {
                    text.push_str(OPEN_DELIM);
                    text.push_str(&self.name_buf);
                    text.push_str(&self.args_buf);
                    text.push_str(&self.buffer);
                }
            }

            self.reset();
            (!text.is_empty()).then_some(ToolCallFinish::Text(text))
        }

        fn reset(&mut self) {
            self.buffer.clear();
            self.name_buf.clear();
            self.args_buf.clear();
            self.state = State::Normal;
        }
    }

    fn append(target: &mut Option<String>, s: &str) {
        match target {
            Some(existing) => existing.push_str(s),
            None => *target = Some(s.to_string()),
        }
    }
}

// ─── Default ToolCallParser ────────────────────────────────────────────────

mod default_tool_call {
    use super::delimiter::partial_prefix_at_end;

    const OPEN_DELIM: &str = "call:";

    pub(super) struct ParsedToolCall {
        pub name: String,
        pub args: String,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum ToolCallFinish {
        Text(String),
        PartialToolCall { name: String, args: String },
    }

    #[derive(Debug)]
    pub(super) struct ToolCallParser {
        state: State,
        buffer: String,
        name_buf: String,
        args_buf: String,
        json_depth: usize,
        in_string: bool,
        escape: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        PartialOpen,
        InName,
        InArgs,
    }

    impl ToolCallParser {
        pub const fn new() -> Self {
            Self {
                state: State::Normal,
                buffer: String::new(),
                name_buf: String::new(),
                args_buf: String::new(),
                json_depth: 0,
                in_string: false,
                escape: false,
            }
        }

        #[allow(clippy::too_many_lines)]
        pub fn process(&mut self, content: &str) -> (Vec<ParsedToolCall>, Option<String>) {
            self.buffer.push_str(content);

            let mut calls = Vec::new();
            let mut text_out = None;

            loop {
                match self.state {
                    State::Normal => {
                        if let Some(pos) = self.buffer.find(OPEN_DELIM) {
                            let before = &self.buffer[..pos];
                            if !before.is_empty() {
                                append(&mut text_out, before);
                            }
                            self.buffer = self.buffer[pos + OPEN_DELIM.len()..].to_string();
                            self.name_buf.clear();
                            self.args_buf.clear();
                            self.reset_json_state();
                            self.state = State::InName;
                            continue;
                        }
                        if let Some(partial_len) = partial_prefix_at_end(&self.buffer, OPEN_DELIM) {
                            let flush_end = self.buffer.len() - partial_len;
                            let flush = &self.buffer[..flush_end];
                            if !flush.is_empty() {
                                append(&mut text_out, flush);
                            }
                            self.buffer = self.buffer[flush_end..].to_string();
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
                                self.buffer = self.buffer[OPEN_DELIM.len()..].to_string();
                                self.name_buf.clear();
                                self.args_buf.clear();
                                self.reset_json_state();
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
                            let candidate = &self.buffer[..pos];
                            if !candidate.chars().all(is_tool_name_char) {
                                self.flush_invalid_call(&mut text_out);
                                self.state = State::Normal;
                                continue;
                            }
                            self.name_buf.push_str(candidate);
                            if self.name_buf.is_empty() {
                                self.flush_invalid_call(&mut text_out);
                                self.state = State::Normal;
                                continue;
                            }
                            self.buffer = self.buffer[pos..].to_string();
                            self.state = State::InArgs;
                            continue;
                        }

                        let valid_prefix_len = self
                            .buffer
                            .char_indices()
                            .find_map(|(idx, ch)| (!is_tool_name_char(ch)).then_some(idx))
                            .unwrap_or(self.buffer.len());
                        if valid_prefix_len < self.buffer.len() {
                            self.name_buf.push_str(&self.buffer[..valid_prefix_len]);
                            self.flush_invalid_call(&mut text_out);
                            self.buffer = self.buffer[valid_prefix_len..].to_string();
                            self.state = State::Normal;
                            continue;
                        }

                        self.name_buf.push_str(&self.buffer);
                        self.buffer.clear();
                        break;
                    }
                    State::InArgs => {
                        if let Some(end_idx) = self.consume_args_buffer() {
                            calls.push(ParsedToolCall {
                                name: self.name_buf.trim().to_string(),
                                args: self.args_buf.clone(),
                            });
                            self.buffer = self.buffer[end_idx..].to_string();
                            self.name_buf.clear();
                            self.args_buf.clear();
                            self.reset_json_state();
                            self.state = State::Normal;
                            continue;
                        }
                        self.buffer.clear();
                        break;
                    }
                }
            }

            (calls, text_out)
        }

        pub fn finish(&mut self, preserve_partial_tool_call: bool) -> Option<ToolCallFinish> {
            if self.buffer.is_empty() && self.name_buf.is_empty() && self.args_buf.is_empty() {
                self.state = State::Normal;
                return None;
            }

            if preserve_partial_tool_call
                && matches!(self.state, State::InArgs)
                && !self.name_buf.trim().is_empty()
            {
                let result = ToolCallFinish::PartialToolCall {
                    name: self.name_buf.trim().to_string(),
                    args: self.args_buf.clone(),
                };
                self.reset();
                return Some(result);
            }

            let mut text = String::new();
            match self.state {
                State::Normal | State::PartialOpen => text.push_str(&self.buffer),
                State::InName => {
                    text.push_str(OPEN_DELIM);
                    text.push_str(&self.name_buf);
                    text.push_str(&self.buffer);
                }
                State::InArgs => {
                    text.push_str(OPEN_DELIM);
                    text.push_str(&self.name_buf);
                    text.push_str(&self.args_buf);
                    text.push_str(&self.buffer);
                }
            }

            self.reset();
            (!text.is_empty()).then_some(ToolCallFinish::Text(text))
        }

        fn reset(&mut self) {
            self.buffer.clear();
            self.name_buf.clear();
            self.args_buf.clear();
            self.reset_json_state();
            self.state = State::Normal;
        }

        fn consume_args_buffer(&mut self) -> Option<usize> {
            for (idx, ch) in self.buffer.char_indices() {
                self.args_buf.push(ch);

                if self.escape {
                    self.escape = false;
                    continue;
                }

                if self.in_string {
                    match ch {
                        '\\' => self.escape = true,
                        '"' => self.in_string = false,
                        _ => {}
                    }
                    continue;
                }

                match ch {
                    '"' => self.in_string = true,
                    '{' => self.json_depth += 1,
                    '}' => {
                        self.json_depth = self.json_depth.saturating_sub(1);
                        if self.json_depth == 0 {
                            return Some(idx + ch.len_utf8());
                        }
                    }
                    _ => {}
                }
            }

            None
        }

        fn flush_invalid_call(&mut self, text_out: &mut Option<String>) {
            append(text_out, OPEN_DELIM);
            append(text_out, &self.name_buf);
            self.name_buf.clear();
            self.args_buf.clear();
            self.reset_json_state();
        }

        fn reset_json_state(&mut self) {
            self.json_depth = 0;
            self.in_string = false;
            self.escape = false;
        }
    }

    fn is_tool_name_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
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
    default_tool_call_parser: default_tool_call::ToolCallParser,
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
            default_tool_call_parser: default_tool_call::ToolCallParser::new(),
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

        // Step 2: Pass text through the model-family tool call parser.
        #[cfg(feature = "gemma4")]
        let final_text = if let Some(text) = text_part {
            if let Some(parser) = self.tool_call_parser.as_mut() {
                let (calls, remaining) = parser.process(&text);
                for call in calls {
                    self.emit_tool_call(call.name, call.args);
                }
                remaining
            } else {
                self.process_default_tool_call_text(&text)
            }
        } else {
            None
        };
        #[cfg(not(feature = "gemma4"))]
        let final_text = text_part
            .as_deref()
            .and_then(|text| self.process_default_tool_call_text(text));

        if let Some(text) = final_text
            && !text.is_empty()
        {
            self.events.extend(self.blocks.close_thinking(None));
            self.events.extend(self.blocks.ensure_text_open());
            self.events.extend(self.blocks.text_delta(text));
        }
    }

    fn process_default_tool_call_text(&mut self, text: &str) -> Option<String> {
        let (calls, remaining) = self.default_tool_call_parser.process(text);
        for call in calls {
            self.emit_tool_call(call.name, call.args);
        }
        remaining
    }

    fn emit_tool_call(&mut self, name: String, args: String) {
        self.events.extend(self.blocks.close_text());
        self.events.extend(self.blocks.close_thinking(None));

        let id = Uuid::new_v4().to_string();
        self.has_tool_calls = true;
        let (tc_content_index, start_ev) = self.blocks.open_tool_call(id, name);
        self.events.push(start_ev);

        if !args.is_empty() {
            self.events
                .push(BlockAccumulator::tool_call_delta(tc_content_index, args));
        }
    }

    fn flush_pending_parsers(&mut self, preserve_partial_tool_call: bool) {
        #[cfg(feature = "gemma4")]
        if let Some(parser) = self.channel_parser.as_mut() {
            let (thinking_part, text_part) = parser.finish();
            self.emit_thinking(thinking_part);

            if let Some(text) = text_part
                && !text.is_empty()
            {
                self.process_gemma_tool_call_text(&text);
            }

            self.flush_pending_gemma_tool_call(preserve_partial_tool_call);
            return;
        }

        let (thinking_part, text_part) = self.think_parser.finish();
        self.emit_thinking(thinking_part);

        if let Some(text) = text_part
            && !text.is_empty()
        {
            let final_text = self.process_default_tool_call_text(&text);
            self.emit_text(final_text);
        }

        self.flush_pending_default_tool_call(preserve_partial_tool_call);
    }

    fn emit_thinking(&mut self, thinking_part: Option<String>) {
        if let Some(think) = thinking_part
            && !think.is_empty()
        {
            self.events.extend(self.blocks.ensure_thinking_open());
            self.events.extend(self.blocks.thinking_delta(think));
        }
    }

    #[cfg(feature = "gemma4")]
    fn process_gemma_tool_call_text(&mut self, text: &str) {
        if let Some(parser) = self.tool_call_parser.as_mut() {
            let (calls, remaining) = parser.process(text);
            for call in calls {
                self.emit_tool_call(call.name, call.args);
            }
            self.emit_text(remaining);
        }
    }

    #[cfg(feature = "gemma4")]
    fn flush_pending_gemma_tool_call(&mut self, preserve_partial_tool_call: bool) {
        if let Some(parser) = self.tool_call_parser.as_mut()
            && let Some(finished) = parser.finish(preserve_partial_tool_call)
        {
            match finished {
                tool_call::ToolCallFinish::Text(text) => self.emit_text(Some(text)),
                tool_call::ToolCallFinish::PartialToolCall { name, args } => {
                    self.emit_tool_call(name, args);
                }
            }
        }
    }

    fn flush_pending_default_tool_call(&mut self, preserve_partial_tool_call: bool) {
        if let Some(finished) = self
            .default_tool_call_parser
            .finish(preserve_partial_tool_call)
        {
            match finished {
                default_tool_call::ToolCallFinish::Text(text) => self.emit_text(Some(text)),
                default_tool_call::ToolCallFinish::PartialToolCall { name, args } => {
                    self.emit_tool_call(name, args);
                }
            }
        }
    }

    fn emit_text(&mut self, text: Option<String>) {
        if let Some(text) = text
            && !text.is_empty()
        {
            self.events.extend(self.blocks.close_thinking(None));
            self.events.extend(self.blocks.ensure_text_open());
            self.events.extend(self.blocks.text_delta(text));
        }
    }

    fn finalize(mut self) -> Vec<AssistantMessageEvent> {
        let preserve_partial_tool_call = self.finish_reason == FinishReason::Length;
        self.flush_pending_parsers(preserve_partial_tool_call);
        self.events.extend(finalize_blocks(&mut self.blocks));

        let stop_reason = match self.finish_reason {
            FinishReason::Length => StopReason::Length,
            FinishReason::Stop if self.has_tool_calls => StopReason::ToolUse,
            FinishReason::Stop => StopReason::Stop,
        };

        self.events.push(AssistantMessageEvent::Done {
            stop_reason,
            usage: Usage::default()
                .with_input(u64::from(self.prompt_tokens))
                .with_output(u64::from(self.completion_tokens))
                .with_total(u64::from(self.prompt_tokens + self.completion_tokens)),
            cost: Cost::default(),
        });

        self.events
    }

    fn finalize_error(mut self, message: impl Into<String>) -> Vec<AssistantMessageEvent> {
        self.flush_pending_parsers(false);
        self.events.extend(finalize_blocks(&mut self.blocks));
        self.events
            .push(AssistantMessageEvent::error(message.into()));
        self.events
    }

    fn finalize_cancelled(mut self) -> Vec<AssistantMessageEvent> {
        self.flush_pending_parsers(false);
        self.events.extend(finalize_blocks(&mut self.blocks));
        self.events
            .push(cancelled_event("local inference cancelled"));
        self.events
    }

    fn finalize_eof_without_done(self) -> Vec<AssistantMessageEvent> {
        self.finalize_error("local inference stream ended before completion")
    }
}

fn cancelled_event(message: impl Into<String>) -> AssistantMessageEvent {
    AssistantMessageEvent::Error {
        stop_reason: StopReason::Aborted,
        error_message: message.into(),
        usage: None,
        error_kind: None,
        retry_after: None,
    }
}

async fn race_pre_stream_cancellation<T, F>(
    cancellation_token: &CancellationToken,
    operation: F,
) -> Result<T, AssistantMessageEvent>
where
    F: std::future::Future<Output = Result<T, AssistantMessageEvent>>,
{
    if cancellation_token.is_cancelled() {
        return Err(cancelled_event("local inference cancelled"));
    }

    tokio::select! {
        () = cancellation_token.cancelled() => Err(cancelled_event("local inference cancelled")),
        result = operation => result,
    }
}

fn thinking_enabled_for_model(model: &ModelSpec) -> bool {
    model.thinking_level != ThinkingLevel::Off
        && model
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.supports_thinking)
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
        if let Err(event) = race_pre_stream_cancellation(&cancellation_token, async {
            local_model
                .ensure_ready()
                .await
                .map_err(|e| AssistantMessageEvent::error(format!("local model not ready: {e}")))
        })
        .await
        {
            return stream::iter(vec![AssistantMessageEvent::Start, event]);
        }

        #[cfg(feature = "gemma4")]
        let is_gemma4 = local_model.config().is_gemma4();
        #[cfg(not(feature = "gemma4"))]
        let is_gemma4 = false;

        let thinking_enabled = thinking_enabled_for_model(model);

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

        let max_prompt_tokens = prompt_token_budget(local_model.config().context_length);
        let local_messages =
            match truncate_messages_to_context(local_messages, max_prompt_tokens, |messages| {
                let prompt = build_prompt(runner, messages, use_manual_format)?;
                runner.tokenize(&prompt).map(|tokens| tokens.len())
            }) {
                Ok(messages) => messages,
                Err(e) => {
                    error!(error = %e, "prompt truncation failed");
                    return stream::iter(vec![
                        AssistantMessageEvent::Start,
                        AssistantMessageEvent::error(format!("prompt truncation error: {e}")),
                    ]);
                }
            };

        let prompt = match build_prompt(runner, &local_messages, use_manual_format) {
            Ok(prompt) => prompt,
            Err(e) => {
                error!(error = %e, "chat template application failed");
                return stream::iter(vec![
                    AssistantMessageEvent::Start,
                    AssistantMessageEvent::error(format!("chat template error: {e}")),
                ]);
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

        let rx = runner.generate_stream(
            tokens,
            generation_options_from_stream_options(options),
            cancellation_token.clone(),
        );

        // Release the state guard before consuming the stream — the runner
        // Arc keeps the model alive independently.
        drop(state_guard);

        let events = drain_token_stream(rx, &cancellation_token, is_gemma4).await;
        stream::iter(events)
    })
    .flatten()
}

// ─── Context truncation ─────────────────────────────────────────────────────

fn build_prompt(
    runner: &crate::runner::LlamaRunner,
    messages: &[crate::convert::LocalMessage],
    use_manual_format: bool,
) -> Result<String, LocalModelError> {
    #[cfg(feature = "gemma4")]
    if use_manual_format {
        let prompt = crate::convert::format_gemma4_prompt(messages);
        debug!(
            prompt_len = prompt.len(),
            "gemma4 prompt formatted manually"
        );
        return Ok(prompt);
    }
    #[cfg(not(feature = "gemma4"))]
    let _ = use_manual_format;

    let chat_messages: Vec<LlamaChatMessage> = messages
        .iter()
        .filter_map(
            |m| match LlamaChatMessage::new(m.role.clone(), m.content.clone()) {
                Ok(msg) => Some(msg),
                Err(e) => {
                    warn!(role = %m.role, error = %e, "failed to create chat message, skipping");
                    None
                }
            },
        )
        .collect();

    debug!(
        chat_message_count = chat_messages.len(),
        "built chat messages"
    );
    let prompt = runner.apply_chat_template(&chat_messages, true)?;
    debug!(prompt_len = prompt.len(), "chat template applied");
    Ok(prompt)
}

fn prompt_token_budget(context_length: usize) -> usize {
    if context_length <= 1 {
        context_length
    } else {
        context_length - 1
    }
}

fn truncate_messages_to_context<E>(
    mut messages: Vec<crate::convert::LocalMessage>,
    max_prompt_tokens: usize,
    token_count: impl Fn(&[crate::convert::LocalMessage]) -> Result<usize, E>,
) -> Result<Vec<crate::convert::LocalMessage>, E> {
    loop {
        if token_count(&messages)? <= max_prompt_tokens {
            return Ok(messages);
        }

        let Some(drop_idx) = messages.iter().position(|m| m.role != "system") else {
            return Ok(messages);
        };

        messages.remove(drop_idx);
        drop_leading_tool_results(&mut messages);
    }
}

fn drop_leading_tool_results(messages: &mut Vec<crate::convert::LocalMessage>) {
    while let Some(idx) = messages.iter().position(|m| m.role != "system")
        && messages[idx].role == "tool"
    {
        messages.remove(idx);
    }
}

async fn drain_token_stream(
    mut rx: tokio::sync::mpsc::Receiver<TokenEvent>,
    cancellation_token: &CancellationToken,
    is_gemma4: bool,
) -> Vec<AssistantMessageEvent> {
    let mut state = StreamState::new(is_gemma4);
    loop {
        let event = tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                drop(rx);
                return state.finalize_cancelled();
            }
            event = rx.recv() => event,
        };

        let Some(event) = event else {
            break;
        };

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
                return state.finalize_error(format!("local inference error: {msg}"));
            }
        }
    }

    if state.saw_done {
        state.finalize()
    } else {
        warn!("local stream ended without Done; emitting terminal error");
        state.finalize_eof_without_done()
    }
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
#[path = "stream_tests.rs"]
mod tests;
