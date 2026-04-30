#![cfg(feature = "mistral")]
//! Wiremock-based tests for `MistralStreamFn`.
//!
//! Full test parity with the `OpenAI` adapter: text streaming, tool calls,
//! multi-tool, errors, cancellation, usage, edge cases.

mod common;

use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessage,
    AssistantMessageEvent, ContentBlock, LlmMessage, ModelSpec, StopReason, StreamErrorKind,
    StreamFn, StreamOptions, ToolResultMessage, UserMessage,
};
use swink_agent_adapters::MistralStreamFn;

use common::{
    event_name, extract_stop_reason, find_error_message, notify_on_request, sse_response,
    test_context,
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("mistral", "mistral-small-latest")
}

async fn collect_events(stream_fn: &MistralStreamFn) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    stream_fn
        .stream(&model, &context, &options, token)
        .collect::<Vec<_>>()
        .await
}

async fn collect_events_with_context(
    stream_fn: &MistralStreamFn,
    context: &AgentContext,
) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    stream_fn
        .stream(&model, context, &options, token)
        .collect::<Vec<_>>()
        .await
}

fn make_assistant_with_tool_call(tool_call_id: &str, tool_name: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: tool_call_id.to_string(),
            name: tool_name.to_string(),
            arguments: serde_json::json!({}),
            partial_json: None,
        }],
        stop_reason: StopReason::ToolUse,
        usage: swink_agent::Usage::default(),
        cost: swink_agent::Cost::default(),
        model_id: String::new(),
        provider: String::new(),
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }
}

fn make_tool_result(tool_call_id: &str, result_text: &str) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        content: vec![ContentBlock::Text {
            text: result_text.to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: Value::Null,
        cache_hint: None,
    }
}

fn make_user_message(text: &str) -> UserMessage {
    UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }
}

// ── US1: Text Streaming ────────────────────────────────────────────────────

#[tokio::test]
async fn text_stream_happy_path() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    let delta_text: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_text, "ok");
}

#[tokio::test]
async fn usage_from_final_chunk() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":42,"completion_tokens":17}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("missing Done event");
    assert_eq!(usage.input, 42);
    assert_eq!(usage.output, 17);
}

#[tokio::test]
async fn request_body_no_stream_options() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions {
        max_tokens: Some(100),
        ..StreamOptions::default()
    };
    let token = CancellationToken::new();
    let _events: Vec<_> = sf.stream(&model, &context, &options, token).collect().await;

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();

    assert_eq!(body["max_tokens"], 100);
    assert!(
        body.get("max_completion_tokens").is_none(),
        "must not send max_completion_tokens"
    );
    assert!(
        body.get("stream_options").is_none(),
        "must not send stream_options to Mistral"
    );
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn model_length_finish_reason() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"model_length","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":100}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let stop_reason = extract_stop_reason(&events).expect("missing Done event");
    assert_eq!(
        stop_reason,
        StopReason::Length,
        "model_length should map to Length"
    );
}

#[tokio::test]
async fn stream_cancellation() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
    ]
    .join("\n");

    let (slow_response, request_seen) =
        notify_on_request(sse_response(&body).set_delay(std::time::Duration::from_secs(30)));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let cancel_token = token.clone();
    let events_handle = tokio::spawn(async move {
        sf.stream(&model, &context, &options, token)
            .collect::<Vec<_>>()
            .await
    });

    request_seen.notified().await;
    cancel_token.cancel();
    let events = events_handle.await.expect("stream task should complete");

    let has_aborted = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Aborted,
                ..
            }
        )
    });
    assert!(has_aborted, "expected Aborted event, got: {events:?}");
}

#[tokio::test]
async fn multi_chunk_text_assembly() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"one "},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"two "},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"three "},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"four "},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"five"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":5}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), 5);
    assert_eq!(deltas.join(""), "one two three four five");
}

// ── US2: Tool Call Streaming ───────────────────────────────────────────────

