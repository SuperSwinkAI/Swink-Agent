//! Tests for `bedrock`.
#![cfg(test)]

use super::*;
use reqwest::header::{CONTENT_TYPE, HOST};
use swink_agent::StreamErrorKind;

#[test]
fn sigv4_signs_request_with_expected_headers() {
    let signer = BedrockStreamFn::new("us-east-1", "AKIDEXAMPLE", "secret", None);
    let body = b"{}";
    let mut request = signer
        .client
        .post("https://bedrock-runtime.us-east-1.amazonaws.com/model/test-model/converse-stream")
        .body(body.to_vec())
        .build()
        .unwrap();
    request.headers_mut().insert(
        CONTENT_TYPE,
        HttpHeaderValue::from_static("application/json"),
    );
    request.headers_mut().insert(
        HOST,
        HttpHeaderValue::from_static("bedrock-runtime.us-east-1.amazonaws.com"),
    );

    signer.sign_request(&mut request, body).unwrap();

    assert!(request.headers().contains_key("authorization"));
    assert!(request.headers().contains_key("x-amz-date"));
    assert!(request.headers().contains_key("x-amz-content-sha256"));
    let authorization = request.headers()["authorization"].to_str().unwrap();
    assert!(authorization.contains("AWS4-HMAC-SHA256"));
    assert!(authorization.contains("Credential=AKIDEXAMPLE/"));
    assert!(authorization.contains("/us-east-1/bedrock/aws4_request"));
}

#[test]
fn sigv4_signing_includes_session_token() {
    let signer = BedrockStreamFn::new(
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        Some("session-token".to_string()),
    );
    let body = b"{}";
    let mut request = signer
        .client
        .post("https://bedrock-runtime.us-east-1.amazonaws.com/model/test-model/converse-stream")
        .body(body.to_vec())
        .build()
        .unwrap();
    request.headers_mut().insert(
        CONTENT_TYPE,
        HttpHeaderValue::from_static("application/json"),
    );
    request.headers_mut().insert(
        HOST,
        HttpHeaderValue::from_static("bedrock-runtime.us-east-1.amazonaws.com"),
    );

    signer.sign_request(&mut request, body).unwrap();

    assert_eq!(request.headers()["x-amz-security-token"], "session-token");
    let authorization = request.headers()["authorization"].to_str().unwrap();
    assert!(authorization.contains("x-amz-security-token"));
}

#[test]
fn parse_message_start_event() {
    let mut state = BedrockStreamState::new();
    let payload = br#"{"role":"assistant"}"#;
    let events = parse_event_frame("messageStart", payload, &mut state).unwrap();
    assert!(matches!(
        events.as_deref(),
        Some([AssistantMessageEvent::Start])
    ));
}

#[test]
fn parse_text_content_block_events() {
    let mut state = BedrockStreamState::new();

    // contentBlockStart with text
    let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
    let events = parse_event_frame("contentBlockStart", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
    assert!(state.blocks.text_open());

    // contentBlockDelta with text
    let payload = br#"{"contentBlockIndex":0,"delta":{"type":"text","text":"Hello"}}"#;
    let events = parse_event_frame("contentBlockDelta", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello"
    ));

    // contentBlockStop
    let payload = br#"{"contentBlockIndex":0}"#;
    let events = parse_event_frame("contentBlockStop", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(!state.blocks.text_open());
}

#[test]
fn parse_tool_use_content_block_events() {
    let mut state = BedrockStreamState::new();

    // contentBlockStart with toolUse — provider index 1, but harness index
    // is 0 because no prior blocks were opened through the accumulator.
    let payload = br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_123","name":"get_weather"}}"#;
    let events = parse_event_frame("contentBlockStart", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallStart { content_index: 0, id, name }
            if id == "tc_123" && name == "get_weather"
    ));
    assert!(state.provider_blocks.contains_key(&1));

    // contentBlockDelta with toolUse input
    let payload =
        br#"{"contentBlockIndex":1,"delta":{"type":"toolUse","input":"{\"city\":\"SF\"}"}}"#;
    let events = parse_event_frame("contentBlockDelta", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
            if delta == r#"{"city":"SF"}"#
    ));

    // contentBlockStop
    let payload = br#"{"contentBlockIndex":1}"#;
    let events = parse_event_frame("contentBlockStop", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 0 }
    ));
}

