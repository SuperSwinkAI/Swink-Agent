//! Built-in `StreamFn` implementation that forwards LLM calls to an HTTP
//! proxy server over SSE.
//!
//! [`ProxyStreamFn`] holds a base URL and bearer token, sends a JSON request
//! to `{base_url}/v1/stream`, and parses the resulting SSE event stream into
//! [`AssistantMessageEvent`] values.

use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, Cost, LlmMessage, ModelSpec, StopReason, Usage};

use crate::classify::error_event_from_status;

// ─── Request types ──────────────────────────────────────────────────────────

/// JSON body sent to the proxy endpoint.
#[derive(Serialize)]
struct ProxyRequest<'a> {
    model: &'a str,
    system: &'a str,
    messages: Vec<&'a LlmMessage>,
    options: ProxyRequestOptions<'a>,
}

/// Options subset forwarded to the proxy.
#[derive(Serialize)]
struct ProxyRequestOptions<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

// ─── SSE event JSON schema ──────────────────────────────────────────────────

/// The JSON payload inside each SSE `data` field.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseEventData {
    Start,
    TextStart {
        content_index: usize,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
    },
    ThinkingStart {
        content_index: usize,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        signature: Option<String>,
    },
    ToolCallStart {
        content_index: usize,
        id: String,
        name: String,
    },
    ToolCallDelta {
        content_index: usize,
        delta: String,
    },
    ToolCallEnd {
        content_index: usize,
    },
    Done {
        stop_reason: StopReason,
        usage: Usage,
        cost: Cost,
    },
    Error {
        stop_reason: StopReason,
        error_message: String,
        usage: Option<Usage>,
    },
}

// ─── ProxyStreamFn ──────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation that proxies LLM calls over HTTP/SSE.
///
/// Sends a JSON POST to `{base_url}/v1/stream` with bearer token
/// authentication and parses the SSE response into `AssistantMessageEvent`
/// values.
pub struct ProxyStreamFn {
    base_url: String,
    bearer_token: String,
    client: Client,
}

impl ProxyStreamFn {
    /// Create a new proxy stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the proxy server (without trailing slash).
    /// * `bearer_token` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            bearer_token: bearer_token.into(),
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for ProxyStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyStreamFn")
            .field("base_url", &self.base_url)
            .field("bearer_token", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for ProxyStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(proxy_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

/// Build and execute the proxy request, returning the event stream.
fn proxy_stream<'a>(
    proxy: &'a ProxyStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(proxy, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let event = error_event_from_status(status.as_u16(), "", "Proxy");
            return stream::iter(vec![event]).left_stream();
        }

        parse_sse_stream(response, cancellation_token).right_stream()
    })
    .flatten()
}

/// Send the HTTP POST request to the proxy.
async fn send_request(
    proxy: &ProxyStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/v1/stream", proxy.base_url);

    let llm_messages: Vec<&LlmMessage> = context
        .messages
        .iter()
        .filter_map(|msg| match msg {
            swink_agent::types::AgentMessage::Llm(llm) => Some(llm),
            swink_agent::types::AgentMessage::Custom(_) => None,
        })
        .collect();

    let body = ProxyRequest {
        model: &model.model_id,
        system: &context.system_prompt,
        messages: llm_messages,
        options: ProxyRequestOptions {
            temperature: options.temperature,
            max_tokens: options.max_tokens,
            session_id: options.session_id.as_deref(),
        },
    };

    let bearer_token = options.api_key.as_deref().unwrap_or(&proxy.bearer_token);

    proxy
        .client
        .post(&url)
        .bearer_auth(bearer_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("network error: {e}")))
}

/// Parse the SSE byte stream into `AssistantMessageEvent` values.
///
/// Respects the cancellation token by racing each SSE event against
/// token cancellation.
fn parse_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    use eventsource_stream::Eventsource as _;

    let sse_stream = response.bytes_stream().eventsource();

    stream::unfold(
        (Box::pin(sse_stream), cancellation_token, false),
        |(mut sse, token, mut done)| async move {
            if done {
                return None;
            }

            tokio::select! {
                biased;
                () = token.cancelled() => {
                    Some((AssistantMessageEvent::Error {
                        stop_reason: StopReason::Aborted,
                        error_message: "operation cancelled".to_owned(),
                        usage: None,
                        error_kind: None,
                    }, (sse, token, true)))
                }
                item = sse.next() => {
                    match item {
                        None => {
                            // Stream ended without a Done/Error event — treat as
                            // connection drop.
                            done = true;
                            Some((
                                AssistantMessageEvent::error_network("network error: SSE stream ended unexpectedly"),
                                (sse, token, done),
                            ))
                        }
                        Some(Ok(event)) => {
                            let parsed = parse_sse_event_data(&event.data);
                            let is_terminal = is_terminal_event(&parsed);
                            if is_terminal {
                                done = true;
                            }
                            Some((parsed, (sse, token, done)))
                        }
                        Some(Err(e)) => {
                            done = true;
                            Some((
                                AssistantMessageEvent::error_network(format!("network error: SSE stream error: {e}")),
                                (sse, token, done),
                            ))
                        }
                    }
                }
            }
        },
    )
}

/// Parse a single SSE event's `data` field into an `AssistantMessageEvent`.
fn parse_sse_event_data(data: &str) -> AssistantMessageEvent {
    match serde_json::from_str::<SseEventData>(data) {
        Ok(event) => convert_sse_event(event),
        Err(e) => AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: format!("malformed SSE event JSON: {e}"),
            usage: None,
            error_kind: None,
        },
    }
}