#[tokio::test]
async fn single_tool_call() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"abc123DEF","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"ToolCallStart"));
    assert!(types.contains(&"ToolCallDelta"));
    assert!(types.contains(&"ToolCallEnd"));

    let start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
        _ => None,
    });
    let (id, name) = start.expect("missing ToolCallStart");
    assert_eq!(name, "bash");
    assert!(
        id.starts_with("call_"),
        "expected harness-format ID starting with 'call_', got: {id}"
    );

    assert_eq!(extract_stop_reason(&events), Some(StopReason::ToolUse));
}

#[tokio::test]
async fn multi_tool_calls() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"aaaaaaaaa","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"ls\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"bbbbbbbbb","function":{"name":"read_file","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":\"foo.txt\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":30}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let tool_starts: Vec<(usize, String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => Some((*content_index, id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(tool_starts.len(), 2);
    assert_eq!(tool_starts[0].2, "bash");
    assert_eq!(tool_starts[1].2, "read_file");
    assert_eq!(tool_starts[0].0, 0);
    assert_eq!(tool_starts[1].0, 1);

    for (_, id, _) in &tool_starts {
        assert!(id.starts_with("call_"), "expected harness ID, got: {id}");
    }

    let tool_end_count = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallEnd { .. }))
        .count();
    assert_eq!(tool_end_count, 2);
}

#[tokio::test]
async fn tool_call_id_format_in_request() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");

    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::Assistant(make_assistant_with_tool_call(
                "call_abc123xyz456",
                "bash",
            ))),
            AgentMessage::Llm(LlmMessage::ToolResult(make_tool_result(
                "call_abc123xyz456",
                "result",
            ))),
        ],
        tools: Vec::new(),
    };

    let _events = collect_events_with_context(&sf, &context).await;

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();

    let messages = body["messages"].as_array().unwrap();
    let assistant_msg = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("missing assistant message");
    let tc_id = assistant_msg["tool_calls"][0]["id"].as_str().unwrap();

    assert_eq!(tc_id.len(), 9, "Mistral ID must be 9 chars, got: {tc_id}");
    assert!(
        tc_id.chars().all(|c| c.is_ascii_alphanumeric()),
        "Mistral ID must be alphanumeric, got: {tc_id}"
    );

    let tool_msg = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("missing tool message");
    let tool_call_id = tool_msg["tool_call_id"].as_str().unwrap();
    assert_eq!(
        tool_call_id, tc_id,
        "tool result ID must match assistant ID"
    );
}

#[tokio::test]
async fn full_tool_call_in_single_chunk() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"XyZ987AbC","function":{"name":"read_file","arguments":"{\"path\":\"hello.txt\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(start, Some("read_file".to_string()));

    let delta_text: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_text, r#"{"path":"hello.txt"}"#);
}

#[tokio::test]
async fn tool_definitions_in_request() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");

    let tool: Arc<dyn AgentTool> = Arc::new(DummyTool);
    let context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![tool],
    };

    let _events = collect_events_with_context(&sf, &context).await;

    let received = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();

    let tools = body["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "dummy");
    assert_eq!(body["tool_choice"], "auto");
}

// ── US3: Mistral-Specific Endpoint ─────────────────────────────────────────

#[tokio::test]
async fn endpoint_url_trailing_slash() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(format!("{}/", server.uri()), "test-key");
    let events = collect_events(&sf).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );
}

#[tokio::test]
async fn bearer_auth_header() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer my-secret-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "my-secret-key");
    let events = collect_events(&sf).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );
}

#[tokio::test]
async fn message_ordering_normalization() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");

    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::User(make_user_message("call the tool"))),
            AgentMessage::Llm(LlmMessage::Assistant(make_assistant_with_tool_call(
                "call_xyz", "bash",
            ))),
            AgentMessage::Llm(LlmMessage::ToolResult(make_tool_result(
                "call_xyz", "result",
            ))),
            // User message immediately after tool result.
            AgentMessage::Llm(LlmMessage::User(make_user_message("thanks"))),
        ],
        tools: Vec::new(),
    };

    let _events = collect_events_with_context(&sf, &context).await;

    let received = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let messages = body["messages"].as_array().unwrap();

    let tool_idx = messages
        .iter()
        .position(|m| m["role"] == "tool")
        .expect("missing tool message");
    let next_msg = &messages[tool_idx + 1];

    assert_eq!(
        next_msg["role"], "assistant",
        "expected synthetic assistant between tool and user, got: {next_msg}"
    );

    let user_after = &messages[tool_idx + 2];
    assert_eq!(user_after["role"], "user");
}