#[test]
fn parse_message_stop_and_metadata() {
    let mut state = BedrockStreamState::new();

    // messageStop captures stop_reason
    let payload = br#"{"stopReason":"end_turn"}"#;
    let events = parse_event_frame("messageStop", payload, &mut state).unwrap();
    assert!(events.is_none());
    assert_eq!(state.stop_reason.as_deref(), Some("end_turn"));

    // metadata emits Done
    let payload = br#"{"usage":{"inputTokens":10,"outputTokens":20,"totalTokens":30},"metrics":{"latencyMs":150}}"#;
    let events = parse_event_frame("metadata", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::Done { stop_reason: StopReason::Stop, usage, .. }
            if usage.input == 10 && usage.output == 20 && usage.total == 30
    ));
}

#[test]
fn map_stop_reason_variants() {
    assert_eq!(map_stop_reason(Some("end_turn")).unwrap(), StopReason::Stop);
    assert_eq!(
        map_stop_reason(Some("stop_sequence")).unwrap(),
        StopReason::Stop
    );
    assert_eq!(
        map_stop_reason(Some("tool_use")).unwrap(),
        StopReason::ToolUse
    );
    assert_eq!(
        map_stop_reason(Some("max_tokens")).unwrap(),
        StopReason::Length
    );
    assert_eq!(map_stop_reason(None).unwrap(), StopReason::Stop);
    assert!(map_stop_reason(Some("guardrail_intervened")).is_err());
}

#[test]
fn bedrock_exception_classification_uses_exact_suffix() {
    let throttled = classify_bedrock_exception(
        "com.amazonaws.bedrockruntime#ThrottlingException",
        "slow down",
    );
    assert!(matches!(
        throttled,
        AssistantMessageEvent::Error {
            error_kind: Some(StreamErrorKind::Throttled),
            ..
        }
    ));

    let false_positive = classify_bedrock_exception("NotThrottlingException", "plain error");
    assert!(matches!(
        false_positive,
        AssistantMessageEvent::Error {
            error_kind: None,
            ..
        }
    ));
}

#[test]
fn build_request_uses_system_field() {
    let context = AgentContext::new(
        "You are a helpful assistant.".to_string(),
        vec![AgentMessage::Llm(LlmMessage::User(
            swink_agent::UserMessage::new(vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }])
            .with_timestamp(0),
        ))],
        vec![],
    );
    let options = StreamOptions::default();
    let request = build_request(&context, &options);
    assert!(request.system.is_some());
    assert_eq!(
        request.system.unwrap()[0].text,
        "You are a helpful assistant."
    );
    // Should NOT have system prompt as first user message
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, "user");
}

#[test]
fn parse_unknown_event_returns_none() {
    let mut state = BedrockStreamState::new();
    let events = parse_event_frame("someUnknownEvent", b"{}", &mut state).unwrap();
    assert!(events.is_none());
}

