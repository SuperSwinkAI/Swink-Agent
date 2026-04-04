//! State persistence and checkpointing for agent conversations.
//!
//! Provides a [`Checkpoint`] struct that captures a snapshot of agent state
//! (messages, system prompt, model, turn count, metadata) and a
//! [`CheckpointStore`] trait for async save/load of checkpoints.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::message_codec::{self, MessageSlot};
use crate::types::{AgentMessage, Cost, CustomMessageRegistry, LlmMessage, Usage};

mod store;

pub use store::{CheckpointFuture, CheckpointStore};

fn restore_llm_messages(messages: &[LlmMessage]) -> Vec<AgentMessage> {
    messages.iter().cloned().map(AgentMessage::Llm).collect()
}

// ─── Checkpoint ──────────────────────────────────────────────────────────────

/// A serializable snapshot of agent conversation state.
///
/// Captures everything needed to restore an agent to a previous point:
/// messages, system prompt, model info, turn count, accumulated usage/cost,
/// and arbitrary metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint.
    pub id: String,
    /// System prompt at the time of the checkpoint.
    pub system_prompt: String,
    /// Model provider name.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
    /// Conversation messages (LLM messages only).
    pub messages: Vec<LlmMessage>,
    /// Serialized custom messages (envelopes with `type` and `data` fields).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    ///
    /// Empty for checkpoints created before ordering support was added;
    /// `restore_messages` falls back to legacy (LLM-first) behavior in that case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    message_order: Vec<MessageSlot>,
    /// Number of completed turns at the time of checkpointing.
    pub turn_count: usize,
    /// Accumulated token usage.
    pub usage: Usage,
    /// Accumulated cost.
    pub cost: Cost,
    /// Unix timestamp when the checkpoint was created.
    pub created_at: u64,
    /// Arbitrary metadata for application-specific use.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Serialized session state snapshot (`SessionState.data`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl Checkpoint {
    /// Create a new checkpoint from the current agent state.
    ///
    /// Serializes `CustomMessage` variants that support serialization (i.e.
    /// `type_name()` and `to_json()` return `Some`). Custom messages that
    /// cannot be serialized are skipped with a warning.
    ///
    /// Use `with_turn_count()`, `with_usage()`, and `with_cost()` to set
    /// additional fields.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        system_prompt: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
        messages: &[AgentMessage],
    ) -> Self {
        let serialized = message_codec::serialize_messages(messages, "checkpoint");

        Self {
            id: id.into(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            messages: serialized.llm_messages,
            custom_messages: serialized.custom_messages,
            message_order: serialized.message_order,
            turn_count: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
            state: None,
        }
    }

    /// Set the session state snapshot.
    #[must_use]
    pub fn with_state(mut self, state: serde_json::Value) -> Self {
        self.state = Some(state);
        self
    }

    /// Set the turn count.
    #[must_use]
    pub const fn with_turn_count(mut self, turn_count: usize) -> Self {
        self.turn_count = turn_count;
        self
    }

    /// Set the accumulated usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    /// Set the accumulated cost.
    #[must_use]
    pub fn with_cost(mut self, cost: Cost) -> Self {
        self.cost = cost;
        self
    }

    /// Add metadata to this checkpoint.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Restore all messages as `AgentMessage` values, preserving their
    /// original interleaved order.
    ///
    /// If `registry` is `None`, custom messages are silently skipped.
    /// Deserialization failures are logged as warnings but do not cause errors.
    ///
    /// For checkpoints created before ordering support, falls back to
    /// legacy behavior (LLM messages first, then custom messages appended).
    #[must_use]
    pub fn restore_messages(&self, registry: Option<&CustomMessageRegistry>) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.messages,
            &self.custom_messages,
            &self.message_order,
            registry,
            "checkpoint",
        )
    }
}

// ─── LoopCheckpoint ──────────────────────────────────────────────────────

