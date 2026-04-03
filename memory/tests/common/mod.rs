//! Shared test helpers for memory crate integration tests.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, Usage, UserMessage,
};
use swink_agent_memory::SessionMeta;

/// Create a sample `UserMessage` wrapped as `AgentMessage`.
pub fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

/// Create a sample `AssistantMessage` wrapped as `AgentMessage`.
pub fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        provider: "test".to_string(),
        model_id: "test-model".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

/// Create a sample `UserMessage` with timestamp, as raw `LlmMessage`.
///
/// Used for `SessionEntry::Message` which takes `LlmMessage` directly.
pub fn user_message_at(text: &str, timestamp: u64) -> LlmMessage {
    LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp,
        cache_hint: None,
    })
}

/// Create a raw `LlmMessage::User` for use with `SessionEntry::Message`.
pub fn llm_user_message(text: &str) -> LlmMessage {
    LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
        cache_hint: None,
    })
}

/// Create a raw `LlmMessage::Assistant` for use with `SessionEntry::Message`.
pub fn llm_assistant_message(text: &str) -> LlmMessage {
    LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        provider: "test".to_string(),
        model_id: "test-model".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
        cache_hint: None,
    })
}

/// Create a sample `SessionMeta` with the given id and title.
pub fn sample_meta(id: &str, title: &str) -> SessionMeta {
    let now = Utc::now();
    SessionMeta {
        id: id.to_string(),
        title: title.to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    }
}

/// Create a `SessionMeta` with specific timestamps.
pub fn sample_meta_with_times(
    id: &str,
    title: &str,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> SessionMeta {
    SessionMeta {
        id: id.to_string(),
        title: title.to_string(),
        created_at,
        updated_at,
        version: 1,
        sequence: 0,
    }
}
