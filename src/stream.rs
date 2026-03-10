//! Streaming interface traits and types.
//!
//! Defines the `StreamFn` trait (the pluggable boundary between the harness and
//! LLM providers), the event protocol for incremental message delivery, and a
//! delta-accumulation function that reconstructs a finalized `AssistantMessage`
//! from a collected sequence of events.

use std::pin::Pin;
use std::time::SystemTime;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::types::{
    AgentContext, AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage,
};

// ─── StreamTransport ─────────────────────────────────────────────────────────

/// Transport protocol for streaming responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamTransport {
    /// Server-Sent Events (default).
    #[default]
    Sse,
}

// ─── StreamOptions ───────────────────────────────────────────────────────────

/// Per-call configuration passed through to the LLM provider.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// Sampling temperature (optional).
    pub temperature: Option<f64>,
    /// Output token limit (optional).
    pub max_tokens: Option<u64>,
    /// Provider-side session identifier for caching (optional).
    pub session_id: Option<String>,
    /// Preferred transport protocol.
    pub transport: StreamTransport,
}

// ─── AssistantMessageEvent ───────────────────────────────────────────────────

/// An incremental event emitted by a `StreamFn` implementation.
///
/// Events follow a strict start/delta/end protocol per content block. Each
/// block carries a `content_index` that identifies its position in the final
/// message's content vec.
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    /// The stream has opened.
    Start,

    /// A new text content block is starting at `content_index`.
    TextStart { content_index: usize },
    /// An incremental text fragment for the block at `content_index`.
    TextDelta { content_index: usize, delta: String },
    /// The text block at `content_index` is complete.
    TextEnd { content_index: usize },

    /// A new thinking content block is starting at `content_index`.
    ThinkingStart { content_index: usize },
    /// An incremental thinking fragment for the block at `content_index`.
    ThinkingDelta { content_index: usize, delta: String },
    /// The thinking block at `content_index` is complete, with an optional
    /// provider verification signature.
    ThinkingEnd {
        content_index: usize,
        signature: Option<String>,
    },

    /// A new tool call content block is starting at `content_index`.
    ToolCallStart {
        content_index: usize,
        id: String,
        name: String,
    },
    /// An incremental JSON argument fragment for the tool call at `content_index`.
    ToolCallDelta { content_index: usize, delta: String },
    /// The tool call at `content_index` is complete.
    ToolCallEnd { content_index: usize },

    /// The stream completed successfully.
    Done {
        stop_reason: StopReason,
        usage: Usage,
        cost: Cost,
    },

    /// The stream ended with an error.
    Error {
        stop_reason: StopReason,
        error_message: String,
        usage: Option<Usage>,
    },
}

// ─── AssistantMessageDelta ───────────────────────────────────────────────────

/// A typed incremental update during streaming, used in `MessageUpdate` events.
#[derive(Debug, Clone)]
pub enum AssistantMessageDelta {
    /// An appended text string fragment.
    Text { content_index: usize, delta: String },
    /// An appended reasoning fragment.
    Thinking { content_index: usize, delta: String },
    /// An appended JSON argument fragment for a tool call.
    ToolCall { content_index: usize, delta: String },
}

// ─── StreamFn Trait ──────────────────────────────────────────────────────────

/// The pluggable boundary between the harness and LLM providers.
///
/// Callers supply an implementation that accepts a model specification, an
/// agent context, and stream options, and returns an async stream of
/// `AssistantMessageEvent` values. The harness consumes this stream to build
/// up the assistant message incrementally.
///
/// This trait is object-safe and requires `Send + Sync` so that it can be
/// stored behind an `Arc` and shared across async tasks.
pub trait StreamFn: Send + Sync {
    /// Initiate a streaming LLM call.
    ///
    /// The returned stream yields `AssistantMessageEvent` values following the
    /// start/delta/end protocol. Implementations must respect the provided
    /// `cancellation_token` — when the token is cancelled, the stream should
    /// terminate promptly.
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}

// ─── Delta Accumulation ──────────────────────────────────────────────────────

