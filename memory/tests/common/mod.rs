//! Shared test helpers for memory crate integration tests.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use swink_agent::{AgentMessage, AssistantMessage, ContentBlock, LlmMessage, UserMessage};
use swink_agent_memory::SessionMeta;

/// Create a sample `UserMessage` wrapped as `AgentMessage`.
pub fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: text.to_owned(),
        }])
        .with_timestamp(0),
    ))
}

/// Create a sample `AssistantMessage` wrapped as `AgentMessage`.
pub fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(
        AssistantMessage::new(
            vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            "test",
            "test-model",
        )
        .with_timestamp(0),
    ))
}

/// Create a sample `UserMessage` with timestamp, as raw `LlmMessage`.
///
/// Used for `SessionEntry::Message` which takes `LlmMessage` directly.
pub fn user_message_at(text: &str, timestamp: u64) -> LlmMessage {
    LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: text.to_owned(),
        }])
        .with_timestamp(timestamp),
    )
}

/// Create a raw `LlmMessage::User` for use with `SessionEntry::Message`.
pub fn llm_user_message(text: &str) -> LlmMessage {
    LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: text.to_owned(),
        }])
        .with_timestamp(0),
    )
}

/// Create a raw `LlmMessage::Assistant` for use with `SessionEntry::Message`.
pub fn llm_assistant_message(text: &str) -> LlmMessage {
    LlmMessage::Assistant(
        AssistantMessage::new(
            vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            "test",
            "test-model",
        )
        .with_timestamp(0),
    )
}

/// Create a sample `SessionMeta` with the given id and title.
pub fn sample_meta(id: &str, title: &str) -> SessionMeta {
    let now = Utc::now();
    SessionMeta::new(id, title, now, now)
}

/// Create a `SessionMeta` with specific timestamps.
pub fn sample_meta_with_times(
    id: &str,
    title: &str,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> SessionMeta {
    SessionMeta::new(id, title, created_at, updated_at)
}
