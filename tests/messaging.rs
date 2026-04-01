mod common;

use std::sync::Arc;

use common::*;
use swink_agent::{
    Agent, AgentMailbox, AgentMessage, AgentOptions, AgentRegistry, LlmMessage, UserMessage,
    send_to,
};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

#[tokio::test]
async fn send_to_delivers_via_steer() {
    let registry = AgentRegistry::new();
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    );
    let agent = Agent::new(options);
    let agent_ref = registry.register("worker", agent);

    send_to(&registry, "worker", user_message("hello"))
        .await
        .unwrap();

    let has_pending = agent_ref.lock().await.has_pending_messages();
    assert!(has_pending);
}

#[tokio::test]
async fn send_to_unknown_agent_errors() {
    let registry = AgentRegistry::new();
    let result = send_to(&registry, "nonexistent", user_message("hello")).await;
    assert!(result.is_err());
    // Just verify it's an error — the specific message format is an implementation detail
    assert!(result.is_err());
}

#[tokio::test]
async fn mailbox_send_and_drain() {
    let mailbox = AgentMailbox::new();
    assert!(mailbox.is_empty());
    assert_eq!(mailbox.len(), 0);

    mailbox.send(user_message("first"));
    mailbox.send(user_message("second"));

    assert!(mailbox.has_messages());
    assert_eq!(mailbox.len(), 2);

    let messages = mailbox.drain();
    assert_eq!(messages.len(), 2);
    assert!(mailbox.is_empty());
}

#[tokio::test]
async fn mailbox_drain_leaves_empty() {
    let mailbox = AgentMailbox::new();
    mailbox.send(user_message("msg"));
    let _ = mailbox.drain();
    assert!(!mailbox.has_messages());
    assert_eq!(mailbox.len(), 0);

    // Second drain returns empty
    let messages = mailbox.drain();
    assert!(messages.is_empty());
}

#[tokio::test]
async fn mailbox_clone_shares_state() {
    let mailbox = AgentMailbox::new();
    let clone = mailbox.clone();

    mailbox.send(user_message("from original"));
    assert_eq!(clone.len(), 1);

    let messages = clone.drain();
    assert_eq!(messages.len(), 1);
    assert!(mailbox.is_empty());
}
