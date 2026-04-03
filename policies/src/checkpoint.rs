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
/// Uses `tokio::spawn` (fire-and-forget) to avoid blocking the sync
/// policy evaluation loop. Captures a `tokio::runtime::Handle` at
/// construction time.
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

    fn evaluate(&self, ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let checkpoint = Checkpoint::new(
            format!("turn-{}", ctx.turn_index),
            String::new(), // system_prompt not available in PolicyContext
            String::new(), // provider
            String::new(), // model_id
            &[],           // messages snapshot not available in PolicyContext
        )
        .with_turn_count(ctx.turn_index)
        .with_usage(ctx.accumulated_usage.clone())
        .with_cost(ctx.accumulated_cost.clone());

        let store = Arc::clone(&self.store);
        self.handle.spawn(async move {
            if let Err(e) = store.save_checkpoint(&checkpoint).await {
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

    use swink_agent::{AssistantMessage, Cost, StopReason, Usage};

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
    }

    impl CheckpointStore for MockCheckpointStore {
        fn save_checkpoint(&self, checkpoint: &Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(checkpoint).unwrap();
            let id = checkpoint.id.clone();
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
        let msg = AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            cache_hint: None,
        };
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
        };

        let result = policy.evaluate(&ctx, &turn);
        assert!(matches!(result, PolicyVerdict::Continue));
    }
}
