//! Tests for `stream`.
#![cfg(test)]

use super::*;

// ── RateLimitSnapshot ────────────────────────────────────────────────

#[test]
fn rate_limit_snapshot_parses_codex_subscription_headers() {
    // Captured live from the Codex endpoint (issue #1264).
    let snapshot = RateLimitSnapshot::from_headers([
        ("x-codex-plan-type", "prolite"),
        ("X-Codex-Primary-Used-Percent", "98"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-primary-reset-after-seconds", "288059"),
        ("x-codex-credits-balance", "0"),
        ("content-type", "text/event-stream"),
    ]);
    assert_eq!(snapshot.used_percent, Some(98.0));
    assert_eq!(snapshot.window, Some(Duration::from_secs(10080 * 60)));
    assert_eq!(snapshot.resets_in, Some(Duration::from_secs(288_059)));
    assert_eq!(snapshot.plan.as_deref(), Some("prolite"));
    assert_eq!(snapshot.remaining_requests, None);
    // Case-folded names; unknown codex header kept; unrelated header dropped.
    assert_eq!(
        snapshot
            .raw
            .get("x-codex-primary-used-percent")
            .map(String::as_str),
        Some("98")
    );
    assert_eq!(
        snapshot
            .raw
            .get("x-codex-credits-balance")
            .map(String::as_str),
        Some("0")
    );
    assert!(!snapshot.raw.contains_key("content-type"));
    assert!(!snapshot.raw.is_empty());
}

#[test]
fn rate_limit_snapshot_parses_openai_and_anthropic_headers() {
    let openai = RateLimitSnapshot::from_headers([
        ("x-ratelimit-remaining-requests", "199"),
        ("x-ratelimit-remaining-tokens", "39500"),
        ("x-ratelimit-reset-requests", "6m0s"),
        ("x-ratelimit-reset-tokens", "250ms"),
    ]);
    assert_eq!(openai.remaining_requests, Some(199));
    assert_eq!(openai.remaining_tokens, Some(39_500));
    assert_eq!(openai.resets_in, Some(Duration::from_secs(360)));
    assert_eq!(openai.raw.len(), 4);

    let anthropic = RateLimitSnapshot::from_headers([
        ("anthropic-ratelimit-requests-remaining", "49"),
        ("anthropic-ratelimit-tokens-remaining", "9000"),
        ("anthropic-ratelimit-requests-reset", "2026-09-07T20:00:00Z"),
    ]);
    assert_eq!(anthropic.remaining_requests, Some(49));
    assert_eq!(anthropic.remaining_tokens, Some(9000));
    // RFC 3339 reset is not parsed into a duration but is still visible.
    assert_eq!(anthropic.resets_in, None);
    assert!(
        anthropic
            .raw
            .contains_key("anthropic-ratelimit-requests-reset")
    );
}

#[test]
fn rate_limit_snapshot_retry_after_only_fills_a_gap() {
    let only_retry = RateLimitSnapshot::from_headers([("retry-after", "30")]);
    assert_eq!(only_retry.resets_in, Some(Duration::from_secs(30)));

    let both = RateLimitSnapshot::from_headers([
        ("retry-after", "30"),
        ("x-codex-primary-reset-after-seconds", "120"),
    ]);
    assert_eq!(both.resets_in, Some(Duration::from_secs(120)));
}

#[test]
fn rate_limit_snapshot_malformed_values_yield_none_but_stay_raw() {
    let snapshot = RateLimitSnapshot::from_headers([
        ("x-codex-primary-used-percent", "lots"),
        ("x-ratelimit-remaining-requests", "-1"),
        ("x-ratelimit-reset-requests", "soon"),
        ("x-codex-primary-window-minutes", ""),
    ]);
    assert_eq!(snapshot.used_percent, None);
    assert_eq!(snapshot.remaining_requests, None);
    assert_eq!(snapshot.resets_in, None);
    assert_eq!(snapshot.window, None);
    assert_eq!(snapshot.raw.len(), 4);
}

#[test]
fn rate_limit_snapshot_with_no_rate_limit_headers_is_empty() {
    let snapshot =
        RateLimitSnapshot::from_headers([("content-type", "application/json"), ("date", "x")]);
    assert!(snapshot.raw.is_empty());
    assert_eq!(snapshot, RateLimitSnapshot::default());
}

#[test]
fn parse_reset_duration_accepts_seconds_and_openai_shapes() {
    assert_eq!(
        parse_reset_duration("288059"),
        Some(Duration::from_secs(288_059))
    );
    assert_eq!(
        parse_reset_duration("1.5"),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(
        parse_reset_duration("1h2m3s"),
        Some(Duration::from_secs(3723))
    );
    assert_eq!(
        parse_reset_duration("250ms"),
        Some(Duration::from_millis(250))
    );
    assert_eq!(parse_reset_duration("6m0s"), Some(Duration::from_secs(360)));
    assert_eq!(parse_reset_duration("-5"), None);
    assert_eq!(parse_reset_duration("5x"), None);
    assert_eq!(
        parse_reset_duration("5m5"),
        None,
        "trailing bare number is malformed"
    );
    assert_eq!(parse_reset_duration(""), None);
}

#[test]
fn stream_options_debug_shows_rate_limit_callback_presence_only() {
    let options = StreamOptions::default().with_on_rate_limit(Arc::new(|_| {}));
    let rendered = format!("{options:?}");
    assert!(
        rendered.contains("on_rate_limit: Some(\"<callback>\")"),
        "{rendered}"
    );
}

/// A `StreamFn` that records the `max_tokens` it was called with and
/// yields a fixed terminal event.
struct OptionsCapturingStreamFn {
    seen_max_tokens: std::sync::Mutex<Vec<Option<u64>>>,
}

impl StreamFn for OptionsCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.seen_max_tokens
            .lock()
            .unwrap()
            .push(options.max_tokens);
        Box::pin(futures::stream::iter([
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ]))
    }
}

