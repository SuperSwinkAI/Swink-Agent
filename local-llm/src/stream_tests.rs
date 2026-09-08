//! Tests for `stream`.
#![cfg(test)]

use super::delimiter::partial_prefix_at_end;
use super::think_tags::ThinkTagParser;
use super::*;

fn local_message(role: &str, content: &str) -> crate::convert::LocalMessage {
    crate::convert::LocalMessage {
        role: role.to_string(),
        content: content.to_string(),
    }
}

#[allow(clippy::unnecessary_wraps)]
fn content_token_count(
    messages: &[crate::convert::LocalMessage],
) -> Result<usize, std::convert::Infallible> {
    Ok(messages
        .iter()
        .map(|message| message.content.split_whitespace().count())
        .sum())
}

#[test]
fn prompt_token_budget_reserves_generation_room() {
    assert_eq!(prompt_token_budget(0), 0);
    assert_eq!(prompt_token_budget(1), 1);
    assert_eq!(prompt_token_budget(4), 3);
}

#[test]
fn thinking_enabled_requires_capability_and_non_off_level() {
    let mut model = ModelSpec::new("local", "gemma-4-E2B-it");
    model.capabilities = Some(swink_agent::ModelCapabilities::none().with_thinking(true));

    assert!(!thinking_enabled_for_model(&model));

    model.thinking_level = ThinkingLevel::Low;
    assert!(thinking_enabled_for_model(&model));

    model.capabilities = Some(swink_agent::ModelCapabilities::none().with_thinking(false));
    assert!(!thinking_enabled_for_model(&model));
}

#[test]
fn catalog_local_thinking_model_thinks_by_default() {
    let preset = swink_agent::model_catalog()
        .preset("local", "gemma4_e2b")
        .expect("gemma4_e2b must exist in the model catalog");
    let model = preset.model_spec();

    assert!(
        thinking_enabled_for_model(&model),
        "catalog-built thinking-capable local models must think by default"
    );

    let disabled = model.with_thinking_level(ThinkingLevel::Off);
    assert!(
        !thinking_enabled_for_model(&disabled),
        "explicitly setting ThinkingLevel::Off must disable thinking"
    );
}

#[test]
fn context_truncation_keeps_messages_when_prompt_fits() {
    let messages = vec![
        local_message("system", "sys"),
        local_message("user", "hello"),
        local_message("assistant", "hi"),
    ];

    let truncated = truncate_messages_to_context(messages.clone(), 8, content_token_count).unwrap();

    assert_eq!(truncated, messages);
}

#[test]
fn context_truncation_drops_oldest_non_system_messages() {
    let messages = vec![
        local_message("system", "sys"),
        local_message("user", "old user"),
        local_message("assistant", "old assistant"),
        local_message("user", "recent user"),
    ];

    let truncated = truncate_messages_to_context(messages, 3, content_token_count).unwrap();

    assert_eq!(
        truncated,
        vec![
            local_message("system", "sys"),
            local_message("user", "recent user"),
        ]
    );
}

#[test]
fn context_truncation_removes_orphaned_leading_tool_results() {
    let messages = vec![
        local_message("system", "sys"),
        local_message("user", "old user"),
        local_message("assistant", "called tool"),
        local_message("tool", "tool result"),
        local_message("user", "recent"),
    ];

    let truncated = truncate_messages_to_context(messages, 2, content_token_count).unwrap();

    assert_eq!(
        truncated,
        vec![
            local_message("system", "sys"),
            local_message("user", "recent"),
        ]
    );
}

#[tokio::test]
async fn token_stream_cancellation_wins_while_waiting_for_runner_event() {
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let token = CancellationToken::new();
    let stream = drain_token_stream(rx, &token, false);
    tokio::pin!(stream);

    std::future::poll_fn(|cx| match stream.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(events) => {
            panic!("quiet runner channel resolved before cancellation: {events:?}")
        }
    })
    .await;
    token.cancel();

    let events = tokio::time::timeout(std::time::Duration::from_secs(1), stream)
        .await
        .expect("cancellation should interrupt a quiet runner channel");

    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn token_stream_cancellation_beats_ready_buffered_event() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(TokenEvent::Done {
        prompt_tokens: 1,
        completion_tokens: 0,
        finish_reason: FinishReason::Stop,
    })
    .await
    .unwrap();
    let token = CancellationToken::new();
    token.cancel();

    let events = drain_token_stream(rx, &token, false).await;

    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        })
    ));
}

