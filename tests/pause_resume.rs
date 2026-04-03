#![cfg(feature = "testkit")]
//! Integration tests for pause/resume/checkpoint functionality.

mod common;

use std::sync::Arc;

use common::{MockStreamFn, default_model, text_only_events, user_msg};
use swink_agent::{Agent, AgentOptions, LoopCheckpoint, default_convert};

fn simple_agent(responses: Vec<Vec<swink_agent::AssistantMessageEvent>>) -> Agent {
    let stream_fn = Arc::new(MockStreamFn::new(responses));
    let options = AgentOptions::new("Be helpful.", default_model(), stream_fn, default_convert);
    Agent::new(options)
}

#[tokio::test]
async fn pause_when_not_running_returns_none() {
    let mut agent = simple_agent(vec![text_only_events("hello")]);
    let result = agent.pause();
    assert!(result.is_none());
}

#[tokio::test]
async fn pause_captures_pending_follow_up_messages() {
    use futures::future::pending;

    struct PendingStreamFn;

    impl swink_agent::StreamFn for PendingStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a swink_agent::ModelSpec,
            _context: &'a swink_agent::AgentContext,
            _options: &'a swink_agent::StreamOptions,
            _cancellation_token: tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = swink_agent::AssistantMessageEvent> + Send + 'a>,
        > {
            Box::pin(futures::stream::once(async {
                pending::<()>().await;
                swink_agent::AssistantMessageEvent::error("unreachable")
            }))
        }
    }

    let stream_fn = Arc::new(PendingStreamFn);
    let options = AgentOptions::new("Be helpful.", default_model(), stream_fn, default_convert);
    let mut agent = Agent::new(options);

    agent.follow_up(swink_agent::AgentMessage::Llm(
        swink_agent::LlmMessage::User(swink_agent::UserMessage {
            content: vec![swink_agent::ContentBlock::Text {
                text: "queued follow-up".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        }),
    ));

    let _stream = agent.prompt_stream(vec![user_msg("start")]).unwrap();
    let checkpoint = agent
        .pause()
        .expect("pause should snapshot a running agent");
    assert_eq!(checkpoint.pending_messages.len(), 1);
}

#[tokio::test]
async fn resume_with_empty_checkpoint_returns_no_messages() {
    let mut agent = simple_agent(vec![text_only_events("hello")]);
    let checkpoint = LoopCheckpoint::new("prompt", "test", "test-model", &[]);
    let result = agent.resume(&checkpoint).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, swink_agent::AgentError::NoMessages));
}

#[tokio::test]
async fn resume_restores_messages_and_continues() {
    // Set up agent with enough responses:
    // 1. initial prompt_text
    // 2. continue_async from resume (processes existing messages)
    // 3. follow-up round triggered by pending messages
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("resumed response"),
        text_only_events("follow-up response"),
    ]));
    let options = AgentOptions::new(
        "Be helpful.",
        default_model(),
        stream_fn.clone(),
        default_convert,
    );
    let mut agent = Agent::new(options);

    let result = agent.prompt_text("hello").await.unwrap();
    assert_eq!(result.stop_reason, swink_agent::StopReason::Stop);

    // Create a checkpoint from current state, with a pending follow-up so
    // continue_async is valid (last message is assistant, so we need pending).
    // The pending message triggers a follow-up round inside the loop.
    let pending = vec![swink_agent::LlmMessage::User(swink_agent::UserMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: "continue please".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    })];
    let checkpoint = LoopCheckpoint::new(
        "Be helpful.",
        "test",
        "test-model",
        agent.state().messages.as_slice(),
    )
    .with_pending_messages(pending);

    // Resume from the checkpoint
    let result = agent.resume(&checkpoint).await.unwrap();
    assert_eq!(result.stop_reason, swink_agent::StopReason::Stop);
}

#[tokio::test]
async fn resume_stream_returns_event_stream() {
    let mut agent = simple_agent(vec![text_only_events("first"), text_only_events("second")]);

    // Run once to populate messages
    let _ = agent.prompt_text("hi").await.unwrap();

    let pending = vec![swink_agent::LlmMessage::User(swink_agent::UserMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: "continue".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    })];
    let checkpoint = LoopCheckpoint::new(
        "Be helpful.",
        "test",
        "test-model",
        agent.state().messages.as_slice(),
    )
    .with_pending_messages(pending);

    let stream = agent.resume_stream(&checkpoint);
    assert!(stream.is_ok());
}

#[tokio::test]
async fn resume_while_running_returns_already_running() {
    use futures::StreamExt;

    let mut agent = simple_agent(vec![text_only_events("hello")]);

    // Start a stream but don't consume it yet
    let stream = agent.prompt_stream(vec![user_msg("hi")]).unwrap();

    // Agent is now "running"
    let checkpoint = LoopCheckpoint::new("prompt", "test", "test-model", &[user_msg("hi")]);
    let result = agent.resume(&checkpoint).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        swink_agent::AgentError::AlreadyRunning
    ));

    // Clean up - consume the stream
    let mut stream = stream;
    while stream.next().await.is_some() {}
}

#[test]
fn loop_checkpoint_serde_stable() {
    let checkpoint = LoopCheckpoint::new("prompt", "anthropic", "claude-3", &[])
        .with_metadata("session", serde_json::json!("sess-abc"));

    let json = serde_json::to_string_pretty(&checkpoint).unwrap();
    let restored: LoopCheckpoint = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.system_prompt, "prompt");
    assert_eq!(restored.provider, "anthropic");
    assert_eq!(restored.model_id, "claude-3");
    assert_eq!(restored.metadata["session"], "sess-abc");
}

#[test]
fn loop_checkpoint_to_standard_checkpoint_integration() {
    let msgs = vec![user_msg("hello")];
    let checkpoint =
        LoopCheckpoint::new("sys", "prov", "mod", &msgs).with_metadata("k", serde_json::json!("v"));

    let standard = checkpoint.to_checkpoint("my-id");
    assert_eq!(standard.id, "my-id");
    assert_eq!(standard.turn_count, 0);
    assert_eq!(standard.system_prompt, "sys");
    assert_eq!(standard.messages.len(), 1);
}
