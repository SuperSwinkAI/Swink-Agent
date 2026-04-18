//! Checkpoint policy — persists agent state after each turn.
#![forbid(unsafe_code)]

use std::sync::Arc;

#[cfg(test)]
use swink_agent::CheckpointFuture;
use swink_agent::{
    Checkpoint, CheckpointStore, PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext,
};

/// Persists agent state after each turn via a [`CheckpointStore`].
///
/// Uses `tokio::spawn` to avoid blocking the sync policy evaluation loop.
/// Captures a `tokio::runtime::Handle` at construction time.
///
/// The checkpoint includes the real system prompt, model identity, and full
/// message history from the turn context.
///
/// Always returns [`PolicyVerdict::Continue`] — persistence is a side effect.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::CheckpointPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_post_turn_policy(CheckpointPolicy::new(store));
/// ```
pub struct CheckpointPolicy {
    store: Arc<dyn CheckpointStore>,
    handle: tokio::runtime::Handle,
}

impl CheckpointPolicy {
    /// Create a new `CheckpointPolicy`. Captures `Handle::current()`.
    ///
    /// # Panics
    /// Panics if called outside a tokio runtime context.
    pub fn new(store: Arc<dyn CheckpointStore>) -> Self {
        Self {
            store,
            handle: tokio::runtime::Handle::current(),
        }
    }

    /// Override the tokio runtime handle used for spawning saves.
    #[must_use]
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.handle = handle;
        self
    }
}

impl std::fmt::Debug for CheckpointPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointPolicy")
            .field("store", &"...")
            .finish()
    }
}

impl PostTurnPolicy for CheckpointPolicy {
    fn name(&self) -> &'static str {
        "checkpoint"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let checkpoint = Checkpoint::new(
            format!("turn-{}", ctx.turn_index),
            turn.system_prompt,
            &turn.model_spec.provider,
            &turn.model_spec.model_id,
            turn.context_messages,
        )
        .with_turn_count(ctx.turn_index)
        .with_usage(ctx.accumulated_usage.clone())
        .with_cost(ctx.accumulated_cost.clone());

        let store = Arc::clone(&self.store);
        self.handle.spawn(async move {
            if let Err(e) = store.save_checkpoint(checkpoint).await {
                tracing::warn!(error = %e, "checkpoint save failed");
            }
        });

        PolicyVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use swink_agent::{AgentMessage, AssistantMessage, Cost, ModelSpec, StopReason, Usage};

    /// Minimal in-memory checkpoint store for testing.
    struct MockCheckpointStore {
        data: std::sync::Mutex<HashMap<String, String>>,
    }

    impl MockCheckpointStore {
        fn new() -> Self {
            Self {
                data: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn get(&self, id: &str) -> Option<Checkpoint> {
            let guard = self.data.lock().unwrap();
            guard.get(id).map(|s| serde_json::from_str(s).unwrap())
        }
    }

    impl CheckpointStore for MockCheckpointStore {
        fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(&checkpoint).unwrap();
            let id = checkpoint.id;
            Box::pin(async move {
                self.data.lock().unwrap().insert(id, json);
                Ok(())
            })
        }

        fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>> {
            let id = id.to_string();
            Box::pin(async move {
                let guard = self.data.lock().unwrap();
                Ok(guard.get(&id).map(|s| serde_json::from_str(s).unwrap()))
            })
        }

        fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>> {
            Box::pin(async move { Ok(self.data.lock().unwrap().keys().cloned().collect()) })
        }

        fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()> {
            let id = id.to_string();
            Box::pin(async move {
                self.data.lock().unwrap().remove(&id);
                Ok(())
            })
        }
    }

    fn sample_model_spec() -> ModelSpec {
        ModelSpec::new("anthropic", "claude-sonnet-4-20250514")
    }

    fn sample_assistant_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![swink_agent::ContentBlock::Text {
                text: "Hello!".to_string(),
            }],
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4-20250514".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }
    }

    fn sample_messages() -> Vec<AgentMessage> {
        use swink_agent::{ContentBlock, LlmMessage, UserMessage};
        vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "What is 2+2?".to_string(),
                }],
                timestamp: 100,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(sample_assistant_message())),
        ]
    }

    #[test]
    fn name_returns_checkpoint() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store);
        assert_eq!(policy.name(), "checkpoint");
    }

    #[test]
    fn evaluate_returns_continue() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store);

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 0,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "Be helpful.",
            model_spec: &model,
            context_messages: &messages,
        };

        let result = policy.evaluate(&ctx, &turn);
        assert!(matches!(result, PolicyVerdict::Continue));
    }

    #[tokio::test]
    async fn checkpoint_contains_system_prompt() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 0,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 2,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "You are a helpful math tutor.",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn);
        // Let spawned task complete
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let cp = store.get("turn-0").expect("checkpoint should exist");
        assert_eq!(cp.system_prompt, "You are a helpful math tutor.");
    }

    #[tokio::test]
    async fn checkpoint_contains_model_identity() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 1,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 2,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "prompt",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn);
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let cp = store.get("turn-1").expect("checkpoint should exist");
        assert_eq!(cp.provider, "anthropic");
        assert_eq!(cp.model_id, "claude-sonnet-4-20250514");
    }

    #[tokio::test]
    async fn checkpoint_contains_message_history() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 0,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 2,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "prompt",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn);
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let cp = store.get("turn-0").expect("checkpoint should exist");
        assert_eq!(
            cp.messages.len(),
            2,
            "should contain both user and assistant messages"
        );
    }

    #[tokio::test]
    async fn checkpoint_roundtrip_save_load() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

        let usage = Usage {
            input: 100,
            output: 50,
            ..Default::default()
        };
        let cost = Cost {
            input: 0.01,
            output: 0.005,
            ..Default::default()
        };
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 3,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 2,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "You are a math tutor.",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn);
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Load via the CheckpointStore trait
        let loaded = store
            .load_checkpoint("turn-3")
            .await
            .expect("load should succeed")
            .expect("checkpoint should exist");

        assert_eq!(loaded.system_prompt, "You are a math tutor.");
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.model_id, "claude-sonnet-4-20250514");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.turn_count, 3);
        assert_eq!(loaded.usage.input, 100);
        assert_eq!(loaded.usage.output, 50);

        // Restore messages and verify content
        let restored = loaded.restore_messages(None);
        assert_eq!(restored.len(), 2);
    }
}
