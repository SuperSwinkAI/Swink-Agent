#![cfg(feature = "ollama")]
//! Wiremock-based tests for the Ollama adapter (`OllamaStreamFn`).
//!
//! These tests exercise the public API end-to-end by standing up a mock HTTP
//! server (wiremock) and verifying that `OllamaStreamFn` correctly sends
//! requests and reconstructs `AssistantMessageEvent` streams from NDJSON
//! responses.

mod common;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::test_context;
use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::OllamaStreamFn;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("ollama", "test-model")
}

fn ndjson_response(lines: &[&str]) -> ResponseTemplate {
    let body = lines.join("\n") + "\n";
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "application/x-ndjson")
        .set_body_string(body)
}

async fn collect_events(ollama: &OllamaStreamFn) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = ollama.stream(&model, &context, &options, token);
    stream.collect::<Vec<_>>().await
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// 1. Text-only streaming response with two intermediate chunks and a final done chunk.
#[tokio::test]
async fn ollama_text_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"hel"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"lo"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":20}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    // Expect: Start, TextStart, TextDelta("hel"), TextDelta("lo"), TextEnd, Done
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
    assert!(matches!(&events[2], AssistantMessageEvent::TextDelta { delta, .. } if delta == "hel"));
    assert!(matches!(&events[3], AssistantMessageEvent::TextDelta { delta, .. } if delta == "lo"));
    assert!(matches!(
        events[4],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    match &events[5] {
        AssistantMessageEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::Stop);
            assert_eq!(usage.input, 10);
            assert_eq!(usage.output, 20);
            assert_eq!(usage.total, 30);
        }
        other => panic!("expected Done, got {other:?}"),
    }
    assert_eq!(events.len(), 6);
}

/// 2. Tool call streaming response.
#[tokio::test]
async fn ollama_tool_call_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"bash","arguments":{"command":"ls"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"tool_calls","prompt_eval_count":5,"eval_count":15}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    // Expect: Start, ToolCallStart, ToolCallDelta, ToolCallEnd, Done
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::ToolCallStart { name, .. } => {
            assert_eq!(name, "bash");
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    }
    match &events[2] {
        AssistantMessageEvent::ToolCallDelta { delta, .. } => {
            assert!(delta.contains("command"));
            assert!(delta.contains("ls"));
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
    assert!(matches!(
        events[3],
        AssistantMessageEvent::ToolCallEnd { .. }
    ));
    match &events[4] {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        other => panic!("expected Done, got {other:?}"),
    }
    assert_eq!(events.len(), 5);
}

/// 3. Text followed by tool call in a subsequent chunk.
#[tokio::test]
async fn ollama_text_then_tool() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"Let me check."},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"foo.rs"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"tool_calls","prompt_eval_count":8,"eval_count":12}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    // Verify text block is opened and closed before tool call starts
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
    assert!(
        matches!(&events[2], AssistantMessageEvent::TextDelta { delta, .. } if delta == "Let me check.")
    );
    // Text block closed when tool call arrives
    assert!(matches!(
        events[3],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    // Tool call starts at next content index
    match &events[4] {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            name,
            ..
        } => {
            assert_eq!(*content_index, 1);
            assert_eq!(name, "read_file");
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    }
    assert!(matches!(
        events[5],
        AssistantMessageEvent::ToolCallDelta { .. }
    ));
    assert!(matches!(
        events[6],
        AssistantMessageEvent::ToolCallEnd { .. }
    ));
    match &events[7] {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// 4. Thinking followed by text response.
#[tokio::test]
async fn ollama_thinking_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"","thinking":"let me think"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"the answer"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":10}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::ThinkingStart { content_index: 0 }
    ));
    assert!(
        matches!(&events[2], AssistantMessageEvent::ThinkingDelta { delta, .. } if delta == "let me think")
    );
    // Thinking block closed when text arrives
    assert!(matches!(
        events[3],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));
    assert!(matches!(
        events[4],
        AssistantMessageEvent::TextStart { content_index: 1 }
    ));
    assert!(
        matches!(&events[5], AssistantMessageEvent::TextDelta { delta, .. } if delta == "the answer")
    );
    assert!(matches!(
        events[6],
        AssistantMessageEvent::TextEnd { content_index: 1 }
    ));
    match &events[7] {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::Stop);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// 5. HTTP 500 error produces an Error event containing "500".
#[tokio::test]
async fn ollama_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("500"),
                "expected error containing '500', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

/// 6. Connection error to unreachable host.
#[tokio::test]
async fn ollama_connection_error() {
    let ollama = OllamaStreamFn::new("http://127.0.0.1:1");
    let events = collect_events(&ollama).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.to_lowercase().contains("connection error"),
                "expected 'connection error', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

/// 7. Malformed JSON line produces a parse error.
#[tokio::test]
async fn ollama_malformed_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&["{not valid json!!!}"]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let has_parse_error = events.iter().any(|e| match e {
        AssistantMessageEvent::Error { error_message, .. } => {
            error_message.to_lowercase().contains("parse error")
        }
        _ => false,
    });
    assert!(
        has_parse_error,
        "expected an error containing 'parse error', got: {events:?}"
    );
}

/// 8. Cancellation yields Aborted stop reason.
#[tokio::test]
async fn ollama_cancellation() {
    let server = MockServer::start().await;
    // Use a slow response so the cancellation can fire before it completes
    let slow_response = ndjson_response(&[
        r#"{"message":{"role":"assistant","content":"hello"},"done":false}"#,
        r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":1,"eval_count":1}"#,
    ])
    .set_delay(std::time::Duration::from_secs(30));

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token.cancel();
    });

    let stream = ollama.stream(&model, &context, &options, token);
    let events: Vec<_> = stream.collect().await;

    let has_aborted = events.iter().any(|e| match e {
        AssistantMessageEvent::Error { stop_reason, .. } => *stop_reason == StopReason::Aborted,
        _ => false,
    });
    assert!(
        has_aborted,
        "expected an Aborted event after cancellation, got: {events:?}"
    );
}