#[test]
fn think_tag_single_chunk() {
    let mut parser = ThinkTagParser::new();
    let (thinking, text) =
        parser.process("<think>I need to reason about this.</think>The answer is 42.");
    assert_eq!(thinking.as_deref(), Some("I need to reason about this."));
    assert_eq!(text.as_deref(), Some("The answer is 42."));
}

#[test]
fn think_tag_no_tags() {
    let mut parser = ThinkTagParser::new();
    let (thinking, text) = parser.process("Hello, world!");
    assert!(thinking.is_none());
    assert_eq!(text.as_deref(), Some("Hello, world!"));
}

#[test]
fn think_tag_empty_tags() {
    let mut parser = ThinkTagParser::new();
    let (thinking, text) = parser.process("<think></think>Just text.");
    assert!(thinking.is_none());
    assert_eq!(text.as_deref(), Some("Just text."));
}

#[test]
fn think_tag_with_content_before() {
    let mut parser = ThinkTagParser::new();
    let (thinking, text) = parser.process("Before <think>reasoning</think> after");
    assert_eq!(thinking.as_deref(), Some("reasoning"));
    assert_eq!(text.as_deref(), Some("Before  after"));
}

#[test]
fn think_tag_cross_chunk_open_and_close() {
    let mut parser = ThinkTagParser::new();
    let (t1, txt1) = parser.process("<th");
    assert!(t1.is_none());
    assert!(txt1.is_none());

    let (t2, txt2) = parser.process("ink>reason");
    assert_eq!(t2.as_deref(), Some("reason"));
    assert!(txt2.is_none());

    let (t3, txt3) = parser.process("ing</th");
    assert_eq!(t3.as_deref(), Some("ing"));
    assert!(txt3.is_none());

    let (t4, txt4) = parser.process("ink> after");
    assert!(t4.is_none());
    assert_eq!(txt4.as_deref(), Some(" after"));
}

#[test]
fn think_tag_cross_chunk_with_text_before_open() {
    let mut parser = ThinkTagParser::new();
    let (t1, txt1) = parser.process("Before <thi");
    assert!(t1.is_none());
    assert_eq!(txt1.as_deref(), Some("Before "));

    let (t2, txt2) = parser.process("nk>reasoning</think> after");
    assert_eq!(t2.as_deref(), Some("reasoning"));
    assert_eq!(txt2.as_deref(), Some(" after"));
}

#[test]
fn think_tag_partial_match_is_utf8_safe() {
    let haystack = "alpha🙂<thi";
    assert_eq!(partial_prefix_at_end(haystack, "<think>"), Some(4));

    let mut parser = ThinkTagParser::new();
    let (t1, txt1) = parser.process("alpha🙂<thi");
    assert!(t1.is_none());
    assert_eq!(txt1.as_deref(), Some("alpha🙂"));

    let (t2, txt2) = parser.process("nk>reasoning</think>");
    assert_eq!(t2.as_deref(), Some("reasoning"));
    assert!(txt2.is_none());
}

#[test]
fn default_tool_call_single_chunk() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls, text) = parser.process(r#"call:read_file{"path":"foo.rs"}"#);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].args, r#"{"path":"foo.rs"}"#);
    assert!(text.is_none());
    assert!(parser.finish(false).is_none());
}

#[test]
fn default_tool_call_cross_chunk() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls1, text1) = parser.process(r"call:read_");
    assert!(calls1.is_empty());
    assert!(text1.is_none());

    let (calls2, text2) = parser.process(r#"file{"path":"foo.rs"} trailing"#);
    assert_eq!(calls2.len(), 1);
    assert_eq!(calls2[0].name, "read_file");
    assert_eq!(calls2[0].args, r#"{"path":"foo.rs"}"#);
    assert_eq!(text2.as_deref(), Some(" trailing"));
}

#[test]
fn default_tool_call_handles_nested_json_and_strings() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls, text) =
        parser.process(r#"prefix call:write_file{"text":"{\"k\":1}","meta":{"n":1}}"#);

    assert_eq!(text.as_deref(), Some("prefix "));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(calls[0].args, r#"{"text":"{\"k\":1}","meta":{"n":1}}"#);
}

#[test]
fn default_tool_call_invalid_shape_remains_text() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls, text) = parser.process("please call: read_file next");

    assert!(calls.is_empty());
    assert_eq!(text.as_deref(), Some("please call: read_file next"));
    assert!(parser.finish(false).is_none());
}

