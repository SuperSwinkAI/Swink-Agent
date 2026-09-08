//! Tests for `google`.
#![cfg(test)]

use super::*;

/// Regression for #619: after the loop-level scrub runs, an assistant
/// message that originally carried `arguments: Null` with `partial_json`
/// set must serialize with `args: {}` so the Gemini API accepts the
/// replayed history on the next turn.
#[test]
fn convert_messages_sanitized_tool_use_becomes_empty_object_args() {
    let mut assistant = HarnessAssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: Value::Null,
            partial_json: Some(r#"{"path": "/tm"#.into()),
        }],
        "google",
        "gemini-2.0-flash",
    )
    .with_stop_reason(StopReason::Length)
    .with_timestamp(0);

    swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

    let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(assistant))];
    let converted = convert_messages(&messages);

    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].role, "model");
    let json = serde_json::to_value(&converted[0]).unwrap();
    let part = &json["parts"][0];
    let args = &part["functionCall"]["args"];
    assert!(
        args.is_object(),
        "functionCall.args must be a JSON object, got {args:?}"
    );
    assert_eq!(args.as_object().unwrap().len(), 0);
}

#[test]
fn terminal_parse_error_flushes_final_tool_delta_before_generic_error() {
    let mut state = GeminiStreamState::default();
    let (content_index, _) = state
        .blocks
        .open_tool_call("call_1".into(), "read_file".into());
    state.tool_calls.insert(
        "provider:call_1".into(),
        GeminiToolCallState {
            content_index,
            name: "read_file".into(),
            arguments: r#"{"path":"foo.rs"}"#.into(),
        },
    );

    let events = state.emit_terminal_error(
        AssistantMessageEvent::error("Google JSON parse error: bad payload"),
        true,
    );
    let delta_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
        .expect("final tool delta");
    let end_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
        .expect("tool call end");
    let error_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
        .expect("terminal error");

    assert!(
        delta_index < end_index && end_index < error_index,
        "pending tool-call state must flush before the terminal error: {events:?}"
    );
    match &events[error_index] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert!(
                error_kind.is_none(),
                "JSON parse errors must be non-retryable protocol errors"
            );
        }
        event => panic!("expected terminal error, got {event:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "terminal error path must not emit Done"
    );
}

#[tokio::test]
async fn pre_cancelled_stream_aborts_before_request_send() {
    let gemini = GeminiStreamFn::new("http://127.0.0.1:1", "api-key", ApiVersion::V1beta);
    let model = ModelSpec::new("google", "gemini-2.0-flash");
    let context = AgentContext::new(String::new(), vec![], vec![]);
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let events: Vec<_> = gemini
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
