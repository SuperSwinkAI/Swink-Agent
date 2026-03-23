//! Continuation and multi-turn tests for the [`Agent`] public API.

mod common;

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockApiKeyCapturingStreamFn, MockContextCapturingStreamFn, MockStreamFn, default_convert,
    default_model, text_only_events, user_msg,
};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentError, AgentEvent, AgentMessage, AgentOptions, AssistantMessageEvent, ContentBlock,
    Cost, DefaultRetryStrategy, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions,
    ToolResultMessage, Usage, UserMessage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    )
}

// ─── 4.17: continue_async with empty messages returns NoMessages ─────────

#[tokio::test]
async fn continue_async_no_messages() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
    let mut agent = make_agent(stream_fn);

    // No messages in the agent — continue should fail.
    let err = agent.continue_async().await.unwrap_err();
    assert!(
        matches!(err, AgentError::NoMessages),
        "expected NoMessages, got {err:?}"
    );
}

// ─── Gap tests: multi-turn, continue scenarios ───────────────────────────

#[tokio::test]
async fn multi_turn_across_separate_prompts() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));
    let mut agent = make_agent(stream_fn);

    let r1 = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::Stop);
    assert!(
        !r1.messages.is_empty(),
        "first prompt should produce messages"
    );

    // Second prompt uses a MockContextCapturingStreamFn to verify growing context,
    // but here we just verify it completes and produces a result.
    let r2 = agent
        .prompt_async(vec![user_msg("follow up")])
        .await
        .unwrap();
    assert_eq!(r2.stop_reason, StopReason::Stop);
    assert!(
        !r2.messages.is_empty(),
        "second prompt should produce messages"
    );

    // The agent should have messages in state from the latest run.
    assert!(
        !agent.state().messages.is_empty(),
        "state should have messages after second prompt"
    );
}

#[tokio::test]
async fn continue_from_tool_result() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events(
        "continued response",
    )]));
    let mut agent = make_agent(stream_fn);

    let user = AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "do something".to_string(),
        }],
        timestamp: 0,
    }));
    let assistant = AgentMessage::Llm(LlmMessage::Assistant(swink_agent::AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: "tc_1".to_string(),
            name: "my_tool".to_string(),
            arguments: serde_json::json!({}),
            partial_json: None,
        }],
        provider: String::new(),
        model_id: String::new(),
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
        cost: Cost::default(),
        error_message: None,
        timestamp: 0,
    }));
    let tool_result = AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc_1".to_string(),
        content: vec![ContentBlock::Text {
            text: "tool output".to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
    }));

    agent.set_messages(vec![user, assistant, tool_result]);

    let result = agent.continue_async().await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop);
}

#[tokio::test]
async fn continue_drains_queues_from_assistant_tail() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first"),
        text_only_events("after steering"),
    ]));
    let mut agent = make_agent(stream_fn);

    let _r = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    let last = agent.state().messages.last();
    assert!(
        matches!(last, Some(AgentMessage::Llm(LlmMessage::Assistant(_)))),
        "last message should be assistant"
    );

    // Without queued messages, continue should fail
    let err = agent.continue_async().await;
    assert!(matches!(err, Err(AgentError::InvalidContinue)));

    // Queue a steering message, then continue should succeed
    agent.steer(user_msg("steering message"));
    let result = agent.continue_async().await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop);
}

#[tokio::test]
async fn continue_does_not_reemit_existing_messages() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("continued")]));
    let mut agent = make_agent(stream_fn);

    let user = AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "original".to_string(),
        }],
        timestamp: 0,
    }));
    let assistant = AgentMessage::Llm(LlmMessage::Assistant(swink_agent::AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: "tc_1".to_string(),
            name: "tool".to_string(),
            arguments: serde_json::json!({}),
            partial_json: None,
        }],
        provider: String::new(),
        model_id: String::new(),
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
        cost: Cost::default(),
        error_message: None,
        timestamp: 0,
    }));
    let tool_result = AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc_1".to_string(),
        content: vec![ContentBlock::Text {
            text: "result".to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
    }));
    agent.set_messages(vec![user, assistant, tool_result]);

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);
    let _id = agent.subscribe(move |event| {
        let name = format!("{event:?}");
        let prefix = name.split([' ', '{', '(']).next().unwrap_or("").to_string();
        events_clone.lock().unwrap().push(prefix);
    });

    let _result = agent.continue_async().await.unwrap();

    let collected = events.lock().unwrap().clone();
    let message_end_count = collected.iter().filter(|n| *n == "MessageEnd").count();
    assert_eq!(
        message_end_count, 1,
        "should only emit MessageEnd for the new assistant message, got {message_end_count}"
    );
}

