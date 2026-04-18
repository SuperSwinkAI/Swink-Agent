//! `tiktoken`-backed token counting for context management.

use std::error::Error;
use std::fmt;

use crate::context::TokenCounter;
use crate::types::{AgentMessage, ContentBlock, LlmMessage};

/// Error returned when building a [`TiktokenCounter`] tokenizer.
#[cfg_attr(docsrs, doc(cfg(feature = "tiktoken")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiktokenError {
    message: String,
}

impl TiktokenError {
    /// Create a new error with a caller-provided message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TiktokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for TiktokenError {}

/// [`TokenCounter`] implementation backed by `tiktoken-rs`.
///
/// This counts each text-bearing content block with the selected tokenizer
/// instead of the default `chars / 4` heuristic. `CustomMessage` values stay at
/// the existing flat 100-token estimate because they never reach the provider.
#[cfg_attr(docsrs, doc(cfg(feature = "tiktoken")))]
pub struct TiktokenCounter {
    bpe: tiktoken_rs::CoreBPE,
}

impl TiktokenCounter {
    /// Build a counter from an existing `tiktoken-rs` tokenizer.
    #[must_use]
    pub const fn new(bpe: tiktoken_rs::CoreBPE) -> Self {
        Self { bpe }
    }

    /// Build a counter from the tokenizer mapped to a model name.
    pub fn from_model(model: &str) -> Result<Self, TiktokenError> {
        tiktoken_rs::get_bpe_from_model(model)
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    /// Build a counter using the `cl100k_base` tokenizer.
    pub fn cl100k() -> Result<Self, TiktokenError> {
        tiktoken_rs::cl100k_base()
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    /// Build a counter using the `o200k_base` tokenizer.
    pub fn o200k() -> Result<Self, TiktokenError> {
        tiktoken_rs::o200k_base()
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    /// Build a counter using the `p50k_base` tokenizer.
    pub fn p50k_base() -> Result<Self, TiktokenError> {
        tiktoken_rs::p50k_base()
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    /// Build a counter using the `p50k_edit` tokenizer.
    pub fn p50k_edit() -> Result<Self, TiktokenError> {
        tiktoken_rs::p50k_edit()
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    /// Build a counter using the `r50k_base` tokenizer.
    pub fn r50k_base() -> Result<Self, TiktokenError> {
        tiktoken_rs::r50k_base()
            .map(Self::new)
            .map_err(|err| TiktokenError::new(err.to_string()))
    }

    fn count_text(&self, text: &str) -> usize {
        self.bpe.encode_with_special_tokens(text).len()
    }
}

impl TokenCounter for TiktokenCounter {
    fn count_tokens(&self, message: &AgentMessage) -> usize {
        match message {
            AgentMessage::Llm(llm) => content_blocks(llm)
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => self.count_text(text),
                    ContentBlock::Thinking { thinking, .. } => self.count_text(thinking),
                    ContentBlock::ToolCall {
                        name, arguments, ..
                    } => self.count_text(name) + self.count_text(&arguments.to_string()),
                    ContentBlock::Image { .. } => 0,
                    ContentBlock::Extension { type_name, data } => {
                        self.count_text(type_name) + self.count_text(&data.to_string())
                    }
                })
                .sum(),
            AgentMessage::Custom(_) => 100,
        }
    }
}

impl From<tiktoken_rs::CoreBPE> for TiktokenCounter {
    fn from(bpe: tiktoken_rs::CoreBPE) -> Self {
        Self::new(bpe)
    }
}

fn content_blocks(msg: &LlmMessage) -> &[ContentBlock] {
    match msg {
        LlmMessage::User(m) => &m.content,
        LlmMessage::Assistant(m) => &m.content,
        LlmMessage::ToolResult(m) => &m.content,
    }
}