#[test]
fn guardrail_intervened_emits_error() {
    let mut state = BedrockStreamState::new();
    state.stop_reason = Some("guardrail_intervened".to_string());

    let payload = br#"{"usage":{"inputTokens":5,"outputTokens":0,"totalTokens":5}}"#;
    let events = parse_event_frame("metadata", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
}

/// Build a smithy event-stream `Message` with the given event-type header
/// and JSON payload. Used to simulate Bedrock ConverseStream frames.
fn make_event_message(event_type: &str, payload: &[u8]) -> aws_smithy_types::event_stream::Message {
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
    Message::new_from_parts(
        vec![
            Header::new(
                ":message-type",
                HeaderValue::String(String::from("event").into()),
            ),
            Header::new(
                ":event-type",
                HeaderValue::String(String::from(event_type).into()),
            ),
        ],
        bytes::Bytes::from(payload.to_vec()),
    )
}

/// Build a smithy exception `Message`.
fn make_exception_message(
    exception_type: &str,
    payload: &[u8],
) -> aws_smithy_types::event_stream::Message {
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
    Message::new_from_parts(
        vec![
            Header::new(
                ":message-type",
                HeaderValue::String(String::from("exception").into()),
            ),
            Header::new(
                ":exception-type",
                HeaderValue::String(String::from(exception_type).into()),
            ),
        ],
        bytes::Bytes::from(payload.to_vec()),
    )
}

#[test]
fn text_event_stream_parsing() {
    let mut state = BedrockStreamState::new();

    // 1. messageStart
    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], AssistantMessageEvent::Start));

    // 2. contentBlockStart (text)
    let msg = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));

    // 3. contentBlockDelta (text)
    let msg = make_event_message(
        "contentBlockDelta",
        br#"{"contentBlockIndex":0,"delta":{"type":"text","text":"Hello, world!"}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::TextDelta { content_index: 0, delta }
            if delta == "Hello, world!"
    ));

    // 4. contentBlockStop
    let msg = make_event_message("contentBlockStop", br#"{"contentBlockIndex":0}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));

    // 5. messageStop (no events emitted, captures stop_reason)
    let msg = make_event_message("messageStop", br#"{"stopReason":"end_turn"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert!(events.is_empty());
    assert_eq!(state.stop_reason.as_deref(), Some("end_turn"));

    // 6. metadata → Done
    let msg = make_event_message(
        "metadata",
        br#"{"usage":{"inputTokens":15,"outputTokens":25,"totalTokens":40}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage,
            ..
        } if usage.input == 15 && usage.output == 25 && usage.total == 40
    ));
}

#[test]
fn exception_frame_throttling_is_throttled() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("throttlingException", br#"{"message":"Rate exceeded"}"#);
    let events = process_smithy_message(&msg, &mut state);
    // Before Start: emits [Start, Error]
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Throttled));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_too_many_requests_is_throttled() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("tooManyRequestsException", br#"{"message":"Slow down"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Throttled));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_access_denied_is_auth() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("accessDeniedException", br#"{"message":"Not authorized"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_validation_is_auth() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("validationException", br#"{"message":"Invalid request"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_validation_input_too_long_is_context_overflow() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
        "validationException",
        br#"{"message":"Input is too long for requested model."}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                *error_kind,
                Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_validation_context_limit_is_context_overflow() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
            "validationException",
            br#"{"message":"input length and `max_tokens` exceed context limit: 199999 + 4096 > 200000"}"#,
        );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                *error_kind,
                Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_resource_not_found_is_auth() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
        "resourceNotFoundException",
        br#"{"message":"Model not found"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Auth));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_internal_server_is_network() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
        "internalServerException",
        br#"{"message":"Internal error"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_model_stream_error_is_network() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
        "modelStreamErrorException",
        br#"{"message":"Stream failed"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_model_timeout_is_network() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("modelTimeoutException", br#"{"message":"Timeout"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_service_unavailable_is_network() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message(
        "serviceUnavailableException",
        br#"{"message":"Service down"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn exception_frame_unknown_type_is_unclassified() {
    let mut state = BedrockStreamState::new();
    let msg = make_exception_message("someFutureException", br#"{"message":"Something new"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(
                *error_kind, None,
                "unknown exceptions should not be classified as retryable"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn guardrail_intervened_maps_to_content_filtered() {
    let mut state = BedrockStreamState::new();

    // messageStop with guardrail_intervened
    let msg = make_event_message("messageStop", br#"{"stopReason":"guardrail_intervened"}"#);
    let _ = process_smithy_message(&msg, &mut state);
    assert_eq!(state.stop_reason.as_deref(), Some("guardrail_intervened"));

    // metadata should emit error, not Done
    let msg = make_event_message(
        "metadata",
        br#"{"usage":{"inputTokens":5,"outputTokens":0,"totalTokens":5}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
}

#[test]
fn unexpected_eof_after_message_stop_is_network_error_not_done() {
    let mut state = BedrockStreamState::new();

    parse_event_frame("messageStart", br#"{"role":"assistant"}"#, &mut state).unwrap();
    parse_event_frame(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
        &mut state,
    )
    .unwrap();
    parse_event_frame(
        "contentBlockDelta",
        br#"{"contentBlockIndex":0,"delta":{"type":"text","text":"partial"}}"#,
        &mut state,
    )
    .unwrap();
    parse_event_frame(
        "contentBlockStop",
        br#"{"contentBlockIndex":0}"#,
        &mut state,
    )
    .unwrap();
    parse_event_frame("messageStop", br#"{"stopReason":"end_turn"}"#, &mut state).unwrap();

    let events = unexpected_eof_events(&mut state);

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "bare EOF must not synthesize a Done event"
    );
    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Error {
            error_kind: Some(swink_agent::StreamErrorKind::Network),
            ..
        })
    ));
}

#[test]
fn unexpected_eof_finalizes_open_blocks_before_error() {
    let mut state = BedrockStreamState::new();

    parse_event_frame("messageStart", br#"{"role":"assistant"}"#, &mut state).unwrap();
    parse_event_frame(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
        &mut state,
    )
    .unwrap();

    let events = unexpected_eof_events(&mut state);

    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::Network),
                ..
            }
        ]
    ));
}

