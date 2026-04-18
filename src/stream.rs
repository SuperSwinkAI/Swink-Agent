//! Streaming interface traits and types.
//!
//! Defines the `StreamFn` trait (the pluggable boundary between the harness and
//! LLM providers), the event protocol for incremental message delivery, and a
//! delta-accumulation function that reconstructs a finalized `AssistantMessage`
//! from a collected sequence of events.

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
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

// ─── CacheStrategy ──────────────────────────────────────────────────────────

/// Provider-agnostic caching configuration.
///
/// Adapters translate this to provider-specific cache markers at request
/// construction time. Adapters that don't support caching silently ignore
/// the strategy.
#[derive(Debug, Clone, Default)]
pub enum CacheStrategy {
    /// No caching (default) — no cache markers injected.
    #[default]
    None,
    /// Adapter determines optimal cache points (e.g., system prompt + tool
    /// definitions for Anthropic, long context for Google).
    Auto,
    /// Anthropic-specific: inject `cache_control: { type: "ephemeral" }`
    /// blocks on system prompt and tool definitions.
    Anthropic,
    /// Google-specific: reference a `CachedContent` resource with the given TTL.
    Google {
        /// Time-to-live for the cached content.
        ttl: Duration,
    },
}

// ─── OnRawPayload ───────────────────────────────────────────────────────────

/// Callback for observing raw SSE data lines before event parsing.
///
/// Fires synchronously with each raw `data:` line. Must return quickly
/// (fire-and-forget semantics). Panics are caught and do not interrupt
/// the streaming pipeline.
pub type OnRawPayload = Arc<dyn Fn(&str) + Send + Sync>;

// ─── StreamOptions ───────────────────────────────────────────────────────────

/// Per-call configuration passed through to the LLM provider.
#[derive(Clone, Default)]
pub struct StreamOptions {
    /// Sampling temperature (optional).
    pub temperature: Option<f64>,
    /// Output token limit (optional).
    pub max_tokens: Option<u64>,
    /// Provider-side session identifier for caching (optional).
    pub session_id: Option<String>,
    /// Dynamically resolved API key for this specific request (optional).
    pub api_key: Option<String>,
    /// Preferred transport protocol.
    pub transport: StreamTransport,
    /// Provider-agnostic caching configuration.
    pub cache_strategy: CacheStrategy,
    /// Optional callback for observing raw SSE data lines before parsing.
    pub on_raw_payload: Option<OnRawPayload>,
}

impl std::fmt::Debug for StreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamOptions")
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("session_id", &self.session_id)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("transport", &self.transport)
            .field("cache_strategy", &self.cache_strategy)
            .field(
                "on_raw_payload",
                &self.on_raw_payload.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

// ─── StreamErrorKind ─────────────────────────────────────────────────────────

/// Structured classification of stream errors.
///
/// Adapters can attach a `StreamErrorKind` to an `Error` event so the agent
/// loop can classify errors structurally instead of relying on string matching.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorKind {
    /// The provider throttled the request (HTTP 429 / rate limit).
    Throttled,
    /// The request exceeded the model's context window.
    ContextWindowExceeded,
    /// Authentication or authorization failure (HTTP 401/403).
    Auth,
    /// Transient network or server error (connection drop, 5xx, etc.).
    Network,
    /// Provider safety/content filter blocked the response.
    ContentFiltered,
}

// ─── AssistantMessageEvent ───────────────────────────────────────────────────

/// An incremental event emitted by a `StreamFn` implementation.
///
/// Events follow a strict start/delta/end protocol per content block. Each
/// block carries a `content_index` that identifies its position in the final
/// message's content vec.
#[non_exhaustive]
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
        /// Optional structured error classification.
        ///
        /// When set, the agent loop uses this to classify the error without
        /// falling back to string matching on `error_message`.
        error_kind: Option<StreamErrorKind>,
    },
}

