//! Live integration tests for local model inference.
//!
//! All tests are `#[ignore]` — they download ~1.92 GB on first run.
//! Run with: `cargo test -p swink-agent-local-llm --test local_live -- --ignored`

use std::sync::Arc;

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AgentMessage, ContentBlock, LlmMessage, ModelSpec, UserMessage,
};
use tokio_util::sync::CancellationToken;

use swink_agent_local_llm::{LocalModel, LocalStreamFn, ModelConfig};

fn simple_context(prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: "You are a helpful assistant. Be concise.".to_string(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
            timestamp: 0,
        }))],
        tools: vec![],
    }
}

#[tokio::test]
#[ignore]
async fn stream_produces_valid_event_sequence() {
    let config = ModelConfig::default();
    let local_model = Arc::new(LocalModel::new(config));
    let stream_fn = LocalStreamFn::new(Arc::clone(&local_model));

    let model = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
    let context = simple_context("What is 2 + 2? Answer with just the number.");
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    use futures::StreamExt;
    let events: Vec<AssistantMessageEvent> = stream_fn
        .stream(&model, &context, &options, token)
        .collect()
        .await;

    // Verify event sequence: Start, then content blocks, then Done.
    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "first event must be Start"
    );
    assert!(
        matches!(
            events.last(),
            Some(AssistantMessageEvent::Done { .. })
        ),
        "last event must be Done"
    );

    // Accumulate and verify the message.
    let msg =
        swink_agent::stream::accumulate_message(events, "local", "SmolLM3-3B-Q4_K_M").unwrap();
    let text = ContentBlock::extract_text(&msg.content);
    assert!(!text.is_empty(), "response should contain text");
}

#[tokio::test]
#[ignore]
async fn cancellation_stops_stream() {
    let config = ModelConfig::default();
    let local_model = Arc::new(LocalModel::new(config));
    let stream_fn = LocalStreamFn::new(Arc::clone(&local_model));

    let model = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
    let context = simple_context("Write a very long essay about the history of computing.");
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    // Cancel immediately.
    token.cancel();

    use futures::StreamExt;
    let events: Vec<AssistantMessageEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream_fn.stream(&model, &context, &options, token).collect(),
    )
    .await
    .expect("stream should complete within timeout");

    // Should still have a terminal event.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. })),
        "cancelled stream should have a terminal event"
    );
}

#[tokio::test]
#[ignore]
async fn concurrent_sharing() {
    let config = ModelConfig::default();
    let local_model = Arc::new(LocalModel::new(config));

    // Ensure model is loaded first.
    local_model.ensure_ready().await.unwrap();

    let mut handles = vec![];
    for i in 0..3 {
        let model = Arc::clone(&local_model);
        handles.push(tokio::spawn(async move {
            let stream_fn = LocalStreamFn::new(model);
            let spec = ModelSpec::new("local", "SmolLM3-3B-Q4_K_M");
            let context = simple_context(&format!("Say the number {i}."));
            let options = StreamOptions::default();
            let token = CancellationToken::new();

            use futures::StreamExt;
            let events: Vec<AssistantMessageEvent> = stream_fn
                .stream(&spec, &context, &options, token)
                .collect()
                .await;
            assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
