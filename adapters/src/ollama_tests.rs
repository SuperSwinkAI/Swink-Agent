//! Tests for `ollama`.
#![cfg(test)]

use super::*;
use crate::convert::convert_messages;
use crate::finalize::StreamFinalize;
use futures::StreamExt;
use futures::stream;
use swink_agent::{
    AgentMessage, AssistantMessage as HarnessAssistantMessage, ContentBlock, LlmMessage,
    StopReason, ToolResultMessage, UserMessage,
};

// ─── trailing slash normalization ────────────────────────────────────

#[test]
fn trailing_slash_stripped() {
    let ollama = OllamaStreamFn::new("http://localhost:11434/");
    assert_eq!(ollama.base_url, "http://localhost:11434");
}

#[test]
fn no_trailing_slash_unchanged() {
    let ollama = OllamaStreamFn::new("http://localhost:11434");
    assert_eq!(ollama.base_url, "http://localhost:11434");
}

// ─── convert_messages: user + system ────────────────────────────────

#[test]
fn convert_user_and_system_messages() {
    let messages = vec![AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(0),
    ))];

    let result = convert_messages::<OllamaConverter>(&messages, "test sys");

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, "system");
    assert_eq!(result[0].content, "test sys");
    assert_eq!(result[1].role, "user");
    assert_eq!(result[1].content, "hello");
}

// ─── ndjson_lines ───────────────────────────────────────────────────

#[tokio::test]
async fn ndjson_splits_two_lines() {
    let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("line1\nline2\n"))]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(lines.next().await.unwrap().unwrap(), "line1");
    assert_eq!(lines.next().await.unwrap().unwrap(), "line2");
    assert!(lines.next().await.is_none());
}

#[tokio::test]
async fn ndjson_crlf_line_endings() {
    let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("aaa\r\nbbb\r\n"))]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(lines.next().await.unwrap().unwrap(), "aaa");
    assert_eq!(lines.next().await.unwrap().unwrap(), "bbb");
    assert!(lines.next().await.is_none());
}

#[tokio::test]
async fn ndjson_partial_lines_across_chunks() {
    let bytes_stream = stream::iter(vec![
        Ok(bytes::Bytes::from("hel")),
        Ok(bytes::Bytes::from("lo\nwor")),
        Ok(bytes::Bytes::from("ld\n")),
    ]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(lines.next().await.unwrap().unwrap(), "hello");
    assert_eq!(lines.next().await.unwrap().unwrap(), "world");
    assert!(lines.next().await.is_none());
}

#[tokio::test]
async fn ndjson_preserves_split_utf8_across_chunks() {
    let prefix = br#"{"message":"caf"#.to_vec();
    let suffix = br#""}"#.to_vec();
    let accent = "é".as_bytes();
    let bytes_stream = stream::iter(vec![
        Ok(bytes::Bytes::from(prefix)),
        Ok(bytes::Bytes::from(vec![accent[0]])),
        Ok(bytes::Bytes::from({
            let mut tail = vec![accent[1]];
            tail.extend_from_slice(&suffix);
            tail.extend_from_slice(b"\n");
            tail
        })),
    ]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(
        lines.next().await.unwrap().unwrap(),
        r#"{"message":"café"}"#
    );
    assert!(lines.next().await.is_none());
}

#[tokio::test]
async fn ndjson_flush_remaining_buffer_no_trailing_newline() {
    let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from(
        r#"{"message":{"content":"done"},"done":true}"#,
    ))]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(
        lines.next().await.unwrap().unwrap(),
        r#"{"message":{"content":"done"},"done":true}"#
    );
    assert!(lines.next().await.is_none());
}

#[tokio::test]
async fn ndjson_incomplete_trailing_frame_is_network_error() {
    let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from(
        r#"{"message":{"content":"partial"}"#,
    ))]);
    let mut lines = ndjson_lines(bytes_stream);

    let err = lines.next().await.unwrap().unwrap_err();
    assert!(
        err.contains("incomplete trailing NDJSON frame"),
        "got: {err}"
    );
    assert!(lines.next().await.is_none());
}

/// Regression for issue #230 — transport errors on Ollama's NDJSON byte
/// stream must surface as a terminal `Err` rather than being silently
/// dropped as clean EOF. Uses a TCP listener that closes mid-body.
#[tokio::test]
async fn ndjson_surfaces_transport_error() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let header = "HTTP/1.1 200 OK\r\n\
                    Content-Type: application/x-ndjson\r\n\
                    Transfer-Encoding: chunked\r\n\r\n\
                    10\r\n{\"partial\":true";
            let _ = sock.write_all(header.as_bytes()).await;
            drop(sock);
        }
    });

    crate::base::ensure_default_crypto_provider();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect("connect");
    let mut lines = ndjson_lines(resp.bytes_stream());

    let mut saw_err = false;
    while let Some(item) = lines.next().await {
        if item.is_err() {
            saw_err = true;
            break;
        }
    }
    assert!(saw_err, "expected Err from ndjson_lines on transport error");
}