/// A serializable snapshot of the agent loop's in-flight state.
///
/// Captures everything needed to pause a running loop and resume it later:
/// messages, pending injections, system prompt, model, and session state.
/// Created by
/// [`Agent::pause`](crate::Agent::pause) and consumed by
/// [`Agent::resume`](crate::Agent::resume).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopCheckpoint {
    /// All context messages at the time of pause.
    pub messages: Vec<LlmMessage>,
    /// Serialized custom messages (envelopes with `type` and `data` fields).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    message_order: Vec<MessageSlot>,
    /// Messages queued for injection into the next turn.
    pub pending_messages: Vec<LlmMessage>,
    /// The system prompt active at the time of pause.
    pub system_prompt: String,
    /// Model provider name.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
    /// Unix timestamp when the checkpoint was created.
    pub created_at: u64,
    /// Arbitrary metadata for application-specific use.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Serialized session state snapshot (`SessionState.data`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl LoopCheckpoint {
    /// Create a loop checkpoint from the current agent state.
    ///
    /// Serializes `CustomMessage` variants that support serialization.
    /// Non-serializable custom messages are skipped with a warning.
    #[must_use]
    pub fn new(
        system_prompt: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
        messages: &[AgentMessage],
    ) -> Self {
        let serialized = message_codec::serialize_messages(messages, "loop checkpoint");

        Self {
            messages: serialized.llm_messages,
            custom_messages: serialized.custom_messages,
            message_order: serialized.message_order,
            pending_messages: Vec::new(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
            state: None,
        }
    }

    /// Set the session state snapshot.
    #[must_use]
    pub fn with_state(mut self, state: serde_json::Value) -> Self {
        self.state = Some(state);
        self
    }

    /// Set pending messages.
    #[must_use]
    pub fn with_pending_messages(mut self, pending: Vec<LlmMessage>) -> Self {
        self.pending_messages = pending;
        self
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Restore all messages as `AgentMessage` values, preserving their
    /// original interleaved order.
    ///
    /// If `registry` is `None`, custom messages are silently skipped.
    #[must_use]
    pub fn restore_messages(&self, registry: Option<&CustomMessageRegistry>) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.messages,
            &self.custom_messages,
            &self.message_order,
            registry,
            "loop checkpoint",
        )
    }

    /// Restore pending messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_pending_messages(&self) -> Vec<AgentMessage> {
        restore_llm_messages(&self.pending_messages)
    }

    /// Convert this loop checkpoint into a standard [`Checkpoint`] for storage.
    #[must_use]
    pub fn to_checkpoint(&self, id: impl Into<String>) -> Checkpoint {
        Checkpoint {
            id: id.into(),
            system_prompt: self.system_prompt.clone(),
            provider: self.provider.clone(),
            model_id: self.model_id.clone(),
            messages: self.messages.clone(),
            custom_messages: self.custom_messages.clone(),
            message_order: self.message_order.clone(),
            turn_count: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            created_at: self.created_at,
            metadata: self.metadata.clone(),
            state: self.state.clone(),
        }
    }
}