/// 9. Chunks with empty content should not produce `TextDelta` events.
#[tokio::test]
async fn ollama_empty_content_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":""},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"hi"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":1,"eval_count":1}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let text_deltas: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::TextDelta { .. }))
        .collect();

    assert_eq!(
        text_deltas.len(),
        1,
        "expected exactly 1 TextDelta (for 'hi'), got: {text_deltas:?}"
    );
    match &text_deltas[0] {
        AssistantMessageEvent::TextDelta { delta, .. } => {
            assert_eq!(delta, "hi");
        }
        _ => unreachable!(),
    }
}

/// 10. `done_reason="length"` maps to `StopReason::Length`.
#[tokio::test]
async fn ollama_length_stop_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"partial"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"length","prompt_eval_count":5,"eval_count":100}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let done_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Done { .. }));
    match done_event {
        Some(AssistantMessageEvent::Done {
            stop_reason, usage, ..
        }) => {
            assert_eq!(*stop_reason, StopReason::Length);
            assert_eq!(usage.input, 5);
            assert_eq!(usage.output, 100);
        }
        _ => panic!("expected Done event with Length stop reason, got: {events:?}"),
    }
}

/// 11. Debug impl works and contains expected strings.
#[tokio::test]
async fn ollama_debug_redacts_nothing() {
    let ollama = OllamaStreamFn::new("http://localhost:11434");
    let debug_str = format!("{ollama:?}");

    assert!(
        debug_str.contains("OllamaStreamFn"),
        "Debug output should contain 'OllamaStreamFn', got: {debug_str}"
    );
    assert!(
        debug_str.contains("http://localhost:11434"),
        "Debug output should contain base_url, got: {debug_str}"
    );
}

/// 12. Two tool calls in one chunk produce two separate `ToolCallStart` events with
/// unique `content_index` values (0 and 1).
#[tokio::test]
async fn ollama_multiple_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"bash","arguments":{"command":"ls"}}},{"function":{"name":"read_file","arguments":{"path":"a.rs"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"tool_calls","prompt_eval_count":5,"eval_count":10}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let tool_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallStart { .. }))
        .collect();

    assert_eq!(
        tool_starts.len(),
        2,
        "expected 2 ToolCallStart events, got: {tool_starts:?}"
    );

    match &tool_starts[0] {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            name,
            ..
        } => {
            assert_eq!(*content_index, 0);
            assert_eq!(name, "bash");
        }
        _ => unreachable!(),
    }
    match &tool_starts[1] {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            name,
            ..
        } => {
            assert_eq!(*content_index, 1);
            assert_eq!(name, "read_file");
        }
        _ => unreachable!(),
    }
}

/// 13. Two chunks with the same tool name — the second is deduped; only one
/// `ToolCallStart` for "bash" is emitted.
#[tokio::test]
async fn ollama_duplicate_tool_calls_deduped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"bash","arguments":{"command":"ls"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"bash","arguments":{"command":"pwd"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"tool_calls","prompt_eval_count":5,"eval_count":10}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let tool_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallStart { .. }))
        .collect();

    assert_eq!(
        tool_starts.len(),
        1,
        "expected exactly 1 ToolCallStart (duplicate deduped), got: {tool_starts:?}"
    );
    match &tool_starts[0] {
        AssistantMessageEvent::ToolCallStart { name, .. } => {
            assert_eq!(name, "bash");
        }
        _ => unreachable!(),
    }
}

/// 14. Stream ends without a done chunk — should produce an error about
/// unexpected stream end.
#[tokio::test]
async fn ollama_unexpected_stream_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"partial"},"done":false}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let has_unexpected_end = events.iter().any(|e| match e {
        AssistantMessageEvent::Error { error_message, .. } => error_message
            .to_lowercase()
            .contains("stream ended unexpectedly"),
        _ => false,
    });
    assert!(
        has_unexpected_end,
        "expected error about 'stream ended unexpectedly', got: {events:?}"
    );
}

/// 15. A chunk with an empty thinking string should be skipped — no `ThinkingStart`
/// or `ThinkingDelta` events, only text events.
#[tokio::test]
async fn ollama_empty_thinking_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"answer","thinking":""},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":10}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let has_thinking = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
        )
    });
    assert!(
        !has_thinking,
        "expected no ThinkingStart/ThinkingDelta for empty thinking, got: {events:?}"
    );

    let has_text_delta = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::TextDelta { delta, .. } if delta == "answer"));
    assert!(
        has_text_delta,
        "expected TextDelta('answer'), got: {events:?}"
    );
}

/// 16. Chunk with empty content but tool calls — no `TextStart` events, only
/// `ToolCallStart`.
#[tokio::test]
async fn ollama_assistant_empty_text_with_tools() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson_response(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"bash","arguments":{"cmd":"ls"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"tool_calls","prompt_eval_count":3,"eval_count":7}"#,
        ]))
        .mount(&server)
        .await;

    let ollama = OllamaStreamFn::new(server.uri());
    let events = collect_events(&ollama).await;

    let has_text_start = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::TextStart { .. }));
    assert!(
        !has_text_start,
        "expected no TextStart events for empty content with tools, got: {events:?}"
    );

    let has_tool_start = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::ToolCallStart { .. }));
    assert!(
        has_tool_start,
        "expected at least one ToolCallStart, got: {events:?}"
    );
}