/// Reconstruct a finalized `AssistantMessage` from a collected list of stream
/// events.
///
/// # Errors
///
/// Returns a descriptive error string if the event sequence is malformed (e.g.
/// delta for a non-existent content index, missing `Start` or terminal event).
#[allow(clippy::too_many_lines)]
pub fn accumulate_message(
    events: Vec<AssistantMessageEvent>,
    provider: &str,
    model_id: &str,
) -> Result<AssistantMessage, String> {
    let mut content: Option<Vec<ContentBlock>> = None;
    let mut stop_reason: Option<StopReason> = None;
    let mut usage: Option<Usage> = None;
    let mut cost: Option<Cost> = None;
    let mut error_message: Option<String> = None;
    let mut saw_start = false;

    for event in events {
        match event {
            AssistantMessageEvent::Start => {
                if saw_start {
                    return Err("duplicate Start event".into());
                }
                saw_start = true;
                content = Some(Vec::new());
            }

            AssistantMessageEvent::TextStart { content_index } => {
                let blocks = content.as_mut().ok_or("TextStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "TextStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::Text {
                    text: String::new(),
                });
            }

            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("TextDelta before Start")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("TextDelta: invalid content_index {content_index}"))?;
                match block {
                    ContentBlock::Text { text } => text.push_str(&delta),
                    _ => {
                        return Err(format!(
                            "TextDelta: block at index {content_index} is not Text"
                        ));
                    }
                }
            }

            AssistantMessageEvent::TextEnd { content_index } => {
                let blocks = content.as_ref().ok_or("TextEnd before Start")?;
                let block = blocks
                    .get(content_index)
                    .ok_or_else(|| format!("TextEnd: invalid content_index {content_index}"))?;
                if !matches!(block, ContentBlock::Text { .. }) {
                    return Err(format!(
                        "TextEnd: block at index {content_index} is not Text"
                    ));
                }
            }

            AssistantMessageEvent::ThinkingStart { content_index } => {
                let blocks = content.as_mut().ok_or("ThinkingStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "ThinkingStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                });
            }

            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ThinkingDelta before Start")?;
                let block = blocks.get_mut(content_index).ok_or_else(|| {
                    format!("ThinkingDelta: invalid content_index {content_index}")
                })?;
                match block {
                    ContentBlock::Thinking { thinking, .. } => thinking.push_str(&delta),
                    _ => {
                        return Err(format!(
                            "ThinkingDelta: block at index {content_index} is not Thinking"
                        ));
                    }
                }
            }

            AssistantMessageEvent::ThinkingEnd {
                content_index,
                signature,
            } => {
                let blocks = content.as_mut().ok_or("ThinkingEnd before Start")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("ThinkingEnd: invalid content_index {content_index}"))?;
                match block {
                    ContentBlock::Thinking { signature: sig, .. } => *sig = signature,
                    _ => {
                        return Err(format!(
                            "ThinkingEnd: block at index {content_index} is not Thinking"
                        ));
                    }
                }
            }

            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                let blocks = content.as_mut().ok_or("ToolCallStart before Start")?;
                if content_index != blocks.len() {
                    return Err(format!(
                        "ToolCallStart content_index {content_index} != content length {}",
                        blocks.len()
                    ));
                }
                blocks.push(ContentBlock::ToolCall {
                    id,
                    name,
                    arguments: Value::Null,
                    partial_json: Some(String::new()),
                });
            }

            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ToolCallDelta before Start")?;
                let block = blocks.get_mut(content_index).ok_or_else(|| {
                    format!("ToolCallDelta: invalid content_index {content_index}")
                })?;
                match block {
                    ContentBlock::ToolCall { partial_json, .. } => {
                        let pj = partial_json
                            .as_mut()
                            .ok_or("ToolCallDelta: partial_json already consumed")?;
                        pj.push_str(&delta);
                    }
                    _ => {
                        return Err(format!(
                            "ToolCallDelta: block at index {content_index} is not ToolCall"
                        ));
                    }
                }
            }

            AssistantMessageEvent::ToolCallEnd { content_index } => {
                let blocks = content.as_mut().ok_or("ToolCallEnd before Start")?;
                let block = blocks
                    .get_mut(content_index)
                    .ok_or_else(|| format!("ToolCallEnd: invalid content_index {content_index}"))?;
                match block {
                    ContentBlock::ToolCall {
                        arguments,
                        partial_json,
                        ..
                    } => {
                        let json_str = partial_json
                            .take()
                            .ok_or("ToolCallEnd: partial_json already consumed")?;
                        *arguments = if json_str.is_empty() {
                            Value::Object(serde_json::Map::new())
                        } else {
                            serde_json::from_str(&json_str).map_err(|e| {
                                format!("ToolCallEnd: failed to parse arguments JSON: {e}")
                            })?
                        };
                    }
                    _ => {
                        return Err(format!(
                            "ToolCallEnd: block at index {content_index} is not ToolCall"
                        ));
                    }
                }
            }

            AssistantMessageEvent::Done {
                stop_reason: sr,
                usage: u,
                cost: c,
            } => {
                stop_reason = Some(sr);
                usage = Some(u);
                cost = Some(c);
            }

            AssistantMessageEvent::Error {
                stop_reason: sr,
                error_message: em,
                usage: u,
            } => {
                stop_reason = Some(sr);
                error_message = Some(em);
                if let Some(u) = u {
                    usage = Some(u);
                }
            }
        }
    }

    let content = content.ok_or("no Start event found")?;
    let stop_reason = stop_reason.ok_or("no terminal event (Done or Error) found")?;

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    Ok(AssistantMessage {
        content,
        provider: provider.to_owned(),
        model_id: model_id.to_owned(),
        usage: usage.unwrap_or_default(),
        cost: cost.unwrap_or_default(),
        stop_reason,
        error_message,
        timestamp,
    })
}

// ─── Compile-time Send + Sync assertions ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StreamTransport>();
    assert_send_sync::<StreamOptions>();
    assert_send_sync::<AssistantMessageEvent>();
    assert_send_sync::<AssistantMessageDelta>();
};