#[tokio::test]
async fn ndjson_empty_lines_skipped() {
    let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("a\n\n\nb\n"))]);
    let mut lines = ndjson_lines(bytes_stream);

    assert_eq!(lines.next().await.unwrap().unwrap(), "a");
    assert_eq!(lines.next().await.unwrap().unwrap(), "b");
    assert!(lines.next().await.is_none());
}

// ─── StreamState drain_open_blocks ──────────────────────────────────

#[test]
fn drain_open_blocks_thinking_then_text() {
    let mut blocks = crate::block_accumulator::BlockAccumulator::new();
    blocks.ensure_thinking_open(); // index 0
    blocks.ensure_text_open(); // index 1
    let mut state = StreamState { blocks };

    let drained = state.drain_open_blocks();
    assert_eq!(drained.len(), 2);

    // Thinking comes first (content_index 0), then text (content_index 1)
    match &drained[0] {
        crate::finalize::OpenBlock::Thinking { content_index, .. } => {
            assert_eq!(*content_index, 0);
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
    match &drained[1] {
        crate::finalize::OpenBlock::Text { content_index } => {
            assert_eq!(*content_index, 1);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn drain_open_blocks_idempotent() {
    let mut blocks = crate::block_accumulator::BlockAccumulator::new();
    blocks.ensure_thinking_open();
    blocks.ensure_text_open();
    let mut state = StreamState { blocks };

    let first = state.drain_open_blocks();
    let second = state.drain_open_blocks();
    assert_eq!(first.len(), 2);
    assert!(second.is_empty());
}

// ─── convert_messages: assistant with tool calls ────────────────────

#[test]
fn convert_assistant_with_tool_calls() {
    let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(
        HarnessAssistantMessage::new(
            vec![ContentBlock::ToolCall {
                id: "tc_1".to_string(),
                name: "my_tool".to_string(),
                arguments: serde_json::json!({"key": "val"}),
                partial_json: None,
            }],
            "ollama",
            "test",
        )
        .with_stop_reason(StopReason::ToolUse)
        .with_timestamp(0),
    ))];

    let result = convert_messages::<OllamaConverter>(&messages, "");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "assistant");
    let tool_calls = result[0]
        .tool_calls
        .as_ref()
        .expect("should have tool_calls");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].function.name, "my_tool");
    assert_eq!(
        tool_calls[0].function.arguments,
        serde_json::json!({"key": "val"})
    );
}

// ─── convert_messages: tool result ──────────────────────────────────

#[test]
fn convert_tool_result_message() {
    let messages = vec![AgentMessage::Llm(LlmMessage::ToolResult(
        ToolResultMessage::new(
            "tc_1",
            vec![ContentBlock::Text {
                text: "result text".to_string(),
            }],
        )
        .with_timestamp(0),
    ))];

    let result = convert_messages::<OllamaConverter>(&messages, "");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "tool");
    assert_eq!(result[0].content, "result text");
}

// ─── convert_messages: skips CustomMessage ──────────────────────────

#[test]
fn convert_skips_custom_message() {
    #[derive(Debug)]
    struct TestCustom;
    impl swink_agent::CustomMessage for TestCustom {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let messages = vec![
        AgentMessage::Custom(Box::new(TestCustom)),
        AgentMessage::Llm(LlmMessage::User(
            UserMessage::new(vec![ContentBlock::Text {
                text: "after custom".to_string(),
            }])
            .with_timestamp(0),
        )),
    ];

    let result = convert_messages::<OllamaConverter>(&messages, "");

    // Only the user message should be present; custom is skipped.
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[0].content, "after custom");
}

// ─── emit_tool_calls: regression for issue #209 ─────────────────────

/// Regression for issue #209: when Ollama emits two tool calls with the
/// same function name in one chunk, both must be dispatched. The previous
/// implementation deduped by name via a `HashSet<String>` and silently
/// dropped the second call.
#[test]
fn emit_tool_calls_preserves_repeated_same_name_calls() {
    let mut state = StreamState {
        blocks: crate::block_accumulator::BlockAccumulator::new(),
    };

    let tool_calls = vec![
        OllamaResponseToolCall {
            function: OllamaResponseFunction {
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "a.txt"}),
            },
        },
        OllamaResponseToolCall {
            function: OllamaResponseFunction {
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "b.txt"}),
            },
        },
    ];

    let events = emit_tool_calls(&mut state, &tool_calls);

    // Expect 6 events: Start/Delta/End for each of the two calls.
    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallStart { name, id, .. } => Some((name, id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts.len(),
        2,
        "both tool calls should produce a ToolCallStart"
    );
    assert_eq!(starts[0].0, "read_file");
    assert_eq!(starts[1].0, "read_file");
    assert_ne!(starts[0].1, starts[1].1, "tool call ids must be unique");

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), 2);
    assert!(deltas[0].contains("a.txt"));
    assert!(deltas[1].contains("b.txt"));

    let ends = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallEnd { .. }))
        .count();
    assert_eq!(ends, 2);
}

#[tokio::test]
async fn pre_cancelled_stream_aborts_before_request_send() {
    let ollama = OllamaStreamFn::new("http://127.0.0.1:1");
    let model = ModelSpec::new("ollama", "llama3.2");
    let context = AgentContext::new(String::new(), vec![], vec![]);
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let events: Vec<_> = ollama
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