// ─── Send + Sync assertions ─────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Checkpoint>();
    assert_send_sync::<LoopCheckpoint>();
};

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, UserMessage};

    #[derive(Debug)]
    struct TestCustom;

    impl crate::types::CustomMessage for TestCustom {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn sample_messages() -> Vec<AgentMessage> {
        vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
                timestamp: 100,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(crate::types::AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }],
                provider: "test".to_string(),
                model_id: "test-model".to_string(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: crate::types::StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 101,
                cache_hint: None,
            })),
        ]
    }

    #[test]
    fn checkpoint_creation_skips_non_serializable_custom_messages() {
        let mut messages = sample_messages();
        // Add a custom message without type_name/to_json — should be skipped
        messages.push(AgentMessage::Custom(Box::new(TestCustom)));

        let checkpoint = Checkpoint::new(
            "cp-1",
            "Be helpful.",
            "anthropic",
            "claude-sonnet",
            &messages,
        )
        .with_turn_count(3);

        assert_eq!(checkpoint.id, "cp-1");
        assert_eq!(checkpoint.system_prompt, "Be helpful.");
        assert_eq!(checkpoint.provider, "anthropic");
        assert_eq!(checkpoint.model_id, "claude-sonnet");
        assert_eq!(checkpoint.messages.len(), 2); // LLM messages only
        assert!(checkpoint.custom_messages.is_empty()); // non-serializable skipped
        assert_eq!(checkpoint.turn_count, 3);
    }

    #[test]
    fn checkpoint_custom_message_roundtrip() {
        use crate::types::CustomMessageRegistry;

        #[derive(Debug, Clone, PartialEq)]
        struct SerializableCustom {
            value: String,
        }

        impl crate::types::CustomMessage for SerializableCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn type_name(&self) -> Option<&str> {
                Some("SerializableCustom")
            }
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "value": self.value }))
            }
        }

        let mut messages = sample_messages();
        messages.push(AgentMessage::Custom(Box::new(SerializableCustom {
            value: "hello".to_string(),
        })));

        let checkpoint = Checkpoint::new("cp-custom", "prompt", "p", "m", &messages);

        assert_eq!(checkpoint.messages.len(), 2);
        assert_eq!(checkpoint.custom_messages.len(), 1);
        assert_eq!(checkpoint.custom_messages[0]["type"], "SerializableCustom");
        assert_eq!(checkpoint.custom_messages[0]["data"]["value"], "hello");

        // Serde roundtrip preserves custom_messages
        let json = serde_json::to_string(&checkpoint).unwrap();
        let restored_cp: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(restored_cp.custom_messages.len(), 1);

        // Restore with registry
        let mut registry = CustomMessageRegistry::new();
        registry.register(
            "SerializableCustom",
            Box::new(|val: serde_json::Value| {
                let value = val
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing value".to_string())?;
                Ok(Box::new(SerializableCustom {
                    value: value.to_string(),
                }) as Box<dyn crate::types::CustomMessage>)
            }),
        );

        let restored = restored_cp.restore_messages(Some(&registry));
        assert_eq!(restored.len(), 3);
        assert!(matches!(
            restored[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
        assert!(matches!(
            restored[1],
            AgentMessage::Llm(LlmMessage::Assistant(_))
        ));
        let custom = restored[2].downcast_ref::<SerializableCustom>().unwrap();
        assert_eq!(custom.value, "hello");

        // Restore without registry — custom messages skipped
        let restored_no_reg = restored_cp.restore_messages(None);
        assert_eq!(restored_no_reg.len(), 2);
    }

    #[test]
    fn checkpoint_serde_roundtrip() {
        let messages = sample_messages();
        let checkpoint = Checkpoint::new(
            "cp-roundtrip",
            "System prompt",
            "openai",
            "gpt-4",
            &messages,
        )
        .with_turn_count(5)
        .with_usage(Usage {
            input: 100,
            output: 50,
            ..Default::default()
        })
        .with_cost(Cost {
            input: 0.01,
            output: 0.005,
            ..Default::default()
        })
        .with_metadata("session_id", serde_json::json!("sess-abc"));

        let json = serde_json::to_string(&checkpoint).unwrap();
        let restored: Checkpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, "cp-roundtrip");
        assert_eq!(restored.system_prompt, "System prompt");
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.turn_count, 5);
        assert_eq!(restored.usage.input, 100);
        assert_eq!(restored.usage.output, 50);
        assert_eq!(restored.metadata["session_id"], "sess-abc");
    }

    #[test]
    fn restore_messages_wraps_in_agent_message() {
        let messages = sample_messages();
        let checkpoint =
            Checkpoint::new("cp-restore", "prompt", "p", "m", &messages).with_turn_count(1);

        let restored = checkpoint.restore_messages(None);
        assert_eq!(restored.len(), 2);
        assert!(matches!(
            restored[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
        assert!(matches!(
            restored[1],
            AgentMessage::Llm(LlmMessage::Assistant(_))
        ));
    }

    #[test]
    fn checkpoint_with_metadata_builder() {
        let checkpoint = Checkpoint::new("cp-meta", "p", "p", "m", &[])
            .with_metadata("key1", serde_json::json!("value1"))
            .with_metadata("key2", serde_json::json!(42));

        assert_eq!(checkpoint.metadata.len(), 2);
        assert_eq!(checkpoint.metadata["key1"], "value1");
        assert_eq!(checkpoint.metadata["key2"], 42);
    }

    #[test]
    fn checkpoint_backward_compat_no_metadata() {
        // JSON without metadata field should deserialize fine
        let json = r#"{
            "id": "cp-compat",
            "system_prompt": "hello",
            "provider": "p",
            "model_id": "m",
            "messages": [],
            "turn_count": 0,
            "usage": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "total": 0},
            "cost": {"input": 0.0, "output": 0.0, "cache_read": 0.0, "cache_write": 0.0, "total": 0.0},
            "created_at": 100
        }"#;

        let checkpoint: Checkpoint = serde_json::from_str(json).unwrap();
        assert!(checkpoint.metadata.is_empty());
        assert!(checkpoint.custom_messages.is_empty());
    }

    // ─── LoopCheckpoint Tests ────────────────────────────────────────────

    #[test]
    fn loop_checkpoint_creation_skips_non_serializable_custom_messages() {
        let mut messages = sample_messages();
        messages.push(AgentMessage::Custom(Box::new(TestCustom)));

        let cp = LoopCheckpoint::new("Be helpful.", "anthropic", "claude-sonnet", &messages)
            .with_pending_messages(vec![LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "continue".to_string(),
                }],
                timestamp: 123,
                cache_hint: None,
            })]);

        assert_eq!(cp.messages.len(), 2);
        assert!(cp.custom_messages.is_empty());
        assert_eq!(cp.pending_messages.len(), 1);
        assert_eq!(cp.system_prompt, "Be helpful.");
        assert_eq!(cp.provider, "anthropic");
        assert_eq!(cp.model_id, "claude-sonnet");
    }

    #[test]
    fn loop_checkpoint_serde_roundtrip() {
        let messages = sample_messages();
        let cp = LoopCheckpoint::new("System prompt", "openai", "gpt-4", &messages)
            .with_pending_messages(vec![LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "follow-up".to_string(),
                }],
                timestamp: 200,
                cache_hint: None,
            })])
            .with_metadata("workflow_id", serde_json::json!("wf-123"));

        let json = serde_json::to_string(&cp).unwrap();
        let restored: LoopCheckpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.pending_messages.len(), 1);
        assert_eq!(restored.system_prompt, "System prompt");
        assert_eq!(restored.metadata["workflow_id"], "wf-123");
    }

    #[test]
    fn loop_checkpoint_restore_messages() {
        let messages = sample_messages();
        let cp = LoopCheckpoint::new("p", "p", "m", &messages);

        let restored = cp.restore_messages(None);
        assert_eq!(restored.len(), 2);
        assert!(matches!(
            restored[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
        assert!(matches!(
            restored[1],
            AgentMessage::Llm(LlmMessage::Assistant(_))
        ));
    }

    #[test]
    fn loop_checkpoint_pending_messages_roundtrip() {
        let pending = vec![LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "follow-up".to_string(),
            }],
            timestamp: 200,
            cache_hint: None,
        })];

        let cp = LoopCheckpoint::new("p", "p", "m", &[]).with_pending_messages(pending);

        let restored_pending = cp.restore_pending_messages();
        assert_eq!(restored_pending.len(), 1);
        assert!(matches!(
            restored_pending[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
    }

    #[test]
    fn loop_checkpoint_to_standard_checkpoint() {
        let messages = sample_messages();
        let cp = LoopCheckpoint::new("prompt", "anthropic", "claude", &messages)
            .with_metadata("key", serde_json::json!("val"));

        let standard = cp.to_checkpoint("cp-from-loop");
        assert_eq!(standard.id, "cp-from-loop");
        assert_eq!(standard.system_prompt, "prompt");
        assert_eq!(standard.turn_count, 0);
        assert_eq!(standard.usage.input, 0);
        assert_eq!(standard.messages.len(), 2);
        assert_eq!(standard.metadata["key"], "val");
    }

    // ─── Interleaved ordering regression tests (issue #51) ──────────────

    fn make_registry_and_custom(
        tag: &str,
    ) -> (
        CustomMessageRegistry,
        impl Fn(&str) -> Box<dyn crate::types::CustomMessage>,
    ) {
        #[derive(Debug, Clone, PartialEq)]
        struct TaggedCustom {
            tag: String,
        }

        impl crate::types::CustomMessage for TaggedCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn type_name(&self) -> Option<&str> {
                Some("TaggedCustom")
            }
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::json!({ "tag": self.tag }))
            }
        }

        let _ = tag; // suppress unused warning
        let mut registry = CustomMessageRegistry::new();
        registry.register(
            "TaggedCustom",
            Box::new(|val: serde_json::Value| {
                let tag = val
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing tag".to_string())?;
                Ok(Box::new(TaggedCustom {
                    tag: tag.to_string(),
                }) as Box<dyn crate::types::CustomMessage>)
            }),
        );

        let factory = |tag: &str| -> Box<dyn crate::types::CustomMessage> {
            Box::new(TaggedCustom {
                tag: tag.to_string(),
            })
        };

        (registry, factory)
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::Assistant(crate::types::AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            provider: "test".to_string(),
            model_id: "test-model".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: crate::types::StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }))
    }

    /// Extracts text from an `AgentMessage` for assertion purposes.
    fn message_text(msg: &AgentMessage) -> String {
        match msg {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                ContentBlock::Text { text } => format!("user:{text}"),
                _ => "user:?".to_string(),
            },
            AgentMessage::Llm(LlmMessage::Assistant(a)) => match &a.content[0] {
                ContentBlock::Text { text } => format!("assistant:{text}"),
                _ => "assistant:?".to_string(),
            },
            AgentMessage::Custom(c) => {
                if let Some(json) = c.to_json() {
                    format!("custom:{}", json["tag"].as_str().unwrap_or("?"))
                } else {
                    "custom:?".to_string()
                }
            }
            _ => "other".to_string(),
        }
    }

    #[test]
    fn checkpoint_preserves_interleaved_custom_message_order() {
        let (registry, factory) = make_registry_and_custom("test");

        // Interleaved: User, Custom("A"), Assistant, Custom("B"), User
        let messages = vec![
            user_msg("hello"),
            AgentMessage::Custom(factory("A")),
            assistant_msg("hi"),
            AgentMessage::Custom(factory("B")),
            user_msg("thanks"),
        ];

        let checkpoint = Checkpoint::new("cp-order", "prompt", "p", "m", &messages);

        // Serde roundtrip
        let json = serde_json::to_string(&checkpoint).unwrap();
        let restored_cp: Checkpoint = serde_json::from_str(&json).unwrap();

        let restored = restored_cp.restore_messages(Some(&registry));
        let order: Vec<String> = restored.iter().map(message_text).collect();

        assert_eq!(
            order,
            vec![
                "user:hello",
                "custom:A",
                "assistant:hi",
                "custom:B",
                "user:thanks",
            ],
            "interleaved custom messages must preserve their original position"
        );
    }

    #[test]
    fn loop_checkpoint_preserves_interleaved_custom_message_order() {
        let (registry, factory) = make_registry_and_custom("test");

        let messages = vec![
            user_msg("q1"),
            AgentMessage::Custom(factory("mid")),
            assistant_msg("a1"),
        ];

        let cp = LoopCheckpoint::new("prompt", "p", "m", &messages);

        let json = serde_json::to_string(&cp).unwrap();
        let restored_cp: LoopCheckpoint = serde_json::from_str(&json).unwrap();

        let restored = restored_cp.restore_messages(Some(&registry));
        let order: Vec<String> = restored.iter().map(message_text).collect();

        assert_eq!(
            order,
            vec!["user:q1", "custom:mid", "assistant:a1"],
            "LoopCheckpoint must preserve interleaved custom message order"
        );
    }

    #[test]
    fn loop_checkpoint_to_checkpoint_preserves_order() {
        let (registry, factory) = make_registry_and_custom("test");

        let messages = vec![
            AgentMessage::Custom(factory("first")),
            user_msg("hello"),
            AgentMessage::Custom(factory("second")),
        ];

        let loop_cp = LoopCheckpoint::new("prompt", "p", "m", &messages);
        let standard = loop_cp.to_checkpoint("cp-conv");

        let restored = standard.restore_messages(Some(&registry));
        let order: Vec<String> = restored.iter().map(message_text).collect();

        assert_eq!(
            order,
            vec!["custom:first", "user:hello", "custom:second"],
            "to_checkpoint conversion must preserve message order"
        );
    }

    #[test]
    fn backward_compat_no_message_order_field() {
        // Create a checkpoint with interleaved messages, then strip the
        // message_order field to simulate a legacy checkpoint.
        let (registry, factory) = make_registry_and_custom("test");

        let messages = vec![user_msg("hi"), AgentMessage::Custom(factory("legacy"))];

        let checkpoint = Checkpoint::new("cp-legacy", "hello", "p", "m", &messages);
        // Serialize, strip message_order, deserialize — simulates old format
        let mut json_val = serde_json::to_value(&checkpoint).unwrap();
        json_val.as_object_mut().unwrap().remove("message_order");
        let legacy_cp: Checkpoint = serde_json::from_value(json_val).unwrap();

        // message_order should be empty (stripped)
        assert!(legacy_cp.message_order.is_empty());

        let restored = legacy_cp.restore_messages(Some(&registry));
        // Legacy fallback: LLM first, then custom appended
        assert_eq!(restored.len(), 2);
        assert!(matches!(
            restored[0],
            AgentMessage::Llm(LlmMessage::User(_))
        ));
        let order: Vec<String> = restored.iter().map(message_text).collect();
        assert_eq!(order, vec!["user:hi", "custom:legacy"]);
    }

    #[test]
    fn restore_without_registry_skips_custom_in_ordered_mode() {
        let (_registry, factory) = make_registry_and_custom("test");

        let messages = vec![
            user_msg("hello"),
            AgentMessage::Custom(factory("skipped")),
            assistant_msg("world"),
        ];

        let checkpoint = Checkpoint::new("cp-no-reg", "prompt", "p", "m", &messages);
        let restored = checkpoint.restore_messages(None);

        // Custom messages are skipped when no registry is provided
        assert_eq!(restored.len(), 2);
        let order: Vec<String> = restored.iter().map(message_text).collect();
        assert_eq!(order, vec!["user:hello", "assistant:world"]);
    }
}
