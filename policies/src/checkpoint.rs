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
/// message history from the turn context — every turn writes a **new
/// checkpoint containing the entire history to date**, so an N-turn session
/// stores O(N²) bytes across N checkpoints. For long-session crash-safety
/// where per-turn history is not needed, prefer [`RollingCheckpointPolicy`],
/// and see [`FileCheckpointStore::with_max_checkpoints`] for retention.
///
/// Always returns [`PolicyVerdict::Continue`] — persistence is a side effect.
///
/// [`FileCheckpointStore::with_max_checkpoints`]: https://docs.rs/swink-agent-memory
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::CheckpointPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_post_turn_policy(CheckpointPolicy::new(store).with_session_id("session-42"));
/// ```
pub struct CheckpointPolicy {
    store: Arc<dyn CheckpointStore>,
    handle: tokio::runtime::Handle,
    session_id: Option<String>,
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
            session_id: None,
        }
    }

    /// Override the tokio runtime handle used for spawning saves.
    #[must_use]
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.handle = handle;
        self
    }

    /// Scope checkpoint IDs to a session: IDs become `"{session}-turn-{n}"`.
    ///
    /// Without a session ID, checkpoint IDs are `"turn-{n}"` where `n` is the
    /// turn index — and the turn index **resets to 0 on every `prompt()`
    /// call**. Two runs against the same store therefore reuse the same IDs: a
    /// second run silently overwrites the first run's checkpoints, and if the
    /// second run is shorter, the store ends up holding a mix of fresh and
    /// stale checkpoints under sequential IDs. A consumer restoring "the
    /// highest turn" can then silently restore **stale history from an earlier
    /// run**. Give each `prompt()` run (or logical session) a unique session
    /// ID to keep ID spaces disjoint and prevent that stale-restore hazard.
    ///
    /// The default (no session ID) keeps the historical `"turn-{n}"` format
    /// for backward compatibility.
    #[must_use]
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    fn checkpoint_id(&self, turn_index: usize) -> String {
        match &self.session_id {
            Some(session) => format!("{session}-turn-{turn_index}"),
            None => format!("turn-{turn_index}"),
        }
    }
}

impl std::fmt::Debug for CheckpointPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointPolicy")
            .field("store", &"...")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl PostTurnPolicy for CheckpointPolicy {
    fn name(&self) -> &'static str {
        "checkpoint"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let checkpoint = build_checkpoint(self.checkpoint_id(ctx.turn_index), ctx, turn);
        spawn_save(&self.handle, &self.store, checkpoint);
        PolicyVerdict::Continue
    }
}

/// Persists a **single, continuously overwritten** checkpoint after each turn.
///
/// This is the variant recommended for **long-session crash-safety**.
/// [`CheckpointPolicy`] writes the full history to date under a new ID every
/// turn, so an N-turn session leaves N checkpoint files whose sizes grow
/// linearly — **O(N²) total bytes** (a 300-turn session with a 200 KB final
/// context writes ~300 files and tens of MB, silently). This policy instead
/// reuses one stable ID, so the store's existing save path overwrites a single
/// checkpoint in place and disk cost stays **O(context)** regardless of
/// session length. The `FileCheckpointStore` save path is an atomic
/// temp-file-plus-rename write, so the overwrite can never leave a torn or
/// partial checkpoint behind.
///
/// The trade-offs versus [`CheckpointPolicy`]:
/// - on a crash you lose **at most one turn** (the one being written), and
/// - there is no per-turn history, so no time-travel restore.
///
/// The checkpoint ID is `"rolling"` by default, or `"{session}-rolling"` after
/// [`with_session_id`](Self::with_session_id) — scope it when multiple
/// sessions share one store so they don't overwrite each other's
/// last-known-good state.
///
/// Uses `tokio::spawn` to avoid blocking the sync policy evaluation loop, and
/// always returns [`PolicyVerdict::Continue`] — persistence is a side effect.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::RollingCheckpointPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_post_turn_policy(RollingCheckpointPolicy::new(store).with_session_id("session-42"));
/// ```
pub struct RollingCheckpointPolicy {
    store: Arc<dyn CheckpointStore>,
    handle: tokio::runtime::Handle,
    id: String,
}

impl RollingCheckpointPolicy {
    /// Default checkpoint ID used when no session ID is configured.
    const DEFAULT_ID: &'static str = "rolling";

    /// Create a new `RollingCheckpointPolicy`. Captures `Handle::current()`.
    ///
    /// # Panics
    /// Panics if called outside a tokio runtime context.
    pub fn new(store: Arc<dyn CheckpointStore>) -> Self {
        Self {
            store,
            handle: tokio::runtime::Handle::current(),
            id: Self::DEFAULT_ID.to_string(),
        }
    }