impl AssistantMessageEvent {
    /// Create a stream error event with no structured classification.
    ///
    /// Convenience constructor used by adapters when the stream encounters
    /// an error condition. The `error_kind` is set to `None`, so the agent
    /// loop will fall back to string-based classification.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: None,
        }
    }

    /// Create a throttle/rate-limit error event.
    ///
    /// Sets [`StreamErrorKind::Throttled`] so the agent loop can classify
    /// the error structurally.
    pub fn error_throttled(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Throttled),
        }
    }

    /// Create a context-window overflow error event.
    ///
    /// Sets [`StreamErrorKind::ContextWindowExceeded`] so the agent loop
    /// can trigger context compaction.
    pub fn error_context_overflow(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::ContextWindowExceeded),
        }
    }

    /// Create an authentication error event.
    ///
    /// Sets [`StreamErrorKind::Auth`] so the agent loop can treat this as
    /// a non-retryable failure.
    pub fn error_auth(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Auth),
        }
    }

    /// Create a network/server error event.
    ///
    /// Sets [`StreamErrorKind::Network`] so the agent loop can classify
    /// the error as retryable.
    pub fn error_network(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::Network),
        }
    }

    /// Create a content-filtered error event.
    ///
    /// Sets [`StreamErrorKind::ContentFiltered`] so the agent loop can
    /// treat this as a non-retryable safety policy violation.
    pub fn error_content_filtered(message: impl Into<String>) -> Self {
        Self::Error {
            stop_reason: StopReason::Error,
            error_message: message.into(),
            usage: None,
            error_kind: Some(StreamErrorKind::ContentFiltered),
        }
    }

    /// Build a complete single-text-block response event sequence.
    ///
    /// Useful for testing and mock `StreamFn` implementations. Returns the
    /// five events needed for a valid text-only response: `Start`, `TextStart`,
    /// `TextDelta`, `TextEnd`, and `Done`.
    pub fn text_response(text: &str) -> Vec<Self> {
        vec![
            Self::Start,
            Self::TextStart { content_index: 0 },
            Self::TextDelta {
                content_index: 0,
                delta: text.to_string(),
            },
            Self::TextEnd { content_index: 0 },
            Self::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ]
    }
}

// ─── AssistantMessageDelta ───────────────────────────────────────────────────

