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
#[path = "checkpoint_tests.rs"]
mod tests;