fn empty_context() -> AgentContext {
    AgentContext::new("system", Vec::new(), Vec::new())
}

#[tokio::test]
async fn stream_owned_forwards_all_events_with_static_lifetime() {
    use futures::StreamExt as _;
    let inner: Arc<dyn StreamFn> = Arc::new(OptionsCapturingStreamFn {
        seen_max_tokens: std::sync::Mutex::new(Vec::new()),
    });
    // The returned stream is 'static: it outlives every owned local we
    // pass in, which is the whole point.
    let events: Vec<_> = stream_owned(
        inner,
        ModelSpec::new("mock", "m"),
        empty_context(),
        StreamOptions::default(),
        CancellationToken::new(),
    )
    .collect()
    .await;
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert!(matches!(events[1], AssistantMessageEvent::Done { .. }));
}

#[tokio::test]
async fn map_options_rewrites_the_delegated_options() {
    use futures::StreamExt as _;
    let inner = Arc::new(OptionsCapturingStreamFn {
        seen_max_tokens: std::sync::Mutex::new(Vec::new()),
    });
    let seen = Arc::clone(&inner);
    let wrapped = MapOptionsStreamFn::new(inner, |_model, _context, mut options| {
        options.max_tokens = Some(64);
        Ok(options)
    });

    let model = ModelSpec::new("mock", "m");
    let context = empty_context();
    let options = StreamOptions::default();
    let events: Vec<_> = wrapped
        .stream(&model, &context, &options, CancellationToken::new())
        .collect()
        .await;

    assert_eq!(events.len(), 2);
    assert_eq!(*seen.seen_max_tokens.lock().unwrap(), vec![Some(64)]);
}