/// A typed incremental update during streaming, used in `MessageUpdate` events.
///
/// The `delta` field uses [`Cow<'static, str>`] to avoid cloning on the hot
/// path when the caller can transfer ownership of the underlying `String`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageDelta {
    /// An appended text string fragment.
    Text {
        content_index: usize,
        delta: Cow<'static, str>,
    },
    /// An appended reasoning fragment.
    Thinking {
        content_index: usize,
        delta: Cow<'static, str>,
    },
    /// An appended JSON argument fragment for a tool call.
    ToolCall {
        content_index: usize,
        delta: Cow<'static, str>,
    },
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
    fn ensure_block_open(
        open_blocks: &[bool],
        content_index: usize,
        event_name: &str,
    ) -> Result<(), String> {
        match open_blocks.get(content_index) {
            Some(false) => Err(format!(
                "{event_name}: block at index {content_index} is already closed"
            )),
            Some(true) | None => Ok(()),
        }
    }

    fn all_open_blocks_are_tool_calls(content: &[ContentBlock], open_blocks: &[bool]) -> bool {
        open_blocks
            .iter()
            .enumerate()
            .filter(|(_, open)| **open)
            .all(|(content_index, _)| {
                matches!(
                    content.get(content_index),
                    Some(ContentBlock::ToolCall { .. })
                )
            })
    }

    let mut content: Option<Vec<ContentBlock>> = None;
    // Parallel to `content`: tracks whether each block is still open (awaiting
    // its matching `*End` event). Finalization (on `Done`) fails if any block
    // is still open, preventing silently-corrupt assistant messages.
    let mut open_blocks: Vec<bool> = Vec::new();
    let mut stop_reason: Option<StopReason> = None;
    let mut usage: Option<Usage> = None;
    let mut cost: Option<Cost> = None;
    let mut error_message: Option<String> = None;
    let mut error_kind: Option<StreamErrorKind> = None;
    let mut saw_start = false;
    let mut saw_terminal = false;

    // Pre-scan for a Length stop reason. Providers can emit `ToolCallEnd` with
    // truncated JSON arguments (or omit it entirely) when hitting the max-token
    // limit mid tool-call. We must preserve the incomplete block so the loop's
    // `recover_incomplete_tool_calls` path can convert it into an error tool
    // result and continue on the next turn. See issue #221.
    let tolerate_truncated_tool_args = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                ..
            } | AssistantMessageEvent::Error {
                stop_reason: StopReason::Length,
                ..
            }
        )
    });

    for event in events {
        // Reject content-block events after a terminal event.
        match &event {
            AssistantMessageEvent::TextStart { .. }
            | AssistantMessageEvent::TextDelta { .. }
            | AssistantMessageEvent::TextEnd { .. }
            | AssistantMessageEvent::ThinkingStart { .. }
            | AssistantMessageEvent::ThinkingDelta { .. }
            | AssistantMessageEvent::ThinkingEnd { .. }
            | AssistantMessageEvent::ToolCallStart { .. }
            | AssistantMessageEvent::ToolCallDelta { .. }
            | AssistantMessageEvent::ToolCallEnd { .. } => {
                if saw_terminal {
                    return Err("content event after terminal event".into());
                }
            }
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => {
                if saw_terminal {
                    return Err("duplicate terminal event".into());
                }
            }
            AssistantMessageEvent::Start => {}
        }

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
                open_blocks.push(true);
            }

            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("TextDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "TextDelta")?;
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
                ensure_block_open(&open_blocks, content_index, "TextEnd")?;
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
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
                open_blocks.push(true);
            }

            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ThinkingDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "ThinkingDelta")?;
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
                ensure_block_open(&open_blocks, content_index, "ThinkingEnd")?;
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
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
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
                open_blocks.push(true);
            }

            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                let blocks = content.as_mut().ok_or("ToolCallDelta before Start")?;
                ensure_block_open(&open_blocks, content_index, "ToolCallDelta")?;
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
                ensure_block_open(&open_blocks, content_index, "ToolCallEnd")?;
                match block {
                    ContentBlock::ToolCall {
                        arguments,
                        partial_json,
                        ..
                    } => {
                        let json_str = partial_json
                            .as_ref()
                            .ok_or("ToolCallEnd: partial_json already consumed")?
                            .clone();
                        if json_str.is_empty() {
                            *arguments = Value::Object(serde_json::Map::new());
                            *partial_json = None;
                        } else {
                            match serde_json::from_str::<Value>(&json_str) {
                                Ok(v) => {
                                    *arguments = v;
                                    *partial_json = None;
                                }
                                Err(e) => {
                                    if tolerate_truncated_tool_args {
                                        // Leave `partial_json` as Some so the
                                        // block is flagged incomplete and the
                                        // loop recovers on the next turn.
                                    } else {
                                        return Err(format!(
                                            "ToolCallEnd: failed to parse arguments JSON: {e}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        return Err(format!(
                            "ToolCallEnd: block at index {content_index} is not ToolCall"
                        ));
                    }
                }
                if let Some(open) = open_blocks.get_mut(content_index) {
                    *open = false;
                }
            }

            AssistantMessageEvent::Done {
                stop_reason: sr,
                usage: u,
                cost: c,
            } => {
                if let Some(idx) = open_blocks.iter().position(|open| *open) {
                    let content = content.as_ref().ok_or("Done before Start")?;
                    if tolerate_truncated_tool_args
                        && all_open_blocks_are_tool_calls(content, &open_blocks)
                    {
                        // Max-tokens truncation: leave open tool-call blocks
                        // with `partial_json` set so the loop's
                        // `recover_incomplete_tool_calls` path can convert
                        // them into error tool results on the next turn.
                        tracing::debug!(
                            "Done(Length) with unterminated content block at index {idx} — tolerating for max-tokens recovery"
                        );
                    } else {
                        return Err(format!(
                            "Done received with unterminated content block at index {idx}"
                        ));
                    }
                }
                stop_reason = Some(sr);
                usage = Some(u);
                cost = Some(c);
                saw_terminal = true;
            }

            AssistantMessageEvent::Error {
                stop_reason: sr,
                error_message: em,
                usage: u,
                error_kind: ek,
            } => {
                stop_reason = Some(sr);
                error_message = Some(em);
                error_kind = ek;
                if let Some(u) = u {
                    usage = Some(u);
                }
                saw_terminal = true;
            }
        }
    }

    let content = content.ok_or("no Start event found")?;
    let stop_reason = stop_reason.ok_or("no terminal event (Done or Error) found")?;

    let timestamp = crate::util::now_timestamp();

    Ok(AssistantMessage {
        content,
        provider: provider.to_owned(),
        model_id: model_id.to_owned(),
        usage: usage.unwrap_or_default(),
        cost: cost.unwrap_or_default(),
        stop_reason,
        error_message,
        error_kind,
        timestamp,
        cache_hint: None,
    })
}