#[test]
fn stream_finalize_closes_open_text_block() {
    use crate::finalize::OpenBlock;
    let mut state = BedrockStreamState::new();
    // Simulate opening a text block through the accumulator
    let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
    parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
    let blocks = state.drain_open_blocks();
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], OpenBlock::Text { content_index: 0 }));
}

#[test]
fn stream_finalize_closes_open_tool_block() {
    use crate::finalize::OpenBlock;
    let mut state = BedrockStreamState::new();
    // Simulate opening a tool-call block through the accumulator
    let payload =
        br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_1","name":"tool"}}"#;
    parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
    let blocks = state.drain_open_blocks();
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        blocks[0],
        OpenBlock::ToolCall { content_index: 0 }
    ));
}

#[test]
fn stream_finalize_empty_when_no_open_blocks() {
    let mut state = BedrockStreamState::new();
    let blocks = state.drain_open_blocks();
    assert!(blocks.is_empty());
}

#[test]
fn stream_finalize_drains_multiple_open_blocks() {
    use crate::finalize::OpenBlock;
    let mut state = BedrockStreamState::new();
    // Open text block (provider index 0)
    let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
    parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
    // Close text block
    let payload = br#"{"contentBlockIndex":0}"#;
    parse_event_frame("contentBlockStop", payload, &mut state).unwrap();
    // Open two tool-call blocks (provider indices 1 and 2)
    let payload =
        br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_1","name":"a"}}"#;
    parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
    let payload =
        br#"{"contentBlockIndex":2,"start":{"type":"toolUse","toolUseId":"tc_2","name":"b"}}"#;
    parse_event_frame("contentBlockStart", payload, &mut state).unwrap();
    // Drain without closing — both tool calls should be drained
    let blocks = state.drain_open_blocks();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        blocks[0],
        OpenBlock::ToolCall { content_index: 1 }
    ));
    assert!(matches!(
        blocks[1],
        OpenBlock::ToolCall { content_index: 2 }
    ));
}