    /// Override the tokio runtime handle used for spawning saves.
    #[must_use]
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.handle = handle;
        self
    }

    /// Scope the rolling checkpoint ID to a session: the ID becomes
    /// `"{session}-rolling"`.
    ///
    /// Use this when multiple sessions write to the same store; otherwise they
    /// all roll the same `"rolling"` checkpoint and overwrite each other's
    /// last-known-good state.
    #[must_use]
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.id = format!("{}-{}", id.into(), Self::DEFAULT_ID);
        self
    }
}

impl std::fmt::Debug for RollingCheckpointPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RollingCheckpointPolicy")
            .field("store", &"...")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl PostTurnPolicy for RollingCheckpointPolicy {
    fn name(&self) -> &'static str {
        "rolling-checkpoint"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let checkpoint = build_checkpoint(self.id.clone(), ctx, turn);
        spawn_save(&self.handle, &self.store, checkpoint);
        PolicyVerdict::Continue
    }
}

/// Build a checkpoint from the policy contexts (shared by both policies).
fn build_checkpoint(
    id: String,
    ctx: &PolicyContext<'_>,
    turn: &TurnPolicyContext<'_>,
) -> Checkpoint {
    let mut checkpoint = Checkpoint::new(
        id,
        turn.system_prompt,
        &turn.model_spec.provider,
        &turn.model_spec.model_id,
        turn.context_messages,
    )
    .with_turn_count(ctx.turn_index)
    .with_usage(ctx.accumulated_usage.clone())
    .with_cost(ctx.accumulated_cost.clone());

    if !ctx.state.is_empty() {
        checkpoint = checkpoint.with_state(ctx.state.snapshot());
    }

    checkpoint
}