// ─── Compile-time Send + Sync assertions ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StreamErrorKind>();
    assert_send_sync::<StreamTransport>();
    assert_send_sync::<StreamOptions>();
    assert_send_sync::<AssistantMessageEvent>();
    assert_send_sync::<AssistantMessageDelta>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_with_unterminated_text_block_is_rejected() {
        // Regression for #206: a Text block opened but never closed before Done
        // must not silently produce a corrupt assistant message.
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert!(err.contains("unterminated content block"), "got: {err}");
    }

    #[test]
    fn done_with_unterminated_tool_call_block_is_rejected() {
        // Regression for #206: missing ToolCallEnd must be rejected.
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "t1".into(),
                name: "foo".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert!(err.contains("unterminated content block"), "got: {err}");
    }

    #[test]
    fn done_with_all_blocks_terminated_succeeds() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "ok".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        let msg = accumulate_message(events, "test", "test").expect("should succeed");
        assert_eq!(msg.content.len(), 1);
    }

    #[test]
    fn error_with_unterminated_block_is_allowed() {
        // Errors may legitimately abort mid-block; don't mask them with a
        // validation failure.
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "boom".into(),
                usage: None,
                error_kind: None,
            },
        ];
        let msg = accumulate_message(events, "test", "test").expect("error terminal ok");
        assert_eq!(msg.error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn error_constructor_sets_kind_none() {
        let event = AssistantMessageEvent::error("boom");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(error_kind, None);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_throttled_constructor_sets_kind() {
        let event = AssistantMessageEvent::error_throttled("rate limited");
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert_eq!(error_kind, Some(StreamErrorKind::Throttled));
                assert_eq!(error_message, "rate limited");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_context_overflow_constructor_sets_kind() {
        let event = AssistantMessageEvent::error_context_overflow("too long");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(error_kind, Some(StreamErrorKind::ContextWindowExceeded));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_auth_constructor_sets_kind() {
        let event = AssistantMessageEvent::error_auth("bad key");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(error_kind, Some(StreamErrorKind::Auth));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_network_constructor_sets_kind() {
        let event = AssistantMessageEvent::error_network("timeout");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(error_kind, Some(StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_content_filtered_constructor_sets_kind() {
        let event = AssistantMessageEvent::error_content_filtered("blocked by safety filter");
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert_eq!(error_kind, Some(StreamErrorKind::ContentFiltered));
                assert_eq!(error_message, "blocked by safety filter");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn text_response_produces_valid_event_sequence() {
        let events = AssistantMessageEvent::text_response("hello world");
        assert_eq!(events.len(), 5);
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        assert!(matches!(
            events[1],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
        match &events[2] {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(delta, "hello world");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(matches!(
            events[3],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        assert!(matches!(
            events[4],
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                ..
            }
        ));
    }

    // Regression for #293: Done(Length) with an unterminated tool-call block
    // must NOT be rejected — the block should survive with `partial_json` set
    // so `recover_incomplete_tool_calls` can convert it to an error result.
    #[test]
    fn done_length_with_unterminated_tool_call_is_tolerated() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "tc_1".into(),
                name: "read_file".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: r#"{"path": "/tmp"#.into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        let msg = accumulate_message(events, "test", "test")
            .expect("Done(Length) with open tool-call block should succeed");
        assert_eq!(msg.stop_reason, StopReason::Length);
        // The tool call block should have partial_json set (incomplete)
        match &msg.content[0] {
            ContentBlock::ToolCall { partial_json, .. } => {
                assert!(
                    partial_json.is_some(),
                    "partial_json should be Some for incomplete tool call"
                );
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn done_length_with_unterminated_text_block_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "partial".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert!(err.contains("unterminated content block"), "got: {err}");
    }

    #[test]
    fn done_length_with_unterminated_thinking_block_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "partial".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert!(err.contains("unterminated content block"), "got: {err}");
    }

    #[test]
    fn text_response_accumulates_correctly() {
        let events = AssistantMessageEvent::text_response("accumulated text");
        let msg = accumulate_message(events, "test", "test-model").expect("accumulation failed");
        assert_eq!(msg.content.len(), 1);
        assert_eq!(ContentBlock::extract_text(&msg.content), "accumulated text");
        assert_eq!(msg.stop_reason, StopReason::Stop);
    }

    #[test]
    fn text_delta_after_text_end_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " again".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert_eq!(err, "TextDelta: block at index 0 is already closed");
    }

    #[test]
    fn duplicate_text_end_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert_eq!(err, "TextEnd: block at index 0 is already closed");
    }

    #[test]
    fn duplicate_thinking_end_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "step 1".into(),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                signature: Some("sig-1".into()),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                signature: Some("sig-2".into()),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert_eq!(err, "ThinkingEnd: block at index 0 is already closed");
    }

    #[test]
    fn tool_call_delta_after_end_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "tool-1".into(),
                name: "read_file".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"path\":\"/tmp/a\"}".into(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: ",\"extra\":true}".into(),
            },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert_eq!(err, "ToolCallDelta: block at index 0 is already closed");
    }

    #[test]
    fn duplicate_tool_call_end_is_rejected() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "tool-1".into(),
                name: "read_file".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"path\":\"/tmp/a\"}".into(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let err = accumulate_message(events, "test", "test").unwrap_err();
        assert_eq!(err, "ToolCallEnd: block at index 0 is already closed");
    }
}