#[tokio::test]
async fn map_options_refusal_emits_the_given_events_without_delegating() {
    use futures::StreamExt as _;
    let inner = Arc::new(OptionsCapturingStreamFn {
        seen_max_tokens: std::sync::Mutex::new(Vec::new()),
    });
    let seen = Arc::clone(&inner);
    let wrapped = MapOptionsStreamFn::new(inner, |_model, _context, _options| {
        Err(vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "prompt exceeds reply budget".to_string(),
                usage: None,
                error_kind: Some(StreamErrorKind::ContextWindowExceeded),
                retry_after: None,
            },
        ])
    });

    let model = ModelSpec::new("mock", "m");
    let context = empty_context();
    let options = StreamOptions::default();
    let events: Vec<_> = wrapped
        .stream(&model, &context, &options, CancellationToken::new())
        .collect()
        .await;

    assert!(matches!(
        events[1],
        AssistantMessageEvent::Error {
            error_kind: Some(StreamErrorKind::ContextWindowExceeded),
            ..
        }
    ));
    assert!(
        seen.seen_max_tokens.lock().unwrap().is_empty(),
        "a refused request must never reach the inner adapter"
    );
}

#[test]
fn unsupported_fields_names_only_set_and_unhonored_options() {
    let serving = ServingOptions::default()
        .with_context_length(8192)
        .with_top_p(0.9);

    // Fully supported → nothing reported, even with fields set.
    assert!(
        serving
            .unsupported_fields(ServingOptionSupport::all())
            .is_empty()
    );

    // OAI-shape support: context_length is set but not honored; top_p is
    // honored; unset fields (keep_alive/format/extra) are never reported.
    let oai = ServingOptionSupport::none()
        .with_top_p(true)
        .with_format(true)
        .with_extra(true);
    assert_eq!(serving.unsupported_fields(oai), vec!["context_length"]);

    // Nothing set → nothing reported regardless of support.
    assert!(
        ServingOptions::default()
            .unsupported_fields(ServingOptionSupport::none())
            .is_empty()
    );
}

#[test]
fn stream_fn_default_reports_full_support() {
    struct Bare;
    impl StreamFn for Bare {
        fn stream<'a>(
            &'a self,
            _model: &'a ModelSpec,
            _context: &'a AgentContext,
            _options: &'a StreamOptions,
            _cancellation_token: CancellationToken,
        ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
            Box::pin(futures::stream::empty())
        }
    }
    // External adapters that predate the method must not be falsely
    // reported as dropping options.
    assert_eq!(
        Bare.supported_serving_options(),
        ServingOptionSupport::all()
    );
}

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
fn error_with_unterminated_text_block_is_rejected() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "boom".into(),
            usage: None,
            error_kind: None,
            retry_after: None,
        },
    ];

    let err = accumulate_message(events, "test", "test").unwrap_err();
    assert!(err.contains("unterminated content block"), "got: {err}");
}

#[test]
fn error_with_unterminated_thinking_block_is_rejected() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "partial".into(),
        },
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "boom".into(),
            usage: None,
            error_kind: None,
            retry_after: None,
        },
    ];

    let err = accumulate_message(events, "test", "test").unwrap_err();
    assert!(err.contains("unterminated content block"), "got: {err}");
}

#[test]
fn error_with_unterminated_tool_call_block_is_rejected() {
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
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "boom".into(),
            usage: None,
            error_kind: None,
            retry_after: None,
        },
    ];

    let err = accumulate_message(events, "test", "test").unwrap_err();
    assert!(err.contains("unterminated content block"), "got: {err}");
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
fn error_model_retired_constructor_sets_kind() {
    let event = AssistantMessageEvent::error_model_retired("model gpt-4-32k has been retired");
    match event {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert_eq!(error_kind, Some(StreamErrorKind::ModelRetired));
            assert_eq!(error_message, "model gpt-4-32k has been retired");
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

// ── sanitize_incomplete_tool_calls (#619) ──────────────────────────────

fn build_assistant_with_tool_call(
    arguments: Value,
    partial_json: Option<String>,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: "tc_1".into(),
            name: "read_file".into(),
            arguments,
            partial_json,
        }],
        provider: "test".into(),
        model_id: "test".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Length,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }
}