#[test]
fn default_tool_call_incomplete_flushes_as_text_on_finalize() {
    let mut state = StreamState::new(false);
    state.process_token("Before call:read_file{\"path\"");

    let events = state.finalize();
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(text_deltas, vec!["Before ", "call:read_file{\"path\""]);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::ToolCallStart { .. }))
    );
}

#[test]
fn default_tool_call_finish_preserves_length_truncated_call() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls, text) = parser.process(r#"call:read_file{"path""#);
    assert!(calls.is_empty());
    assert!(text.is_none());

    match parser.finish(true) {
        Some(default_tool_call::ToolCallFinish::PartialToolCall { name, args }) => {
            assert_eq!(name, "read_file");
            assert_eq!(args, r#"{"path""#);
        }
        other => panic!("expected partial tool call, got {other:?}"),
    }
}

#[test]
fn default_tool_call_finish_flushes_incomplete_call_as_text_by_default() {
    let mut parser = default_tool_call::ToolCallParser::new();
    let (calls, text) = parser.process(r#"call:read_file{"path""#);
    assert!(calls.is_empty());
    assert!(text.is_none());

    match parser.finish(false) {
        Some(default_tool_call::ToolCallFinish::Text(text)) => {
            assert_eq!(text, r#"call:read_file{"path""#);
        }
        other => panic!("expected text flush, got {other:?}"),
    }
}

#[test]
fn default_finalize_preserves_length_truncated_tool_call() {
    let mut state = StreamState::new(false);
    state.finish_reason = FinishReason::Length;
    state.process_token(r#"call:read_file{"path""#);

    let events = state.finalize();

    assert!(events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::ToolCallStart { name, .. } if name == "read_file"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::ToolCallDelta { delta, .. } if delta == r#"{"path""#
    )));
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::TextDelta { delta, .. } if delta == r#"call:read_file{"path""#
        )),
        "truncated tool call should not be emitted as assistant text: {events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            ..
        })
    ));
}

#[test]
fn non_gemma_stream_state_emits_tool_call_events() {
    let mut state = StreamState::new(false);
    state.process_token(r#"I'll inspect that. call:read_file{"path":"foo.rs"}"#);

    let events = state.finalize();

    assert!(
        events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::ToolCallStart { name, .. } if name == "read_file"
        )),
        "expected tool call start in events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::ToolCallDelta { delta, .. } if delta == r#"{"path":"foo.rs"}"#
        )),
        "expected tool call delta in events: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. })),
        "expected tool call end in events: {events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        })
    ));
}

#[test]
fn finalize_cancelled_emits_error_terminal() {
    let mut state = StreamState::new(false);
    let start = state.blocks.ensure_text_open();
    state.events.extend(start);

    let events = state.finalize_cancelled();
    let terminal = events.last().expect("at least one event");
    match terminal {
        AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Aborted);
            assert!(
                error_message.contains("cancelled"),
                "expected cancellation message, got: {error_message}"
            );
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. }))
    );
}

