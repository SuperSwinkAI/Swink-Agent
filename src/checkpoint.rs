//! State persistence and checkpointing for agent conversations.
//!
//! Provides a [`Checkpoint`] struct that captures a snapshot of agent state
//! (messages, system prompt, model, turn count, metadata) and a
//! [`CheckpointStore`] trait for async save/load of checkpoints.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::types::{AgentMessage, AssistantMessage, Cost, LlmMessage, Usage};

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
    /// Conversation messages (LLM messages only; custom messages are not serializable).
    pub messages: Vec<LlmMessage>,
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
}

impl Checkpoint {
    /// Create a new checkpoint from the current agent state.
    ///
    /// Filters out `CustomMessage` variants since they are not serializable.
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
        let llm_messages: Vec<LlmMessage> = messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();

        Self {
            id: id.into(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            messages: llm_messages,
            turn_count: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
        }
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

    /// Restore the LLM messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_messages(&self) -> Vec<AgentMessage> {
        self.messages
            .iter()
            .cloned()
            .map(AgentMessage::Llm)
            .collect()
    }
}

// ─── LoopCheckpoint ──────────────────────────────────────────────────────

/// A serializable snapshot of the agent loop's in-flight state.
///
/// Captures everything needed to pause a running loop and resume it later:
/// messages, pending injections, turn counter, accumulated usage/cost,
/// overflow signal, and the last assistant message. Created by
/// [`Agent::pause`](crate::Agent::pause) and consumed by
/// [`Agent::resume`](crate::Agent::resume).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopCheckpoint {
    /// All context messages at the time of pause.
    pub messages: Vec<LlmMessage>,
    /// Messages queued for injection into the next turn.
    pub pending_messages: Vec<LlmMessage>,
    /// Whether the context overflow signal was active.
    pub overflow_signal: bool,
    /// The zero-based turn index at the time of pause.
    pub turn_index: usize,
    /// Accumulated token usage across all completed turns.
    pub usage: Usage,
    /// Accumulated cost across all completed turns.
    pub cost: Cost,
    /// The system prompt active at the time of pause.
    pub system_prompt: String,
    /// Model provider name.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
    /// The last assistant message (if any) for policy/hook continuity.
    pub last_assistant_message: Option<AssistantMessage>,
    /// Unix timestamp when the checkpoint was created.
    pub created_at: u64,
    /// Arbitrary metadata for application-specific use.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl LoopCheckpoint {
    /// Create a loop checkpoint from the current agent state.
    ///
    /// Filters out `CustomMessage` variants since they are not serializable.
    #[must_use]
    pub fn new(
        system_prompt: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
        messages: &[AgentMessage],
    ) -> Self {
        let llm_messages: Vec<LlmMessage> = messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();

        Self {
            messages: llm_messages,
            pending_messages: Vec::new(),
            overflow_signal: false,
            turn_index: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            last_assistant_message: None,
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
        }
    }

    /// Set the turn index.
    #[must_use]
    pub const fn with_turn_index(mut self, turn_index: usize) -> Self {
        self.turn_index = turn_index;
        self
    }

    /// Set accumulated usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    /// Set accumulated cost.
    #[must_use]
    pub fn with_cost(mut self, cost: Cost) -> Self {
        self.cost = cost;
        self
    }

    /// Set pending messages.
    #[must_use]
    pub fn with_pending_messages(mut self, pending: Vec<LlmMessage>) -> Self {
        self.pending_messages = pending;
        self
    }

    /// Set the overflow signal.
    #[must_use]
    pub const fn with_overflow_signal(mut self, signal: bool) -> Self {
        self.overflow_signal = signal;
        self
    }

    /// Set the last assistant message.
    #[must_use]
    pub fn with_last_assistant_message(mut self, msg: AssistantMessage) -> Self {
        self.last_assistant_message = Some(msg);
        self
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Restore the LLM messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_messages(&self) -> Vec<AgentMessage> {
        self.messages
            .iter()
            .cloned()
            .map(AgentMessage::Llm)
            .collect()
    }

    /// Restore pending messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_pending_messages(&self) -> Vec<AgentMessage> {
        self.pending_messages
            .iter()
            .cloned()
            .map(AgentMessage::Llm)
            .collect()
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
            turn_count: self.turn_index,
            usage: self.usage.clone(),
            cost: self.cost.clone(),
            created_at: self.created_at,
            metadata: self.metadata.clone(),
        }
    }
}

