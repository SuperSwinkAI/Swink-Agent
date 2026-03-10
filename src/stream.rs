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

#[cfg(test)]
mod tests {
    use super::*;

    // ── 2.5: Event stream accumulates into correct AssistantMessage (text + tool call) ──

    #[test]
    fn accumulate_text_and_tool_call() {
        let events = vec![
            AssistantMessageEvent::Start,
            // Text block at index 0
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello".into(),
            },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " world".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            // Tool call block at index 1
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                id: "tc_1".into(),
                name: "search".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 1,
                delta: r#"{"q":"#.into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 1,
                delta: r#""rust"}"#.into(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 1 },
            // Terminal
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input: 100,
                    output: 50,
                    cache_read: 0,
                    cache_write: 0,
                    total: 150,
                },
                cost: Cost {
                    input: 0.01,
                    output: 0.02,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    total: 0.03,
                },
            },
        ];

        let msg = accumulate_message(events, "anthropic", "claude-sonnet-4-6").unwrap();

        assert_eq!(msg.content.len(), 2);
        assert_eq!(msg.provider, "anthropic");
        assert_eq!(msg.model_id, "claude-sonnet-4-6");
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
        assert_eq!(msg.usage.input, 100);
        assert_eq!(msg.usage.output, 50);
        assert_eq!(msg.usage.total, 150);
        assert!((msg.cost.total - 0.03).abs() < f64::EPSILON);
        assert!(msg.error_message.is_none());

        // Verify text block
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello world"),
            other => panic!("expected Text, got {other:?}"),
        }

        // Verify tool call block
        match &msg.content[1] {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                partial_json,
            } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "search");
                assert_eq!(arguments, &serde_json::json!({"q": "rust"}));
                assert!(partial_json.is_none());
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    // ── 2.6: Interleaved text and tool call blocks accumulate correctly ──

    #[test]
    fn accumulate_interleaved_text_and_tool_calls() {
        let events = vec![
            AssistantMessageEvent::Start,
            // Thinking block at index 0
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "Let me think".into(),
            },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: " about this.".into(),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                signature: Some("sig123".into()),
            },
            // Text block at index 1
            AssistantMessageEvent::TextStart { content_index: 1 },
            AssistantMessageEvent::TextDelta {
                content_index: 1,
                delta: "I'll search for that.".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 1 },
            // Tool call at index 2
            AssistantMessageEvent::ToolCallStart {
                content_index: 2,
                id: "tc_a".into(),
                name: "web_search".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 2,
                delta: r#"{"query": "rust async"}"#.into(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 2 },
            // Second tool call at index 3
            AssistantMessageEvent::ToolCallStart {
                content_index: 3,
                id: "tc_b".into(),
                name: "read_file".into(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 3,
                delta: r#"{"path": "/tmp/foo.rs"}"#.into(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 3 },
            // Terminal
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input: 200,
                    output: 100,
                    cache_read: 10,
                    cache_write: 5,
                    total: 315,
                },
                cost: Cost::default(),
            },
        ];

        let msg = accumulate_message(events, "openai", "gpt-4").unwrap();

        assert_eq!(msg.content.len(), 4);
        assert_eq!(msg.stop_reason, StopReason::ToolUse);

        // Thinking block
        match &msg.content[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "Let me think about this.");
                assert_eq!(signature.as_deref(), Some("sig123"));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }

        // Text block
        match &msg.content[1] {
            ContentBlock::Text { text } => assert_eq!(text, "I'll search for that."),
            other => panic!("expected Text, got {other:?}"),
        }

        // First tool call
        match &msg.content[2] {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(id, "tc_a");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, &serde_json::json!({"query": "rust async"}));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }

        // Second tool call
        match &msg.content[3] {
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(id, "tc_b");
                assert_eq!(name, "read_file");
                assert_eq!(arguments, &serde_json::json!({"path": "/tmp/foo.rs"}));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    // ── 2.12: StreamOptions defaults are sensible ──

    #[test]
    fn stream_options_defaults() {
        let opts = StreamOptions::default();
        assert!(opts.temperature.is_none());
        assert!(opts.max_tokens.is_none());
        assert!(opts.session_id.is_none());
        assert_eq!(opts.transport, StreamTransport::Sse);
    }

    // ── Additional: StreamTransport serde round-trip ──

    #[test]
    fn stream_transport_serde_roundtrip() {
        let transport = StreamTransport::Sse;
        let json = serde_json::to_string(&transport).unwrap();
        assert_eq!(json, r#""sse""#);
        let parsed: StreamTransport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, transport);
    }

    // ── Error event accumulation ──

    #[test]
    fn accumulate_error_event() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "partial".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "connection lost".into(),
                usage: Some(Usage {
                    input: 50,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    total: 60,
                }),
            },
        ];

        let msg = accumulate_message(events, "anthropic", "claude").unwrap();
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_message.as_deref(), Some("connection lost"));
        assert_eq!(msg.usage.total, 60);
        assert_eq!(msg.content.len(), 1);
    }

    // ── Malformed events produce errors ──

    #[test]
    fn accumulate_no_start_event() {
        let events = vec![AssistantMessageEvent::TextStart { content_index: 0 }];
        let result = accumulate_message(events, "p", "m");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("before Start"));
    }

    #[test]
    fn accumulate_no_terminal_event() {
        let events = vec![AssistantMessageEvent::Start];
        let result = accumulate_message(events, "p", "m");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("terminal event"));
    }

    #[test]
    fn accumulate_wrong_content_index() {
        let events = vec![
            AssistantMessageEvent::Start,
            // Skip index 0, go directly to 1
            AssistantMessageEvent::TextStart { content_index: 1 },
        ];
        let result = accumulate_message(events, "p", "m");
        assert!(result.is_err());
    }

    #[test]
    fn accumulate_delta_on_wrong_block_type() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            // Send a ThinkingDelta to a Text block
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "oops".into(),
            },
        ];
        let result = accumulate_message(events, "p", "m");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not Thinking"));
    }

    #[test]
    fn accumulate_tool_call_empty_args() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "tc_0".into(),
                name: "noop".into(),
            },
            // No deltas — empty arguments
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        let msg = accumulate_message(events, "p", "m").unwrap();
        match &msg.content[0] {
            ContentBlock::ToolCall { arguments, .. } => {
                assert!(arguments.is_object());
                assert!(arguments.as_object().unwrap().is_empty());
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn accumulate_error_event_without_usage() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "fatal".into(),
                usage: None,
            },
        ];

        let msg = accumulate_message(events, "p", "m").unwrap();
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_message.as_deref(), Some("fatal"));
        assert_eq!(msg.usage, Usage::default());
    }
}