/// Fire-and-forget save on the captured runtime handle (shared by both policies).
fn spawn_save(
    handle: &tokio::runtime::Handle,
    store: &Arc<dyn CheckpointStore>,
    checkpoint: Checkpoint,
) {
    let store = Arc::clone(store);
    handle.spawn(async move {
        if let Err(e) = store.save_checkpoint(checkpoint).await {
            tracing::warn!(error = %e, "checkpoint save failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use swink_agent::{AgentMessage, AssistantMessage, Cost, ModelSpec, StopReason, Usage};

    /// Minimal in-memory checkpoint store for testing.
    struct MockCheckpointStore {
        data: std::sync::Mutex<HashMap<String, String>>,
        saved: tokio::sync::Notify,
    }

    impl MockCheckpointStore {
        fn new() -> Self {
            Self {
                data: std::sync::Mutex::new(HashMap::new()),
                saved: tokio::sync::Notify::new(),
            }
        }

        fn get(&self, id: &str) -> Option<Checkpoint> {
            let guard = self.data.lock().unwrap();
            guard.get(id).map(|s| serde_json::from_str(s).unwrap())
        }

        async fn wait_for_checkpoint(&self, id: &str) -> Checkpoint {
            loop {
                if let Some(checkpoint) = self.get(id) {
                    return checkpoint;
                }

                self.saved.notified().await;
            }
        }
    }

    impl CheckpointStore for MockCheckpointStore {
        fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(&checkpoint).unwrap();
            let id = checkpoint.id;
            Box::pin(async move {
                self.data.lock().unwrap().insert(id, json);
                self.saved.notify_waiters();
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

        let cp = store.wait_for_checkpoint("turn-0").await;
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

        let cp = store.wait_for_checkpoint("turn-1").await;
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

        let cp = store.wait_for_checkpoint("turn-0").await;
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
        store.wait_for_checkpoint("turn-3").await;

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

    #[test]
    fn session_id_scopes_checkpoint_ids() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store).with_session_id("sess-a");
        assert_eq!(policy.checkpoint_id(0), "sess-a-turn-0");
        assert_eq!(policy.checkpoint_id(7), "sess-a-turn-7");
    }

    #[test]
    fn default_checkpoint_ids_keep_legacy_format() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        let store: Arc<dyn CheckpointStore> = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store);
        assert_eq!(policy.checkpoint_id(0), "turn-0");
    }

    #[tokio::test]
    async fn session_scoped_runs_do_not_collide() {
        // Two "runs" (turn_index restarts at 0 in each) against one store,
        // each with its own session id: both turn-0 checkpoints survive.
        let store = Arc::new(MockCheckpointStore::new());
        let run1 = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>)
            .with_session_id("run1");
        let run2 = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>)
            .with_session_id("run2");

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
        let turn1 = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "first run",
            model_spec: &model,
            context_messages: &messages,
        };
        run1.evaluate(&ctx, &turn1);

        let turn2 = TurnPolicyContext {
            system_prompt: "second run",
            ..turn1
        };
        run2.evaluate(&ctx, &turn2);

        let cp1 = store.wait_for_checkpoint("run1-turn-0").await;
        let cp2 = store.wait_for_checkpoint("run2-turn-0").await;
        assert_eq!(cp1.system_prompt, "first run");
        assert_eq!(cp2.system_prompt, "second run");
    }

    #[tokio::test]
    async fn default_ids_collide_across_runs() {
        // Documents the CURRENT DEFAULT behavior (kept for backward compat):
        // without a session id, turn_index restarting at 0 in a second run
        // reuses "turn-0" and silently overwrites the first run's checkpoint.
        // This is the stale-restore hazard `with_session_id` exists to prevent.
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
        let turn1 = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
            system_prompt: "first run",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn1);
        let cp = store.wait_for_checkpoint("turn-0").await;
        assert_eq!(cp.system_prompt, "first run");

        // "Second run": turn_index is 0 again.
        let turn2 = TurnPolicyContext {
            system_prompt: "second run",
            ..turn1
        };
        policy.evaluate(&ctx, &turn2);

        loop {
            let cp = store.get("turn-0").unwrap();
            if cp.system_prompt == "second run" {
                break; // run 1's checkpoint was silently overwritten
            }
            store.saved.notified().await;
        }
    }

    #[tokio::test]
    async fn rolling_policy_overwrites_single_id_across_turns() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = RollingCheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);
        assert_eq!(policy.name(), "rolling-checkpoint");

        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let msg = sample_assistant_message();
        let model = sample_model_spec();
        let messages = sample_messages();

        for turn_index in 0..3 {
            let ctx = PolicyContext {
                turn_index,
                accumulated_usage: &usage,
                accumulated_cost: &cost,
                message_count: 2,
                overflow_signal: false,
                new_messages: &[],
                state: &state,
            };
            let turn = TurnPolicyContext {
                assistant_message: &msg,
                tool_results: &[],
                stop_reason: StopReason::Stop,
                system_prompt: "rolling prompt",
                model_spec: &model,
                context_messages: &messages,
            };
            let verdict = policy.evaluate(&ctx, &turn);
            assert!(matches!(verdict, PolicyVerdict::Continue));

            // Wait until this turn's save lands before evaluating the next,
            // so the final content deterministically reflects the last turn.
            loop {
                if let Some(cp) = store.get("rolling") {
                    if cp.turn_count == turn_index {
                        break;
                    }
                }
                store.saved.notified().await;
            }
        }

        // Exactly one checkpoint ID exists, and it matches the latest turn.
        let guard = store.data.lock().unwrap();
        assert_eq!(guard.len(), 1, "rolling policy must keep a single ID");
        let cp: Checkpoint = serde_json::from_str(guard.get("rolling").unwrap()).unwrap();
        assert_eq!(cp.turn_count, 2);
        assert_eq!(cp.system_prompt, "rolling prompt");
    }

    #[tokio::test]
    async fn rolling_policy_session_id_scopes_the_single_id() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = RollingCheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>)
            .with_session_id("sess-a");

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
        let cp = store.wait_for_checkpoint("sess-a-rolling").await;
        assert_eq!(cp.id, "sess-a-rolling");
    }

    #[tokio::test]
    async fn checkpoint_contains_restorable_session_state() {
        let store = Arc::new(MockCheckpointStore::new());
        let policy = CheckpointPolicy::new(store.clone() as Arc<dyn CheckpointStore>);

        let usage = Usage::default();
        let cost = Cost::default();
        let mut state = swink_agent::SessionState::new();
        state.set("workflow_id", "wf-123").unwrap();
        state
            .set("profile", serde_json::json!({"tier": "pro", "score": 42}))
            .unwrap();
        let ctx = PolicyContext {
            turn_index: 4,
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
            system_prompt: "Track session state.",
            model_spec: &model,
            context_messages: &messages,
        };

        policy.evaluate(&ctx, &turn);
        store.wait_for_checkpoint("turn-4").await;

        let loaded = store
            .load_checkpoint("turn-4")
            .await
            .expect("load should succeed")
            .expect("checkpoint should exist");
        let restored = swink_agent::SessionState::restore_from_snapshot(
            loaded
                .state
                .expect("checkpoint should include session state"),
        )
        .expect("state snapshot should restore");

        assert_eq!(restored.get::<String>("workflow_id"), Some("wf-123".into()));
        assert_eq!(
            restored.get_raw("profile"),
            Some(&serde_json::json!({"tier": "pro", "score": 42}))
        );
    }
}