#[test]
fn cancelled_event_uses_aborted_stop_reason() {
    let event = cancelled_event("local inference cancelled");

    match event {
        AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
            error_kind,
            retry_after: _,
        } => {
            assert_eq!(stop_reason, StopReason::Aborted);
            assert_eq!(error_message, "local inference cancelled");
            assert!(usage.is_none());
            assert!(error_kind.is_none());
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
}

#[tokio::test]
async fn race_pre_stream_cancellation_short_circuits() {
    let token = CancellationToken::new();
    token.cancel();

    let result =
        race_pre_stream_cancellation(&token, async { Ok::<_, AssistantMessageEvent>("ok") }).await;

    assert!(matches!(
        result,
        Err(AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn race_pre_stream_cancellation_aborts_in_flight_readiness() {
    let token = CancellationToken::new();
    let readiness_started = Arc::new(tokio::sync::Notify::new());
    let cancel_token = token.clone();
    let cancel_on_readiness = Arc::clone(&readiness_started);
    tokio::spawn(async move {
        cancel_on_readiness.notified().await;
        cancel_token.cancel();
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        race_pre_stream_cancellation(&token, async {
            readiness_started.notify_one();
            std::future::pending::<Result<&str, AssistantMessageEvent>>().await
        }),
    )
    .await
    .expect("cancellation should resolve promptly");

    assert!(matches!(
        result,
        Err(AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn pre_cancelled_stream_short_circuits_before_model_ready() {
    let local_model = Arc::new(LocalModel::from_preset(crate::ModelPreset::SmolLM3_3B));
    let stream_fn = LocalStreamFn::new(local_model);
    let model = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
    let context = AgentContext::new(String::new(), vec![], vec![]);
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let events: Vec<_> = stream_fn
        .stream(&model, &context, &options, token)
        .collect()
        .await;

    assert_eq!(
        events.len(),
        2,
        "expected start + terminal abort: {events:?}"
    );
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
        other => panic!("expected cancellation terminal event, got {other:?}"),
    }
}

#[test]
fn finalize_error_closes_open_blocks_before_terminal_error() {
    let mut state = StreamState::new(false);
    state.events.extend(state.blocks.ensure_text_open());
    state
        .events
        .extend(state.blocks.text_delta("partial".to_string()));

    let events = state.finalize_error("local inference error: runner crashed");
    let terminal_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
        .expect("terminal error event");
    let text_end_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::TextEnd { .. }))
        .expect("text end event");

    assert!(
        text_end_index < terminal_index,
        "open blocks must be finalized before the terminal error: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "terminal error path must not emit Done"
    );
}

#[test]
fn finalize_flushes_pending_partial_open_as_text() {
    let mut state = StreamState::new(false);
    state.process_token("Before <thi");

    let events = state.finalize();
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["Before ", "<thi"]);
}

#[test]
fn finalize_flushes_unclosed_thinking_buffer() {
    let mut state = StreamState::new(false);
    state.process_token("<think>reasoning");

    let events = state.finalize();
    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking_deltas, vec!["reasoning"]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::ThinkingEnd { .. }))
    );
}

#[test]
fn finalize_keeps_tool_use_stop_reason() {
    let mut state = StreamState::new(false);
    state.has_tool_calls = true;
    state.finish_reason = FinishReason::Stop;

    let events = state.finalize();
    let terminal = events.last().expect("at least one event");
    match terminal {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[test]
fn finalize_preserves_length_stop_reason_over_tool_use() {
    let mut state = StreamState::new(false);
    state.has_tool_calls = true;
    state.finish_reason = FinishReason::Length;

    let events = state.finalize();
    let terminal = events.last().expect("at least one event");
    match terminal {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::Length);
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[test]
fn finalize_eof_without_done_emits_error_terminal() {
    let mut state = StreamState::new(false);
    let start = state.blocks.ensure_text_open();
    state.events.extend(start);
    state
        .events
        .extend(state.blocks.text_delta("partial".to_string()));

    assert!(!state.saw_done);

    let events = state.finalize_eof_without_done();
    let terminal = events.last().expect("at least one event");
    match terminal {
        AssistantMessageEvent::Error { error_message, .. } => {
            assert!(
                error_message.contains("ended before completion"),
                "expected EOF error message, got: {error_message}"
            );
        }
        other => panic!("expected Error terminal, got {other:?}"),
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );
}

#[test]
fn usage_tracking() {
    let mut state = StreamState::new(false);
    state.prompt_tokens = 42;
    state.completion_tokens = 13;
    state.saw_done = true;

    let events = state.finalize();
    let terminal = events.last().expect("at least one event");
    match terminal {
        AssistantMessageEvent::Done { usage, .. } => {
            assert_eq!(usage.input, 42);
            assert_eq!(usage.output, 13);
            assert_eq!(usage.total, 55);
        }
        other => panic!("expected Done terminal, got {other:?}"),
    }
}

#[test]
fn stream_options_forward_generation_overrides() {
    let options = StreamOptions::default()
        .with_temperature(0.8)
        .with_max_tokens(256);

    assert_eq!(
        generation_options_from_stream_options(&options),
        GenerateOptions {
            temperature: Some(0.8_f32),
            max_tokens: Some(256),
        }
    );
}

#[test]
fn stream_options_max_tokens_saturates_to_u32() {
    let options = StreamOptions::default().with_max_tokens(u64::MAX);

    assert_eq!(
        generation_options_from_stream_options(&options).max_tokens,
        Some(u32::MAX)
    );
}

#[cfg(feature = "gemma4")]
mod gemma4_tests {
    use super::super::channel_thought::ChannelThoughtParser;
    use super::super::delimiter::partial_prefix_at_end;
    use super::*;

    #[test]
    fn channel_thought_single_chunk() {
        let mut parser = ChannelThoughtParser::new();
        let (thinking, text) = parser.process("<|channel>thought\nreasoning here<channel|>");
        assert_eq!(thinking.as_deref(), Some("reasoning here"));
        assert!(text.is_none());
    }

    #[test]
    fn channel_thought_cross_chunk_open() {
        let mut parser = ChannelThoughtParser::new();
        let (t1, txt1) = parser.process("<|channel>");
        assert!(t1.is_none());
        assert!(txt1.is_none());

        let (t2, txt2) = parser.process("thought\nthinking content<channel|>");
        assert_eq!(t2.as_deref(), Some("thinking content"));
        assert!(txt2.is_none());
    }

    #[test]
    fn channel_thought_cross_chunk_close() {
        let mut parser = ChannelThoughtParser::new();
        let (t1, txt1) = parser.process("<|channel>thought\nsome reasoning<chan");
        assert_eq!(t1.as_deref(), Some("some reasoning"));
        assert!(txt1.is_none());

        let (t2, txt2) = parser.process("nel|>after text");
        assert!(t2.is_none());
        assert_eq!(txt2.as_deref(), Some("after text"));
    }

    #[test]
    fn channel_thought_finish_flushes_unclosed_thinking() {
        let mut parser = ChannelThoughtParser::new();
        let (thinking, text) = parser.process("<|channel>thought\nreasoning<chan");
        assert_eq!(thinking.as_deref(), Some("reasoning"));
        assert!(text.is_none());

        let (thinking, text) = parser.finish();
        assert_eq!(thinking.as_deref(), Some("<chan"));
        assert!(text.is_none());
    }

    #[test]
    fn channel_thought_partial_match_is_utf8_safe() {
        let haystack = "alpha🙂<|chan";
        assert_eq!(
            partial_prefix_at_end(haystack, "<|channel>thought\n"),
            Some(6)
        );

        let mut parser = ChannelThoughtParser::new();
        let (t1, txt1) = parser.process("alpha🙂<|chan");
        assert!(t1.is_none());
        assert_eq!(txt1.as_deref(), Some("alpha🙂"));

        let (t2, txt2) = parser.process("nel>thought\nreasoning<channel|>");
        assert_eq!(t2.as_deref(), Some("reasoning"));
        assert!(txt2.is_none());
    }

    #[test]
    fn channel_thought_no_delimiters() {
        let mut parser = ChannelThoughtParser::new();
        let (thinking, text) = parser.process("Hello, world!");
        assert!(thinking.is_none());
        assert_eq!(text.as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn channel_thought_multiple_blocks() {
        let mut parser = ChannelThoughtParser::new();
        let input = "<|channel>thought\nfirst<channel|><|channel>thought\nsecond<channel|>";
        let (thinking, text) = parser.process(input);
        assert_eq!(thinking.as_deref(), Some("firstsecond"));
        assert!(text.is_none());
    }

    #[test]
    fn channel_thought_mixed_text_and_thinking() {
        let mut parser = ChannelThoughtParser::new();

        let (t1, txt1) = parser.process("before ");
        assert!(t1.is_none());
        assert_eq!(txt1.as_deref(), Some("before "));

        let (t2, txt2) = parser.process("<|channel>thought\nreasoning<channel|>");
        assert_eq!(t2.as_deref(), Some("reasoning"));
        assert!(txt2.is_none());

        let (t3, txt3) = parser.process(" after");
        assert!(t3.is_none());
        assert_eq!(txt3.as_deref(), Some(" after"));
    }

    #[test]
    fn tool_call_single_chunk() {
        use super::super::tool_call::ToolCallParser;
        let mut parser = ToolCallParser::new();
        let (calls, text) =
            parser.process(r#"<|tool_call>call:read_file{"path":"foo.rs"}<tool_call|>"#);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].args, r#"{"path":"foo.rs"}"#);
        assert!(text.is_none());
    }

    #[test]
    fn tool_call_cross_chunk() {
        use super::super::tool_call::ToolCallParser;
        let mut parser = ToolCallParser::new();
        let (calls1, text1) =
            parser.process(r#"<|tool_call>call:read_file{"path":"foo.rs"}<tool_call"#);
        assert!(calls1.is_empty());
        assert!(text1.is_none());

        let (calls2, text2) = parser.process("|>");
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].name, "read_file");
        assert_eq!(calls2[0].args, r#"{"path":"foo.rs"}"#);
        assert!(text2.is_none());
    }

    #[test]
    fn tool_call_finish_flushes_incomplete_call_as_text_by_default() {
        use super::super::tool_call::{ToolCallFinish, ToolCallParser};

        let mut parser = ToolCallParser::new();
        let (calls, text) = parser.process(r#"<|tool_call>call:read_file{"path""#);
        assert!(calls.is_empty());
        assert!(text.is_none());

        match parser.finish(false) {
            Some(ToolCallFinish::Text(text)) => {
                assert_eq!(text, r#"<|tool_call>call:read_file{"path""#);
            }
            other => panic!("expected text flush, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_finish_preserves_length_truncated_call() {
        use super::super::tool_call::{ToolCallFinish, ToolCallParser};

        let mut parser = ToolCallParser::new();
        let (calls, text) = parser.process(r#"<|tool_call>call:read_file{"path""#);
        assert!(calls.is_empty());
        assert!(text.is_none());

        match parser.finish(true) {
            Some(ToolCallFinish::PartialToolCall { name, args }) => {
                assert_eq!(name, "read_file");
                assert_eq!(args, r#"{"path""#);
            }
            other => panic!("expected partial tool call, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_partial_match_is_utf8_safe() {
        use super::super::tool_call::ToolCallParser;

        let haystack = r"prefix🙂<tool_cal";
        assert_eq!(partial_prefix_at_end(haystack, "<tool_call|>"), Some(9));

        let mut parser = ToolCallParser::new();
        let (calls1, text1) =
            parser.process(r#"prefix🙂<|tool_call>call:read_file{"path":"foo.rs"}<tool_cal"#);
        assert!(calls1.is_empty());
        assert_eq!(text1.as_deref(), Some("prefix🙂"));

        let (calls2, text2) = parser.process("l|>");
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].name, "read_file");
        assert_eq!(calls2[0].args, r#"{"path":"foo.rs"}"#);
        assert!(text2.is_none());
    }

    #[test]
    fn tool_call_no_delimiters() {
        use super::super::tool_call::ToolCallParser;
        let mut parser = ToolCallParser::new();
        let (calls, text) = parser.process("Hello, world!");
        assert!(calls.is_empty());
        assert_eq!(text.as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn tool_call_with_thinking() {
        let mut think_parser = ChannelThoughtParser::new();
        let input = r#"<|channel>thought
reasoning<channel|><|tool_call>call:read_file{"path":"foo.rs"}<tool_call|>"#;
        let (thinking, text_opt) = think_parser.process(input);
        assert_eq!(thinking.as_deref(), Some("reasoning"));

        let text = text_opt.expect("tool call text must follow thinking block");
        use super::super::tool_call::ToolCallParser;
        let mut tool_parser = ToolCallParser::new();
        let (calls, remaining) = tool_parser.process(&text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert!(remaining.as_ref().is_none_or(|s| s.is_empty()));
    }

    #[test]
    fn gemma_finalize_flushes_unclosed_thinking() {
        let mut state = StreamState::new(true);
        state.process_token("<|channel>thought\nreasoning<chan");

        let events = state.finalize();
        let thinking_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(thinking_deltas, vec!["reasoning", "<chan"]);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::ThinkingEnd { .. }))
        );
    }

    #[test]
    fn gemma_finalize_flushes_incomplete_tool_call_as_text_on_stop() {
        let mut state = StreamState::new(true);
        state.process_token(r#"<|tool_call>call:read_file{"path""#);

        let events = state.finalize();
        let text_deltas: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(text_deltas, vec![r#"<|tool_call>call:read_file{"path""#]);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::ToolCallStart { .. }))
        );
    }

    #[test]
    fn gemma_finalize_preserves_length_truncated_tool_call() {
        let mut state = StreamState::new(true);
        state.finish_reason = FinishReason::Length;
        state.process_token(r#"<|tool_call>call:read_file{"path""#);

        let events = state.finalize();

        assert!(events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::ToolCallStart { name, .. } if name == "read_file"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::ToolCallDelta { delta, .. } if delta == r#"{"path""#
        )));
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Done {
                stop_reason: StopReason::Length,
                ..
            })
        ));
    }

    #[test]
    fn channel_thought_delimiter_in_text() {
        let mut parser = ChannelThoughtParser::new();
        let (thinking, text) = parser.process("The format is <|channel>thought end");
        assert!(thinking.is_none());
        assert!(text.is_some());
    }
}