// ─── CheckpointStore ─────────────────────────────────────────────────────────

/// A boxed future returned by [`CheckpointStore`] methods.
type AsyncResult<'a, T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

/// Async trait for persisting and loading agent checkpoints.
///
/// Implementations can back onto any storage: filesystem, database, cloud, etc.
pub trait CheckpointStore: Send + Sync {
    /// Save a checkpoint. Overwrites any existing checkpoint with the same ID.
    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> AsyncResult<'_, ()>;

    /// Load a checkpoint by ID.
    fn load_checkpoint(&self, id: &str) -> AsyncResult<'_, Option<Checkpoint>>;

    /// List all checkpoint IDs, most recent first.
    fn list_checkpoints(&self) -> AsyncResult<'_, Vec<String>>;

    /// Delete a checkpoint by ID.
    fn delete_checkpoint(&self, id: &str) -> AsyncResult<'_, ()>;
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

    fn sample_messages() -> Vec<AgentMessage> {
        vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
                timestamp: 100,
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
                timestamp: 101,
            })),
        ]
    }

    #[test]
    fn checkpoint_creation_filters_custom_messages() {
        let mut messages = sample_messages();
        // Add a custom message that should be filtered
        #[derive(Debug)]
        struct TestCustom;
        impl crate::types::CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
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
        assert_eq!(checkpoint.messages.len(), 2); // custom filtered out
        assert_eq!(checkpoint.turn_count, 3);
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

        let restored = checkpoint.restore_messages();
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
    }

    /// In-memory checkpoint store for testing.
    struct InMemoryCheckpointStore {
        data: std::sync::Mutex<HashMap<String, String>>,
    }

    impl InMemoryCheckpointStore {
        fn new() -> Self {
            Self {
                data: std::sync::Mutex::new(HashMap::new()),
            }
        }
    }

    impl CheckpointStore for InMemoryCheckpointStore {
        fn save_checkpoint(&self, checkpoint: &Checkpoint) -> AsyncResult<'_, ()> {
            let json = serde_json::to_string(checkpoint).unwrap();
            let id = checkpoint.id.clone();
            Box::pin(async move {
                self.data
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?
                    .insert(id, json);
                Ok(())
            })
        }

        fn load_checkpoint(&self, id: &str) -> AsyncResult<'_, Option<Checkpoint>> {
            let id = id.to_string();
            Box::pin(async move {
                let guard = self
                    .data
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                match guard.get(&id) {
                    Some(json) => {
                        let cp: Checkpoint = serde_json::from_str(json)
                            .map_err(|e| std::io::Error::other(e.to_string()))?;
                        Ok(Some(cp))
                    }
                    None => Ok(None),
                }
            })
        }

        fn list_checkpoints(&self) -> AsyncResult<'_, Vec<String>> {
            Box::pin(async move {
                let guard = self
                    .data
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                Ok(guard.keys().cloned().collect())
            })
        }

        fn delete_checkpoint(&self, id: &str) -> AsyncResult<'_, ()> {
            let id = id.to_string();
            Box::pin(async move {
                self.data
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?
                    .remove(&id);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn in_memory_checkpoint_store_roundtrip() {
        let store = InMemoryCheckpointStore::new();
        let messages = sample_messages();

        let checkpoint = Checkpoint::new(
            "cp-store-test",
            "Be helpful.",
            "anthropic",
            "claude",
            &messages,
        )
        .with_turn_count(2);

        // Save
        store.save_checkpoint(&checkpoint).await.unwrap();

        // List
        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"cp-store-test".to_string()));

        // Load
        let loaded = store
            .load_checkpoint("cp-store-test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, "cp-store-test");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.turn_count, 2);

        // Load non-existent
        let missing = store.load_checkpoint("nope").await.unwrap();
        assert!(missing.is_none());

        // Delete
        store.delete_checkpoint("cp-store-test").await.unwrap();
        let ids = store.list_checkpoints().await.unwrap();
        assert!(ids.is_empty());
    }

    // ─── LoopCheckpoint Tests ────────────────────────────────────────────

    #[test]
    fn loop_checkpoint_creation_filters_custom_messages() {
        let mut messages = sample_messages();
        #[derive(Debug)]
        struct TestCustom;
        impl crate::types::CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
        messages.push(AgentMessage::Custom(Box::new(TestCustom)));

        let cp = LoopCheckpoint::new("Be helpful.", "anthropic", "claude-sonnet", &messages)
            .with_turn_index(5);

        assert_eq!(cp.messages.len(), 2);
        assert_eq!(cp.turn_index, 5);
        assert_eq!(cp.system_prompt, "Be helpful.");
        assert_eq!(cp.provider, "anthropic");
        assert_eq!(cp.model_id, "claude-sonnet");
    }

    #[test]
    fn loop_checkpoint_serde_roundtrip() {
        let messages = sample_messages();
        let cp = LoopCheckpoint::new("System prompt", "openai", "gpt-4", &messages)
            .with_turn_index(3)
            .with_usage(Usage {
                input: 200,
                output: 100,
                ..Default::default()
            })
            .with_cost(Cost {
                input: 0.02,
                output: 0.01,
                ..Default::default()
            })
            .with_overflow_signal(true)
            .with_metadata("workflow_id", serde_json::json!("wf-123"));

        let json = serde_json::to_string(&cp).unwrap();
        let restored: LoopCheckpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.turn_index, 3);
        assert_eq!(restored.usage.input, 200);
        assert_eq!(restored.usage.output, 100);
        assert!((restored.cost.input - 0.02).abs() < f64::EPSILON);
        assert!(restored.overflow_signal);
        assert_eq!(restored.system_prompt, "System prompt");
        assert_eq!(restored.metadata["workflow_id"], "wf-123");
    }

    #[test]
    fn loop_checkpoint_restore_messages() {
        let messages = sample_messages();
        let cp = LoopCheckpoint::new("p", "p", "m", &messages);

        let restored = cp.restore_messages();
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
            .with_turn_index(7)
            .with_usage(Usage {
                input: 50,
                output: 25,
                ..Default::default()
            })
            .with_metadata("key", serde_json::json!("val"));

        let standard = cp.to_checkpoint("cp-from-loop");
        assert_eq!(standard.id, "cp-from-loop");
        assert_eq!(standard.system_prompt, "prompt");
        assert_eq!(standard.turn_count, 7);
        assert_eq!(standard.usage.input, 50);
        assert_eq!(standard.messages.len(), 2);
        assert_eq!(standard.metadata["key"], "val");
    }

    #[test]
    fn loop_checkpoint_with_last_assistant_message() {
        let assistant = crate::types::AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Hello!".to_string(),
            }],
            provider: "test".to_string(),
            model_id: "test-model".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: crate::types::StopReason::Stop,
            error_message: None,
            timestamp: 300,
        };

        let cp =
            LoopCheckpoint::new("p", "p", "m", &[]).with_last_assistant_message(assistant.clone());

        assert!(cp.last_assistant_message.is_some());
        assert_eq!(cp.last_assistant_message.as_ref().unwrap().timestamp, 300);

        // Verify serde roundtrip preserves last_assistant_message
        let json = serde_json::to_string(&cp).unwrap();
        let deser: LoopCheckpoint = serde_json::from_str(&json).unwrap();
        assert!(deser.last_assistant_message.is_some());
        assert_eq!(deser.last_assistant_message.unwrap().timestamp, 300);
    }
}
