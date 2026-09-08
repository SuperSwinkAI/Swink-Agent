//! Tests for `proxy`.
#![cfg(test)]

use super::*;

// ─── trailing slash normalization ────────────────────────────────────

#[test]
fn trailing_slash_stripped() {
    let proxy = ProxyStreamFn::new("http://localhost:8080/", "token");
    assert_eq!(proxy.base_url, "http://localhost:8080");
}

#[test]
fn no_trailing_slash_unchanged() {
    let proxy = ProxyStreamFn::new("http://localhost:8080", "token");
    assert_eq!(proxy.base_url, "http://localhost:8080");
}

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
    let data = r#"{"type":"error","stop_reason":"error","error_message":"boom","usage":null,"error_kind":"auth"}"#;
    let event = parse_sse_event_data(data);
    match event {
        AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
            error_kind,
            retry_after: _,
        } => {
            assert_eq!(stop_reason, StopReason::Error);
            assert_eq!(error_message, "boom");
            assert!(usage.is_none());
            assert_eq!(error_kind, Some(StreamErrorKind::Auth));
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
        retry_after: None,
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
        retry_after: None,
    };
    assert!(is_terminal_event(&error));

    let start = AssistantMessageEvent::Start;
    assert!(!is_terminal_event(&start));
}

#[test]
fn terminal_error_before_start_is_prefixed() {
    let (events, started, done) = prepare_stream_event(
        AssistantMessageEvent::error_network("boom"),
        false,
        &mut ProxyStreamState::default(),
    );

    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(started);
    assert!(done);
    assert!(matches!(events[1], AssistantMessageEvent::Error { .. }));
}

#[test]
fn terminal_error_after_start_is_not_prefixed() {
    let error = AssistantMessageEvent::error_network("boom");
    let (events, started, done) =
        prepare_stream_event(error, true, &mut ProxyStreamState::default());

    assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
    assert!(started);
    assert!(done);
}

#[test]
fn terminal_error_after_open_blocks_drains_end_events_first() {
    let mut state = ProxyStreamState::default();
    let (events, started, done) = prepare_stream_event(
        AssistantMessageEvent::TextStart { content_index: 0 },
        true,
        &mut state,
    );
    assert!(matches!(
        events.as_slice(),
        [AssistantMessageEvent::TextStart { content_index: 0 }]
    ));
    assert!(started);
    assert!(!done);

    let (events, started, done) = prepare_stream_event(
        AssistantMessageEvent::error_network("boom"),
        true,
        &mut state,
    );

    assert!(started);
    assert!(done);
    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Error { .. }
        ]
    ));
}

#[test]
fn proxy_stream_fn_debug_redacts_token() {
    let proxy = ProxyStreamFn::new("http://localhost", "secret-token");
    let debug = format!("{proxy:?}");
    assert!(!debug.contains("secret-token"));
    assert!(debug.contains("[redacted]"));
}

/// Regression test for #543: transport [DONE] is not a valid substitute
/// for the proxy protocol's terminal done/error JSON event.
#[tokio::test]
async fn sse_done_sentinel_without_protocol_terminal_is_error() {
    use futures::StreamExt as _;

    // Simulate an SSE byte stream with a Start event, a text delta, and
    // then a transport-level [DONE] sentinel without a protocol terminal.
    let sse_body = concat!(
        "data: {\"type\":\"start\"}\n\n",
        "data: {\"type\":\"text_start\",\"content_index\":0}\n\n",
        "data: {\"type\":\"text_delta\",\"content_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"text_end\",\"content_index\":0}\n\n",
        "data: [DONE]\n\n",
    );

    let byte_stream =
        futures::stream::once(async move { Ok::<_, reqwest::Error>(bytes::Bytes::from(sse_body)) });

    let sse_stream = crate::sse::sse_data_lines(byte_stream);

    let cancel = CancellationToken::new();
    let event_stream = stream::unfold(
        (Box::pin(sse_stream), cancel.clone(), false),
        |(mut sse, token, mut done)| async move {
            if done {
                return None;
            }
            tokio::select! {
                biased;
                () = token.cancelled() => {
                    Some((AssistantMessageEvent::Error {
                        stop_reason: StopReason::Aborted,
                        error_message: "cancelled".to_owned(),
                        usage: None,
                        error_kind: None,
                        retry_after: None,
                    }, (sse, token, true)))
                }
                item = sse.next() => {
                    match item {
                        None => {
                            done = true;
                            Some((
                                AssistantMessageEvent::error_network("SSE stream ended unexpectedly"),
                                (sse, token, done),
                            ))
                        }
                        Some(SseLine::Done) => {
                            done = true;
                            Some((
                                AssistantMessageEvent::error_network(
                                    "network error: proxy SSE transport ended before protocol terminal event",
                                ),
                                (sse, token, done),
                            ))
                        }
                        Some(SseLine::Data(data)) => {
                            let parsed = parse_sse_event_data(&data);
                            done = is_terminal_event(&parsed);
                            Some((parsed, (sse, token, done)))
                        }
                        Some(SseLine::TransportError(msg)) => Some((
                            AssistantMessageEvent::error_network(format!("network error: {msg}")),
                            (sse, token, true),
                        )),
                        Some(SseLine::ProtocolError(msg)) => Some((
                            AssistantMessageEvent::error(format!(
                                "proxy SSE protocol error: {msg}"
                            )),
                            (sse, token, true),
                        )),
                        Some(_) => Some((AssistantMessageEvent::error_network(
                            "unexpected SSE line",
                        ), (sse, token, true))),
                    }
                }
            }
        },
    );

    let events: Vec<AssistantMessageEvent> = event_stream.collect().await;

    // The last event must be a terminal network error because the proxy
    // never emitted its protocol-level done/error JSON event.
    let last = events.last().expect("stream should produce events");
    assert!(
        matches!(
            last,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                ..
            }
        ),
        "expected Error as last event, got {last:?}"
    );

    match last {
        AssistantMessageEvent::Error { error_message, .. } => assert!(
            error_message.contains("protocol terminal event"),
            "expected terminal-event diagnostic, got: {error_message}"
        ),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn proxy_stream_raw_returns_error_for_unreachable_server() {
    let proxy = ProxyStreamFn::new("http://127.0.0.1:1", "token");
    let model = ModelSpec::new("test-provider", "test-model");
    let context = AgentContext::new("test".to_string(), vec![], vec![]);
    let options = StreamOptions::default();
    let result = proxy.stream_raw(&model, &context, &options).await;
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.contains("network error"), "got: {err}");
}

#[tokio::test]
async fn pre_cancelled_stream_aborts_before_request_send() {
    use futures::StreamExt as _;

    let proxy = ProxyStreamFn::new("http://127.0.0.1:1", "token");
    let model = ModelSpec::new("test-provider", "test-model");
    let context = AgentContext::new("test".to_string(), vec![], vec![]);
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let events: Vec<_> = proxy
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