#[tokio::test]
async fn session_id_forwarding() {
    use std::sync::Mutex as StdMutex;

    struct SessionCapturingStreamFn {
        responses: StdMutex<Vec<Vec<AssistantMessageEvent>>>,
        captured_session_ids: StdMutex<Vec<Option<String>>>,
        captured_api_keys: StdMutex<Vec<Option<String>>>,
    }

    impl StreamFn for SessionCapturingStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a ModelSpec,
            _context: &'a swink_agent::AgentContext,
            options: &'a StreamOptions,
            _cancellation_token: CancellationToken,
        ) -> Pin<Box<dyn futures::Stream<Item = AssistantMessageEvent> + Send + 'a>> {
            self.captured_session_ids
                .lock()
                .unwrap()
                .push(options.session_id.clone());
            self.captured_api_keys
                .lock()
                .unwrap()
                .push(options.api_key.clone());
            let events = {
                let mut responses = self.responses.lock().unwrap();
                if responses.is_empty() {
                    vec![AssistantMessageEvent::Error {
                        stop_reason: StopReason::Error,
                        error_message: "no more responses".to_string(),
                        usage: None,
                        error_kind: None,
                    }]
                } else {
                    responses.remove(0)
                }
            };
            Box::pin(futures::stream::iter(events))
        }
    }

    let capturing = Arc::new(SessionCapturingStreamFn {
        responses: StdMutex::new(vec![text_only_events("ok")]),
        captured_session_ids: StdMutex::new(Vec::new()),
        captured_api_keys: StdMutex::new(Vec::new()),
    });

    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing) as Arc<dyn StreamFn>;

    let options = StreamOptions {
        session_id: Some("session-abc".to_string()),
        ..StreamOptions::default()
    };

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_stream_options(options)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let ids = capturing.captured_session_ids.lock().unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0], Some("session-abc".to_string()));
    drop(ids);

    let api_keys = capturing.captured_api_keys.lock().unwrap();
    assert_eq!(api_keys.len(), 1);
    assert_eq!(api_keys[0], None);
    drop(api_keys);
}

#[tokio::test]
async fn get_api_key_forwarding() {
    let capturing = Arc::new(MockApiKeyCapturingStreamFn::new(vec![text_only_events("ok")]));

    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing) as Arc<dyn StreamFn>;

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_get_api_key(|provider| {
                assert_eq!(provider, "test");
                Box::pin(async { Some("resolved-key".to_string()) })
            })
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let api_keys = capturing.captured_api_keys.lock().unwrap();
    assert_eq!(api_keys.len(), 1);
    assert_eq!(api_keys[0], Some("resolved-key".to_string()));
    drop(api_keys);
}

#[tokio::test]
async fn agent_end_subscriber_retaining_messages_does_not_lose_history() {
    let stream_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("continued response"),
    ]));
    let mut agent = make_agent(stream_fn.clone());

    let retained_messages: Arc<Mutex<Vec<Arc<Vec<AgentMessage>>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let retained_messages_clone = Arc::clone(&retained_messages);
    let _subscription = agent.subscribe(move |event| {
        if let AgentEvent::AgentEnd { messages } = event {
            retained_messages_clone
                .lock()
                .unwrap()
                .push(Arc::clone(messages));
        }
    });

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(retained_messages.lock().unwrap().len(), 1);

    assert_eq!(
        agent.state().messages.len(),
        2,
        "state should retain user input plus assistant output"
    );
    assert!(
        matches!(
            agent.state().messages.first(),
            Some(AgentMessage::Llm(LlmMessage::User(_)))
        ),
        "first state message should remain the original user input"
    );

    agent.steer(user_msg("follow-up"));
    let continue_result = agent.continue_async().await.unwrap();
    assert_eq!(continue_result.stop_reason, StopReason::Stop);

    let counts = stream_fn.captured_message_counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2);
    assert!(
        counts[1] >= 2,
        "continue should include the prior prompt history, got counts {counts:?}"
    );

    assert_eq!(
        agent.state().messages.len(),
        3,
        "state should not duplicate history across prompt + continue"
    );
}