#[test]
fn sanitize_null_arguments_with_partial_json_returns_empty_object() {
    let mut msg = build_assistant_with_tool_call(Value::Null, Some("{\"path\": \"/tm".into()));
    let fixed = sanitize_incomplete_tool_calls(&mut msg);
    assert_eq!(fixed, 1);
    match &msg.content[0] {
        ContentBlock::ToolCall {
            arguments,
            partial_json,
            ..
        } => {
            assert_eq!(*arguments, Value::Object(serde_json::Map::new()));
            assert!(
                partial_json.is_none(),
                "partial_json must be cleared after scrub"
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn sanitize_leaves_valid_object_arguments_untouched() {
    let args = serde_json::json!({ "path": "/tmp/a" });
    let mut msg = build_assistant_with_tool_call(args.clone(), None);
    let fixed = sanitize_incomplete_tool_calls(&mut msg);
    assert_eq!(fixed, 0);
    match &msg.content[0] {
        ContentBlock::ToolCall {
            arguments,
            partial_json,
            ..
        } => {
            assert_eq!(*arguments, args);
            assert!(partial_json.is_none());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn sanitize_coerces_non_object_arguments() {
    // `Value::String` / arrays / numbers are all not objects — they would
    // confuse downstream providers even if `partial_json` is absent.
    let mut msg = build_assistant_with_tool_call(Value::String("truncated".into()), None);
    let fixed = sanitize_incomplete_tool_calls(&mut msg);
    assert_eq!(fixed, 1);
    match &msg.content[0] {
        ContentBlock::ToolCall { arguments, .. } => {
            assert_eq!(*arguments, Value::Object(serde_json::Map::new()));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn sanitize_is_idempotent() {
    let mut msg = build_assistant_with_tool_call(Value::Null, Some("{\"path\":".into()));
    assert_eq!(sanitize_incomplete_tool_calls(&mut msg), 1);
    // A second pass should be a no-op.
    assert_eq!(sanitize_incomplete_tool_calls(&mut msg), 0);
}

#[test]
fn sanitize_preserves_non_tool_blocks() {
    let mut msg = AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::ToolCall {
                id: "tc_1".into(),
                name: "foo".into(),
                arguments: Value::Null,
                partial_json: Some("{".into()),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ],
        provider: "test".into(),
        model_id: "test".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Length,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    };
    let fixed = sanitize_incomplete_tool_calls(&mut msg);
    assert_eq!(fixed, 1);
    // Text blocks preserved in place.
    match &msg.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
    match &msg.content[2] {
        ContentBlock::Text { text } => assert_eq!(text, "world"),
        other => panic!("expected Text, got {other:?}"),
    }
}

/// Regression for #619: the canned "Length + partial tool-call" stream from
/// the issue, when run through `accumulate_message` and then scrubbed,
/// produces a block suitable for replay to any provider adapter.
#[test]
fn accumulate_plus_sanitize_yields_adapter_safe_tool_call() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".into(),
            name: "read_file".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: r#"{"path": "/tm"#.into(),
        },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];
    let mut msg = accumulate_message(events, "test", "test")
        .expect("Done(Length) with unterminated tool-call should accumulate");
    // Pre-scrub: partial_json present, arguments null.
    match &msg.content[0] {
        ContentBlock::ToolCall {
            arguments,
            partial_json,
            ..
        } => {
            assert!(partial_json.is_some());
            assert!(arguments.is_null());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    sanitize_incomplete_tool_calls(&mut msg);

    // Post-scrub: arguments is an empty object, partial_json cleared.
    match &msg.content[0] {
        ContentBlock::ToolCall {
            arguments,
            partial_json,
            ..
        } => {
            assert!(arguments.is_object());
            assert_eq!(arguments.as_object().unwrap().len(), 0);
            assert!(partial_json.is_none());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}