#[test]
fn content_indices_are_sequential_across_block_types() {
    let mut state = BedrockStreamState::new();
    // Open text (harness index 0)
    let payload = br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#;
    let events = parse_event_frame("contentBlockStart", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
    // Close text
    let payload = br#"{"contentBlockIndex":0}"#;
    parse_event_frame("contentBlockStop", payload, &mut state).unwrap();
    // Open tool (harness index 1)
    let payload =
        br#"{"contentBlockIndex":1,"start":{"type":"toolUse","toolUseId":"tc_1","name":"t"}}"#;
    let events = parse_event_frame("contentBlockStart", payload, &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            ..
        }
    ));
}

#[test]
fn tool_call_event_stream_parsing() {
    let mut state = BedrockStreamState::new();

    // 1. messageStart
    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], AssistantMessageEvent::Start));

    // 2. contentBlockStart (toolUse)
    let msg = make_event_message(
            "contentBlockStart",
            br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_abc123","name":"get_weather"}}"#,
        );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallStart { content_index: 0, id, name }
            if id == "tc_abc123" && name == "get_weather"
    ));

    // 3. contentBlockDelta (toolUse) — partial JSON
    let msg = make_event_message(
        "contentBlockDelta",
        br#"{"contentBlockIndex":0,"delta":{"type":"toolUse","input":"{\"city\""}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
            if delta == r#"{"city""#
    ));

    // 4. contentBlockDelta (toolUse) — rest of JSON
    let msg = make_event_message(
        "contentBlockDelta",
        br#"{"contentBlockIndex":0,"delta":{"type":"toolUse","input":": \"Paris\"}"}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallDelta { content_index: 0, delta }
            if delta == r#": "Paris"}"#
    ));

    // 5. contentBlockStop
    let msg = make_event_message("contentBlockStop", br#"{"contentBlockIndex":0}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 0 }
    ));

    // 6. messageStop (tool_use stop reason)
    let msg = make_event_message("messageStop", br#"{"stopReason":"tool_use"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert!(events.is_empty());
    assert_eq!(state.stop_reason.as_deref(), Some("tool_use"));

    // 7. metadata → Done with ToolUse stop reason
    let msg = make_event_message(
        "metadata",
        br#"{"usage":{"inputTokens":50,"outputTokens":30,"totalTokens":80}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage,
            ..
        } if usage.input == 50 && usage.output == 30 && usage.total == 80
    ));
}

#[test]
fn exception_after_start_does_not_duplicate_start() {
    let mut state = BedrockStreamState::new();

    // Simulate messageStart already received
    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(state.started);

    // Exception after Start should emit only Error, no second Start
    let msg = make_exception_message(
        "modelStreamErrorException",
        br#"{"message":"Stream failed mid-response"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(
        events.len(),
        1,
        "should not prepend Start when already started"
    );
    assert!(matches!(events[0], AssistantMessageEvent::Error { .. }));
}

#[test]
fn exception_after_open_text_block_closes_block_before_error() {
    let mut state = BedrockStreamState::new();

    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
    );
    let _ = process_smithy_message(&msg, &mut state);

    let msg = make_exception_message(
        "modelStreamErrorException",
        br#"{"message":"Stream failed mid-text"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);

    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::Network),
                ..
            }
        ]
    ));
}

#[test]
fn exception_after_open_tool_block_closes_block_before_error() {
    let mut state = BedrockStreamState::new();

    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_1","name":"lookup"}}"#,
    );
    let _ = process_smithy_message(&msg, &mut state);

    let msg = make_exception_message(
        "modelTimeoutException",
        br#"{"message":"Stream timed out mid-tool"}"#,
    );
    let events = process_smithy_message(&msg, &mut state);

    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::Network),
                ..
            }
        ]
    ));
}

#[test]
fn metadata_done_closes_open_blocks_before_terminal_event() {
    let mut state = BedrockStreamState::new();

    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
    );
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message("messageStop", br#"{"stopReason":"end_turn"}"#);
    let _ = process_smithy_message(&msg, &mut state);

    let msg = make_event_message(
        "metadata",
        br#"{"usage":{"inputTokens":15,"outputTokens":25,"totalTokens":40}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);

    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                ..
            }
        ]
    ));
}