/// Convert a parsed SSE event into an `AssistantMessageEvent`.
fn convert_sse_event(event: SseEventData) -> AssistantMessageEvent {
    match event {
        SseEventData::Start => AssistantMessageEvent::Start,
        SseEventData::TextStart { content_index } => {
            AssistantMessageEvent::TextStart { content_index }
        }
        SseEventData::TextDelta {
            content_index,
            delta,
        } => AssistantMessageEvent::TextDelta {
            content_index,
            delta,
        },
        SseEventData::TextEnd { content_index } => AssistantMessageEvent::TextEnd { content_index },
        SseEventData::ThinkingStart { content_index } => {
            AssistantMessageEvent::ThinkingStart { content_index }
        }
        SseEventData::ThinkingDelta {
            content_index,
            delta,
        } => AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta,
        },
        SseEventData::ThinkingEnd {
            content_index,
            signature,
        } => AssistantMessageEvent::ThinkingEnd {
            content_index,
            signature,
        },
        SseEventData::ToolCallStart {
            content_index,
            id,
            name,
        } => AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        },
        SseEventData::ToolCallDelta {
            content_index,
            delta,
        } => AssistantMessageEvent::ToolCallDelta {
            content_index,
            delta,
        },
        SseEventData::ToolCallEnd { content_index } => {
            AssistantMessageEvent::ToolCallEnd { content_index }
        }
        SseEventData::Done {
            stop_reason,
            usage,
            cost,
        } => AssistantMessageEvent::Done {
            stop_reason,
            usage,
            cost,
        },
        SseEventData::Error {
            stop_reason,
            error_message,
            usage,
        } => AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
            error_kind: None,
        },
    }
}

/// Check whether an event is terminal (Done or Error).
const fn is_terminal_event(event: &AssistantMessageEvent) -> bool {
    matches!(
        event,
        AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
    )
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProxyStreamFn>();
};

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_start_event() {
        let data = r#"{"type":"start"}"#;
        let event = parse_sse_event_data(data);
        assert!(matches!(event, AssistantMessageEvent::Start));
    }

    #[test]
    fn parse_text_delta_event() {
        let data = r#"{"type":"text_delta","content_index":0,"delta":"hello"}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                assert_eq!(content_index, 0);
                assert_eq!(delta, "hello");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_done_event() {
        let data = r#"{
            "type": "done",
            "stop_reason": "stop",
            "usage": {"input": 10, "output": 20, "cache_read": 0, "cache_write": 0, "total": 30},
            "cost": {"input": 0.01, "output": 0.02, "cache_read": 0.0, "cache_write": 0.0, "total": 0.03}
        }"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::Done {
                stop_reason,
                usage,
                cost,
            } => {
                assert_eq!(stop_reason, StopReason::Stop);
                assert_eq!(usage.input, 10);
                assert_eq!(usage.output, 20);
                assert!((cost.total - 0.03).abs() < f64::EPSILON);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_end_event() {
        let data = r#"{"type":"thinking_end","content_index":1,"signature":"sig123"}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                signature,
            } => {
                assert_eq!(content_index, 1);
                assert_eq!(signature, Some("sig123".to_owned()));
            }
            other => panic!("expected ThinkingEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_start_event() {
        let data = r#"{"type":"tool_call_start","content_index":2,"id":"tc_1","name":"read_file"}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                assert_eq!(content_index, 2);
                assert_eq!(id, "tc_1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_thinking_delta_event() {
        let data = r#"{"type":"thinking_delta","content_index":1,"delta":"reasoning"}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                assert_eq!(content_index, 1);
                assert_eq!(delta, "reasoning");
            }
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_delta_event() {
        let data = r#"{"type":"tool_call_delta","content_index":2,"delta":"{\"path\":"}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                assert_eq!(content_index, 2);
                assert_eq!(delta, r#"{"path":"#);
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_event() {
        let data = r#"{"type":"error","stop_reason":"error","error_message":"boom","usage":null}"#;
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::Error {
                stop_reason,
                error_message,
                usage,
                ..
            } => {
                assert_eq!(stop_reason, StopReason::Error);
                assert_eq!(error_message, "boom");
                assert!(usage.is_none());
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_yields_error_event() {
        let data = "not valid json {{{";
        let event = parse_sse_event_data(data);
        match event {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(
                    error_message.contains("malformed SSE event JSON"),
                    "got: {error_message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn network_error_uses_canonical_constructor() {
        let event = AssistantMessageEvent::error_network("network error: timeout");
        match event {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(error_message.contains("network error"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn auth_error_contains_status() {
        let event = AssistantMessageEvent::error_auth("authentication failure (401)");
        match event {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(error_message.contains("401"));
                assert!(error_message.contains("authentication"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn rate_limit_error_contains_429() {
        let event = AssistantMessageEvent::error_throttled("rate limit (429)");
        match event {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(error_message.contains("429"));
                assert!(error_message.contains("rate limit"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn aborted_has_correct_stop_reason() {
        let event = AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            error_message: "operation cancelled".to_owned(),
            usage: None,
            error_kind: None,
        };
        match event {
            AssistantMessageEvent::Error { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::Aborted);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn is_terminal_detects_done_and_error() {
        let done = AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        };
        assert!(is_terminal_event(&done));

        let error = AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "test".to_owned(),
            usage: None,
            error_kind: None,
        };
        assert!(is_terminal_event(&error));

        let start = AssistantMessageEvent::Start;
        assert!(!is_terminal_event(&start));
    }

    #[test]
    fn proxy_stream_fn_debug_redacts_token() {
        let proxy = ProxyStreamFn::new("http://localhost", "secret-token");
        let debug = format!("{proxy:?}");
        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("[redacted]"));
    }
}