#[tokio::test]
async fn finish_reason_error() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"error","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Error { .. })),
        "expected Error event, got: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AssistantMessageEvent::Done { .. })),
        "did not expect Done event, got: {events:?}"
    );

    let (message, usage, error_kind) = events
        .iter()
        .find_map(|event| match event {
            AssistantMessageEvent::Error {
                error_message,
                usage,
                error_kind,
                ..
            } => Some((error_message.clone(), usage.clone(), *error_kind)),
            _ => None,
        })
        .expect("missing Error event");

    assert!(
        message.contains("finish_reason=error"),
        "expected finish_reason in message, got: {message}"
    );
    assert_eq!(error_kind, Some(StreamErrorKind::Network));

    let usage = usage.expect("expected usage on terminal error");
    assert_eq!(usage.input, 5);
    assert_eq!(usage.output, 2);
    assert_eq!(usage.total, 7);
}

// ── US4: Error Handling ────────────────────────────────────────────────────

#[tokio::test]
async fn http_429_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("rate limit"), "got: {err}");
}

#[tokio::test]
async fn http_401_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("auth error"), "got: {err}");
}

#[tokio::test]
async fn http_500_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("server error"), "got: {err}");
}

#[tokio::test]
async fn http_422_validation_error() {
    let error_body = r#"{"message":"Extra inputs are not permitted"}"#;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(422).set_body_string(error_body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(!err.is_empty(), "should have descriptive error for 422");
}

// ── Edge cases ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn text_then_tool() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"thinking..."},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"AAAbbbCCC","function":{"name":"bash","arguments":"{\"cmd\":\"ls\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    let text_end_pos = types
        .iter()
        .position(|&t| t == "TextEnd")
        .expect("missing TextEnd");
    let tool_start_pos = types
        .iter()
        .position(|&t| t == "ToolCallStart")
        .expect("missing ToolCallStart");
    assert!(text_end_pos < tool_start_pos);
}

#[tokio::test]
async fn debug_redacts_key() {
    let sf = MistralStreamFn::new("https://api.mistral.ai", "sk-secret-key-12345");
    let debug = format!("{sf:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("sk-secret-key-12345"));
}

#[tokio::test]
async fn api_key_override_in_options() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer override-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "default-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions {
        api_key: Some("override-key".to_string()),
        ..StreamOptions::default()
    };
    let token = CancellationToken::new();
    let events: Vec<_> = sf.stream(&model, &context, &options, token).collect().await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );
}

#[tokio::test]
async fn empty_content_delta_skipped() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":""},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"real text"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["real text"]);
}

#[tokio::test]
async fn malformed_json() {
    let body = [r"data: {not valid json!!!}", "", "data: [DONE]", "", ""].join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("parse error") || err.contains("JSON"));
}

#[tokio::test]
async fn empty_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(""))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert!(events.iter().map(event_name).any(|name| name == "Start"));
    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("stream ended unexpectedly"));
}

#[tokio::test]
async fn done_without_finish_reason() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"index":0}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = MistralStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert_eq!(extract_stop_reason(&events), Some(StopReason::Stop));
}

// ── Dummy tool for tests ───────────────────────────────────────────────────

static DUMMY_SCHEMA: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    serde_json::json!({
        "type": "object",
        "properties": {
            "input": {"type": "string"}
        }
    })
});

struct DummyTool;

impl AgentTool for DummyTool {
    fn name(&self) -> &'static str {
        "dummy"
    }
    fn label(&self) -> &'static str {
        "Dummy"
    }
    fn description(&self) -> &'static str {
        "A dummy tool for testing"
    }
    fn parameters_schema(&self) -> &Value {
        &DUMMY_SCHEMA
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _arguments: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { AgentToolResult::text("done") })
    }
}