#[test]
fn metadata_error_closes_open_tool_block_before_terminal_event() {
    let mut state = BedrockStreamState::new();

    let msg = make_event_message("messageStart", br#"{"role":"assistant"}"#);
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"toolUse","toolUseId":"tc_1","name":"lookup"}}"#,
    );
    let _ = process_smithy_message(&msg, &mut state);
    let msg = make_event_message("messageStop", br#"{"stopReason":"guardrail_intervened"}"#);
    let _ = process_smithy_message(&msg, &mut state);

    let msg = make_event_message(
        "metadata",
        br#"{"usage":{"inputTokens":15,"outputTokens":0,"totalTokens":15}}"#,
    );
    let events = process_smithy_message(&msg, &mut state);

    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Error { .. }
        ]
    ));
}

#[test]
fn message_start_sets_started_flag() {
    let mut state = BedrockStreamState::new();
    assert!(!state.started);

    let payload = br#"{"role":"assistant"}"#;
    let _ = parse_event_frame("messageStart", payload, &mut state);
    assert!(state.started);
}

#[test]
fn malformed_known_event_emits_terminal_error() {
    let mut state = BedrockStreamState::new();
    let msg = make_event_message("metadata", br#"{"usage":"bad"}"#);
    let events = process_smithy_message(&msg, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error {
            error_message,
            error_kind: None,
            ..
        } if error_message.contains("Bedrock metadata parse error")
    ));
}

#[test]
fn pre_start_terminal_error_is_prefixed() {
    let mut started = false;
    let events = prefix_pre_start_terminal_error(
        vec![AssistantMessageEvent::error_network("boom")],
        &mut started,
    );

    assert!(started);
    assert!(matches!(
        events.as_slice(),
        [
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error { .. }
        ]
    ));
}

#[test]
fn malformed_known_event_drains_open_blocks_before_error() {
    let mut state = BedrockStreamState::new();
    let start = make_event_message(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
    );
    let events = process_smithy_message(&start, &mut state);
    assert!(matches!(
        events.as_slice(),
        [AssistantMessageEvent::TextStart { content_index: 0 }]
    ));

    let bad_delta = make_event_message(
        "contentBlockDelta",
        br#"{"contentBlockIndex":0,"delta":"bad"}"#,
    );
    let events = process_smithy_message(&bad_delta, &mut state);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error { error_message, .. }
            if error_message.contains("Bedrock contentBlockDelta parse error")
    ));
}

// ── Issue #619: incomplete tool_use sanitization ─────────────────────

/// Regression for #619: after the loop-level scrub runs, an assistant
/// message that originally carried `arguments: Null` with `partial_json`
/// set must serialize with `toolUse.input: {}` so Bedrock Converse accepts
/// the replayed history on the next turn.
#[test]
fn convert_messages_sanitized_tool_use_becomes_empty_object_input() {
    use swink_agent::AssistantMessage as HarnessAssistantMessage;

    let mut assistant = HarnessAssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "tooluse_abc".into(),
            name: "read_file".into(),
            arguments: serde_json::Value::Null,
            partial_json: Some(r#"{"path": "/tm"#.into()),
        }],
        "bedrock",
        "anthropic.claude-3-sonnet",
    )
    .with_stop_reason(StopReason::Length)
    .with_timestamp(0);

    swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

    let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(assistant))];
    let converted = convert_messages(&messages);

    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].role, "assistant");
    let json = serde_json::to_value(&converted[0]).unwrap();
    let block = &json["content"][0];
    let input = &block["toolUse"]["input"];
    assert!(
        input.is_object(),
        "toolUse.input must be a JSON object, got {input:?}"
    );
    assert_eq!(input.as_object().unwrap().len(), 0);
}
