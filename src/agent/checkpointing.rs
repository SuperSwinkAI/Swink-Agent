use std::pin::Pin;
use std::sync::atomic::Ordering;

use futures::Stream;

use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::AgentError;
use crate::loop_::AgentEvent;

use super::Agent;
use super::queueing::drain_messages_from_queue;

fn invalid_state_snapshot(error: &serde_json::Error) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("corrupted session state snapshot: {error}"),
    )
}

fn restore_session_state(
    snapshot: Option<&serde_json::Value>,
) -> Result<crate::SessionState, std::io::Error> {
    snapshot.map_or_else(
        || Ok(crate::SessionState::new()),
        |state_val| {
            crate::SessionState::restore_from_snapshot(state_val.clone())
                .map_err(|e| invalid_state_snapshot(&e))
        },
    )
}

impl Agent {
    /// Rebind `self.stream_fn` if the current model's `provider`/`model_id`
    /// matches one of the registered `model_stream_fns`.
    fn rebind_stream_fn_for_current_model(&mut self) {
        if let Some((_, stream_fn)) = self.model_stream_fns.iter().find(|(m, _)| {
            m.provider == self.state.model.provider && m.model_id == self.state.model.model_id
        }) {
            self.stream_fn = std::sync::Arc::clone(stream_fn);
        }
    }

    // ── Checkpointing ────────────────────────────────────────────────────

    /// Create a checkpoint of the current agent state.
    ///
    /// If a [`CheckpointStore`] is configured, the checkpoint is also persisted.
    /// Returns the checkpoint regardless of whether a store is configured.
    pub async fn save_checkpoint(
        &self,
        id: impl Into<String>,
    ) -> Result<Checkpoint, std::io::Error> {
        let mut checkpoint = Checkpoint::new(
            id,
            &self.state.system_prompt,
            &self.state.model.provider,
            &self.state.model.model_id,
            &self.state.messages,
        );

        {
            let s = self
                .session_state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !s.is_empty() {
                checkpoint.state = Some(s.snapshot());
            }
        }

        if let Some(ref store) = self.checkpoint_store {
            store.save_checkpoint(checkpoint.clone()).await?;
        }

        Ok(checkpoint)
    }

    fn ensure_idle_for_checkpoint_restore(&mut self) -> Result<(), std::io::Error> {
        self.check_not_running().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "cannot restore checkpoint while agent is running",
            )
        })
    }

    /// Restore agent message history from a checkpoint.
    ///
    /// Replaces the current messages with those from the checkpoint and
    /// updates the system prompt to match. If the checkpoint's model
    /// matches one of the [`available_models`](crate::AgentOptions::with_available_models),
    /// the stream function is rebound automatically; otherwise the current
    /// stream function is left in place. Persisted custom messages are
    /// restored when a [`CustomMessageRegistry`](crate::types::CustomMessageRegistry)
    /// has been configured on [`AgentOptions`](crate::AgentOptions) via
    /// [`with_custom_message_registry`](crate::AgentOptions::with_custom_message_registry);
    /// otherwise they are dropped. Returns [`std::io::ErrorKind::WouldBlock`]
    /// if a loop is still active; callers must wait for the agent to become
    /// idle before restoring a checkpoint into it.
    pub fn restore_from_checkpoint(
        &mut self,
        checkpoint: &Checkpoint,
    ) -> Result<(), std::io::Error> {
        self.ensure_idle_for_checkpoint_restore()?;
        let restored_messages =
            checkpoint.restore_messages(self.custom_message_registry.as_deref());
        let restored_state = restore_session_state(checkpoint.state.as_ref())?;

        self.clear_transient_runtime_state();
        self.state.messages = restored_messages;
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
        self.state.model.provider.clone_from(&checkpoint.provider);
        self.state.model.model_id.clone_from(&checkpoint.model_id);
        self.rebind_stream_fn_for_current_model();
        *self
            .session_state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = restored_state;

        Ok(())
    }

    /// Load a checkpoint from the configured store and restore state from it.
    ///
    /// Returns the loaded checkpoint, or `None` if not found.
    /// Returns an error if no checkpoint store is configured. Returns
    /// [`std::io::ErrorKind::WouldBlock`] if the agent is still running.
    pub async fn load_and_restore_checkpoint(
        &mut self,
        id: &str,
    ) -> Result<Option<Checkpoint>, std::io::Error> {
        self.ensure_idle_for_checkpoint_restore()?;
        let store = self
            .checkpoint_store
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no checkpoint store configured"))?;

        let maybe = store.load_checkpoint(id).await?;
        if let Some(ref checkpoint) = maybe {
            self.restore_from_checkpoint(checkpoint)?;
        }
        Ok(maybe)
    }

    /// Access the checkpoint store, if configured.
    #[must_use]
    pub fn checkpoint_store(&self) -> Option<&dyn CheckpointStore> {
        self.checkpoint_store.as_deref()
    }

    /// Pause the currently running loop and capture its state as a [`crate::checkpoint::LoopCheckpoint`].
    ///
    /// Signals the loop to stop via the cancellation token and snapshots the
    /// agent's messages, system prompt, and queued LLM messages into a serializable
    /// checkpoint. The checkpoint can later be passed to [`resume`](Self::resume)
    /// to continue the loop from where it left off.
    ///
    /// The agent remains in the *running* state after this call. It becomes idle
    /// when the caller either drains the event stream to completion or drops the
    /// stream returned by [`prompt_stream`](Self::prompt_stream). This prevents a
    /// new run from starting while the previous loop is still tearing down.
    ///
    /// Returns `None` if the agent is not currently running.
    pub fn pause(&mut self) -> Option<crate::checkpoint::LoopCheckpoint> {
        if !self.loop_active.load(Ordering::Acquire) {
            return None;
        }

        if let Some(ref token) = self.abort_controller {
            tracing::info!("pausing agent loop");
            token.cancel();
        }

        let mut pending_messages = self.pending_message_snapshot.snapshot();
        pending_messages.extend(drain_messages_from_queue(&self.follow_up_queue));

        // Prefer the loop_context_snapshot when available: it is updated
        // immediately after pending messages are drained into loop-local
        // context_messages, closing the window where a concurrent pause() would
        // miss those messages (they've left the shared pending queue but haven't
        // yet been delivered back via a TurnEnd event that updates
        // in_flight_messages).
        let loop_ctx = self.loop_context_snapshot.snapshot();
        let checkpoint_messages: &[crate::types::AgentMessage] = if let Some(ref ctx) = loop_ctx {
            ctx.as_slice()
        } else {
            self.in_flight_messages
                .as_deref()
                .unwrap_or(&self.state.messages)
        };

        let mut checkpoint = crate::checkpoint::LoopCheckpoint::new(
            &self.state.system_prompt,
            &self.state.model.provider,
            &self.state.model.model_id,
            checkpoint_messages,
        )
        .with_pending_message_batch(&pending_messages)
        .with_pending_steering_message_batch(&drain_messages_from_queue(&self.steering_queue));

        let s = self
            .session_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !s.is_empty() {
            checkpoint.state = Some(s.snapshot());
        }
        drop(s);

        // Do NOT clear is_running / abort_controller / notify idle here.
        // The agent stays "running" until the LoopGuardStream is dropped or
        // the stream is drained to AgentEnd, which guarantees the spawned loop
        // task has finished using the channel before a new run can start.

        Some(checkpoint)
    }

    /// Resume the agent loop from a previously captured [`crate::checkpoint::LoopCheckpoint`].
    pub async fn resume(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<crate::types::AgentResult, AgentError> {
        self.check_not_running()?;
        self.restore_from_loop_checkpoint(checkpoint)?;
        self.continue_async().await
    }

    /// Resume the agent loop from a checkpoint, returning an event stream.
    pub fn resume_stream(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.check_not_running()?;
        self.restore_from_loop_checkpoint(checkpoint)?;
        self.continue_stream()
    }

    fn restore_from_loop_checkpoint(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<(), AgentError> {
        let restored_messages =
            checkpoint.restore_messages(self.custom_message_registry.as_deref());
        if restored_messages.is_empty() {
            return Err(AgentError::NoMessages);
        }
        let restored_state =
            restore_session_state(checkpoint.state.as_ref()).map_err(AgentError::stream)?;

        self.clear_transient_runtime_state();
        self.state.messages = restored_messages;
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
        self.state.model.provider.clone_from(&checkpoint.provider);
        self.state.model.model_id.clone_from(&checkpoint.model_id);
        self.rebind_stream_fn_for_current_model();
        {
            let mut s = self
                .session_state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *s = restored_state;
        }

        // Clear live queues before re-enqueueing from the checkpoint so that
        // an in-process pause→resume cycle does not duplicate pending work.
        self.clear_queues();

        for msg in checkpoint.restore_pending_messages(self.custom_message_registry.as_deref()) {
            self.follow_up(msg);
        }
        for msg in
            checkpoint.restore_pending_steering_messages(self.custom_message_registry.as_deref())
        {
            self.steer(msg);
        }

        tracing::info!(
            messages = self.state.messages.len(),
            "resuming agent loop from checkpoint"
        );

        Ok(())
    }
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use tokio_util::sync::CancellationToken;

    use crate::agent::Agent;
    use crate::agent_options::AgentOptions;
    use crate::checkpoint::{CheckpointFuture, CheckpointStore, LoopCheckpoint};
    use crate::testing::SimpleMockStreamFn;
    use crate::types::{
        AgentMessage, CustomMessage, CustomMessageRegistry, LlmMessage, ModelSpec, UserMessage,
    };
    use crate::{AgentError, Checkpoint};

    #[derive(Debug, Clone, PartialEq)]
    struct Tagged {
        value: String,
    }

    impl CustomMessage for Tagged {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn type_name(&self) -> Option<&str> {
            Some("Tagged")
        }
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({ "value": self.value }))
        }
        fn clone_box(&self) -> Option<Box<dyn CustomMessage>> {
            Some(Box::new(self.clone()))
        }
    }

    fn tagged_registry() -> CustomMessageRegistry {
        let mut reg = CustomMessageRegistry::new();
        reg.register(
            "Tagged",
            Box::new(|val: serde_json::Value| {
                let value = val
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing value".to_string())?;
                Ok(Box::new(Tagged {
                    value: value.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );
        reg
    }

    fn make_agent(registry: Option<CustomMessageRegistry>) -> Agent {
        let stream_fn = Arc::new(SimpleMockStreamFn::from_text("ok"));
        let mut opts =
            AgentOptions::new_simple("system", ModelSpec::new("mock", "mock-model"), stream_fn);
        if let Some(reg) = registry {
            opts = opts.with_custom_message_registry(reg);
        }
        Agent::new(opts)
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![crate::types::ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn seed_transient_runtime_state(agent: &mut Agent) {
        agent.state.is_running = true;
        agent.state.stream_message = Some(user_msg("streaming"));
        agent
            .state
            .pending_tool_calls
            .insert("tool-call-1".to_string());
        agent.state.error = Some("stale error".to_string());
        agent.abort_controller = Some(CancellationToken::new());
        agent.in_flight_llm_messages = Some(vec![user_msg("in-flight-llm")]);
        agent.in_flight_messages = Some(vec![user_msg("in-flight-checkpoint")]);
    }

    #[derive(Default)]
    struct TestCheckpointStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl CheckpointStore for TestCheckpointStore {
        fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(&checkpoint).unwrap();
            let id = checkpoint.id;
            Box::pin(async move {
                self.data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(id, json);
                Ok(())
            })
        }

        fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>> {
            let id = id.to_string();
            Box::pin(async move {
                self.data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&id)
                    .map(|json| serde_json::from_str(json).map_err(std::io::Error::other))
                    .transpose()
            })
        }

        fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>> {
            Box::pin(async move {
                Ok(self
                    .data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .cloned()
                    .collect())
            })
        }

        fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()> {
            let id = id.to_string();
            Box::pin(async move {
                self.data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&id);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn restore_from_checkpoint_rehydrates_custom_messages_via_registry() {
        let mut source = make_agent(None);
        source
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![crate::types::ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));
        source
            .state
            .messages
            .push(AgentMessage::Custom(Box::new(Tagged {
                value: "preserved".to_string(),
            })));

        let checkpoint = source.save_checkpoint("cp-1").await.unwrap();
        let json = serde_json::to_string(&checkpoint).unwrap();
        let loaded: crate::checkpoint::Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.custom_messages.len(), 1);

        // Without a registry the custom message is dropped (legacy behavior).
        let mut no_reg = make_agent(None);
        no_reg.restore_from_checkpoint(&loaded).unwrap();
        assert_eq!(no_reg.state.messages.len(), 1);

        // With a registry configured on AgentOptions, the custom message
        // survives restoration through the public API.
        let mut with_reg = make_agent(Some(tagged_registry()));
        with_reg.restore_from_checkpoint(&loaded).unwrap();
        assert_eq!(with_reg.state.messages.len(), 2);
        let restored = with_reg.state.messages[1]
            .downcast_ref::<Tagged>()
            .expect("custom message should be restored via registry");
        assert_eq!(restored.value, "preserved");
    }

    #[tokio::test]
    async fn pause_captures_both_steering_and_follow_up_queues() {
        use crate::types::ContentBlock;

        let mut agent = make_agent(None);
        // Give the agent a message so it's valid to resume
        agent
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));

        // Queue a steering message and a follow-up message
        agent.steer(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "steering-msg".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        })));
        agent.follow_up(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "followup-msg".to_string(),
            }],
            timestamp: 2,
            cache_hint: None,
        })));

        // Simulate a running loop so pause() doesn't return None
        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be running");

        // Verify both queues are captured separately
        assert_eq!(
            checkpoint.pending_messages.len(),
            1,
            "follow-up queue should be captured"
        );
        assert_eq!(
            checkpoint.pending_steering_messages.len(),
            1,
            "steering queue should be captured"
        );

        // Verify the content is correct
        match &checkpoint.pending_messages[0] {
            LlmMessage::User(u) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "followup-msg"),
                _ => panic!("expected text content"),
            },
            _ => panic!("expected user message"),
        }
        match &checkpoint.pending_steering_messages[0] {
            LlmMessage::User(u) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "steering-msg"),
                _ => panic!("expected text content"),
            },
            _ => panic!("expected user message"),
        }

        // After pause, live queues must be drained (#337).
        assert!(
            !agent.has_pending_messages(),
            "queues should be empty after pause drains them"
        );
    }

    #[tokio::test]
    async fn restore_from_loop_checkpoint_routes_steering_to_steering_queue() {
        use crate::checkpoint::LoopCheckpoint;
        use crate::types::ContentBlock;

        let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))];

        let cp = LoopCheckpoint::new("system", "mock", "mock-model", &messages)
            .with_pending_messages(vec![LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "followup".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            })])
            .with_pending_steering_messages(vec![LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "steering".to_string(),
                }],
                timestamp: 2,
                cache_hint: None,
            })]);

        let mut agent = make_agent(None);
        agent.restore_from_loop_checkpoint(&cp).unwrap();

        // Verify steering went to steering queue, follow-up to follow-up queue
        let steering = agent.steering_queue.lock().unwrap();
        let follow_up = agent.follow_up_queue.lock().unwrap();

        assert_eq!(steering.len(), 1, "steering queue should have 1 message");
        assert_eq!(follow_up.len(), 1, "follow-up queue should have 1 message");

        match &steering[0] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "steering"),
                _ => panic!("expected text"),
            },
            _ => panic!("expected user message in steering queue"),
        }
        match &follow_up[0] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "followup"),
                _ => panic!("expected text"),
            },
            _ => panic!("expected user message in follow-up queue"),
        }
    }

    /// Regression test for #337: pause then resume must not duplicate queued
    /// messages.  Before the fix, `pause()` snapshotted the queues without
    /// draining them, and `restore_from_loop_checkpoint()` re-enqueued the
    /// same entries on top of the still-populated live queues.
    #[tokio::test]
    async fn pause_drains_queues_so_resume_does_not_duplicate() {
        use crate::types::ContentBlock;

        let mut agent = make_agent(None);
        agent
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));

        // Enqueue one steering and one follow-up message.
        agent.steer(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "steering-1".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        })));
        agent.follow_up(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "followup-1".to_string(),
            }],
            timestamp: 2,
            cache_hint: None,
        })));

        // Simulate a running loop so pause() doesn't return None.
        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be running");

        // After pause, live queues must be empty (drained into checkpoint).
        assert!(
            !agent.has_pending_messages(),
            "queues should be drained after pause"
        );

        // Restore from the checkpoint — queues should have exactly 1 each.
        agent
            .loop_active
            .store(false, std::sync::atomic::Ordering::Release);
        agent.restore_from_loop_checkpoint(&checkpoint).unwrap();

        let steering = agent.steering_queue.lock().unwrap();
        let follow_up = agent.follow_up_queue.lock().unwrap();

        assert_eq!(
            steering.len(),
            1,
            "steering queue should have exactly 1 message, not duplicated"
        );
        assert_eq!(
            follow_up.len(),
            1,
            "follow-up queue should have exactly 1 message, not duplicated"
        );
    }

    #[tokio::test]
    async fn pause_and_resume_preserves_serializable_custom_pending_messages() {
        use crate::types::ContentBlock;

        let mut agent = make_agent(Some(tagged_registry()));
        agent
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));

        agent.follow_up(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "followup-1".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        })));
        agent.follow_up(AgentMessage::Custom(Box::new(Tagged {
            value: "followup-custom".to_string(),
        })));
        agent.steer(AgentMessage::Custom(Box::new(Tagged {
            value: "steering-custom".to_string(),
        })));
        agent.steer(AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "steering-1".to_string(),
            }],
            timestamp: 2,
            cache_hint: None,
        })));

        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be running");
        assert!(
            !agent.has_pending_messages(),
            "queues should be drained after pause"
        );

        let json = serde_json::to_string(&checkpoint).unwrap();
        let loaded: LoopCheckpoint = serde_json::from_str(&json).unwrap();

        agent
            .loop_active
            .store(false, std::sync::atomic::Ordering::Release);
        agent.restore_from_loop_checkpoint(&loaded).unwrap();

        let steering = agent.steering_queue.lock().unwrap();
        let follow_up = agent.follow_up_queue.lock().unwrap();

        assert_eq!(
            follow_up.len(),
            2,
            "follow-up queue should keep mixed messages"
        );
        assert_eq!(
            steering.len(),
            2,
            "steering queue should keep mixed messages"
        );

        match &follow_up[0] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "followup-1"),
                _ => panic!("expected text content"),
            },
            _ => panic!("expected llm follow-up message"),
        }
        let follow_up_custom = follow_up[1]
            .downcast_ref::<Tagged>()
            .expect("custom follow-up should be restored");
        assert_eq!(follow_up_custom.value, "followup-custom");

        let steering_custom = steering[0]
            .downcast_ref::<Tagged>()
            .expect("custom steering should be restored");
        assert_eq!(steering_custom.value, "steering-custom");
        match &steering[1] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "steering-1"),
                _ => panic!("expected text content"),
            },
            _ => panic!("expected llm steering message"),
        }
    }

    #[tokio::test]
    async fn pause_captures_messages_already_moved_into_loop_local_pending_state() {
        let mut agent = make_agent(Some(tagged_registry()));
        agent.state.messages.push(user_msg("hi"));
        agent.pending_message_snapshot.replace(&[
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![crate::types::ContentBlock::Text {
                    text: "polled-follow-up".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            })),
            AgentMessage::Custom(Box::new(Tagged {
                value: "polled-custom".to_string(),
            })),
        ]);

        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be running");
        let pending = checkpoint.restore_pending_messages(agent.custom_message_registry.as_deref());

        assert_eq!(
            pending.len(),
            2,
            "pause should include loop-local pending messages even when the shared queue is already empty"
        );
        match &pending[0] {
            AgentMessage::Llm(LlmMessage::User(user)) => match &user.content[0] {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "polled-follow-up");
                }
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
        let restored_custom = pending[1]
            .downcast_ref::<Tagged>()
            .expect("custom pending message should be preserved");
        assert_eq!(restored_custom.value, "polled-custom");
    }

    #[tokio::test]
    async fn pause_preserves_in_flight_custom_messages_during_streamed_runs() {
        use futures::future::pending;

        struct PendingStreamFn;

        impl crate::stream::StreamFn for PendingStreamFn {
            fn stream<'a>(
                &'a self,
                _model: &'a crate::ModelSpec,
                _context: &'a crate::AgentContext,
                _options: &'a crate::StreamOptions,
                _cancellation_token: tokio_util::sync::CancellationToken,
            ) -> std::pin::Pin<
                Box<dyn futures::Stream<Item = crate::AssistantMessageEvent> + Send + 'a>,
            > {
                Box::pin(futures::stream::once(async {
                    pending::<()>().await;
                    crate::AssistantMessageEvent::error("unreachable")
                }))
            }
        }

        let stream_fn = Arc::new(PendingStreamFn);
        let opts =
            AgentOptions::new_simple("system", ModelSpec::new("mock", "mock-model"), stream_fn)
                .with_custom_message_registry(tagged_registry());
        let mut agent = Agent::new(opts);
        agent
            .state
            .messages
            .push(AgentMessage::Custom(Box::new(Tagged {
                value: "history-custom".to_string(),
            })));

        let _stream = agent.prompt_stream(vec![user_msg("start")]).unwrap();
        let checkpoint = agent.pause().expect("agent should be running");
        let restored = checkpoint.restore_messages(agent.custom_message_registry.as_deref());

        assert_eq!(
            restored.len(),
            2,
            "pause should keep custom history in checkpoint"
        );

        let restored_custom = restored[0]
            .downcast_ref::<Tagged>()
            .expect("custom history should be restored from the paused checkpoint");
        assert_eq!(restored_custom.value, "history-custom");

        match &restored[1] {
            AgentMessage::Llm(LlmMessage::User(user)) => match &user.content[0] {
                crate::types::ContentBlock::Text { text } => assert_eq!(text, "start"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn restore_from_checkpoint_rebinds_stream_fn_for_matching_model() {
        use crate::stream::StreamFn;
        use crate::types::ContentBlock;

        let model_a = ModelSpec::new("provider-a", "model-a");
        let model_b = ModelSpec::new("provider-b", "model-b");
        let stream_a = Arc::new(SimpleMockStreamFn::from_text("from-a"));
        let stream_b = Arc::new(SimpleMockStreamFn::from_text("from-b"));

        // Agent starts on model_a, with model_b registered as available.
        let opts = AgentOptions::new_simple("system", model_a.clone(), stream_a.clone())
            .with_available_models(vec![(model_b.clone(), stream_b.clone())]);
        let mut agent = Agent::new(opts);

        // Confirm initial stream_fn points to stream_a.
        assert!(
            Arc::ptr_eq(&agent.stream_fn, &(stream_a.clone() as Arc<dyn StreamFn>)),
            "initial stream_fn should be stream_a"
        );

        // Build a checkpoint from a source agent that uses model_b.
        let source_opts = AgentOptions::new_simple("system", model_b.clone(), stream_b.clone());
        let mut source = Agent::new(source_opts);
        source
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));
        let checkpoint = source.save_checkpoint("cp-rebind").await.unwrap();

        // Restore into agent (currently on model_a).
        agent.restore_from_checkpoint(&checkpoint).unwrap();

        // Model metadata should reflect model_b.
        assert_eq!(agent.state.model.provider, "provider-b");
        assert_eq!(agent.state.model.model_id, "model-b");

        // Stream function should now be rebound to stream_b.
        assert!(
            Arc::ptr_eq(&agent.stream_fn, &(stream_b.clone() as Arc<dyn StreamFn>)),
            "stream_fn should be rebound to stream_b after checkpoint restore"
        );
    }

    #[tokio::test]
    async fn restore_from_checkpoint_clears_transient_runtime_state() {
        let mut source = make_agent(None);
        source.state.messages.push(user_msg("restored"));
        let checkpoint = source.save_checkpoint("cp-clear-runtime").await.unwrap();

        let mut agent = make_agent(None);
        seed_transient_runtime_state(&mut agent);

        agent.restore_from_checkpoint(&checkpoint).unwrap();

        assert!(!agent.state.is_running);
        assert!(agent.state.stream_message.is_none());
        assert!(agent.state.pending_tool_calls.is_empty());
        assert!(agent.state.error.is_none());
        assert!(agent.abort_controller.is_none());
        assert!(agent.in_flight_llm_messages.is_none());
        assert!(agent.in_flight_messages.is_none());
    }

    #[tokio::test]
    async fn restore_from_checkpoint_rejects_restore_while_running() {
        let mut source = make_agent(None);
        source.state.messages.push(user_msg("restored"));
        let checkpoint = source.save_checkpoint("cp-running-guard").await.unwrap();

        let mut agent = make_agent(None);
        let stream = agent.prompt_stream(vec![user_msg("hi")]).unwrap();

        let err = agent.restore_from_checkpoint(&checkpoint).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            err.to_string()
                .contains("cannot restore checkpoint while agent is running")
        );
        assert!(agent.is_running());

        drop(stream);
        agent.wait_for_idle().await;
    }

    #[tokio::test]
    async fn restore_from_loop_checkpoint_rebinds_stream_fn_for_matching_model() {
        use crate::checkpoint::LoopCheckpoint;
        use crate::stream::StreamFn;
        use crate::types::ContentBlock;

        let model_a = ModelSpec::new("provider-a", "model-a");
        let model_b = ModelSpec::new("provider-b", "model-b");
        let stream_a = Arc::new(SimpleMockStreamFn::from_text("from-a"));
        let stream_b = Arc::new(SimpleMockStreamFn::from_text("from-b"));

        let opts = AgentOptions::new_simple("system", model_a.clone(), stream_a.clone())
            .with_available_models(vec![(model_b.clone(), stream_b.clone())]);
        let mut agent = Agent::new(opts);

        assert!(
            Arc::ptr_eq(&agent.stream_fn, &(stream_a.clone() as Arc<dyn StreamFn>)),
            "initial stream_fn should be stream_a"
        );

        // Build a LoopCheckpoint for model_b.
        let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))];
        let cp = LoopCheckpoint::new("system", "provider-b", "model-b", &messages);

        agent.restore_from_loop_checkpoint(&cp).unwrap();

        assert_eq!(agent.state.model.provider, "provider-b");
        assert_eq!(agent.state.model.model_id, "model-b");
        assert!(
            Arc::ptr_eq(&agent.stream_fn, &(stream_b.clone() as Arc<dyn StreamFn>)),
            "stream_fn should be rebound to stream_b after loop checkpoint restore"
        );
    }

    #[tokio::test]
    async fn restore_from_loop_checkpoint_clears_transient_runtime_state() {
        let checkpoint = LoopCheckpoint::new("system", "mock", "mock-model", &[user_msg("hi")]);
        let mut agent = make_agent(None);
        seed_transient_runtime_state(&mut agent);

        agent.restore_from_loop_checkpoint(&checkpoint).unwrap();

        assert!(!agent.state.is_running);
        assert!(agent.state.stream_message.is_none());
        assert!(agent.state.pending_tool_calls.is_empty());
        assert!(agent.state.error.is_none());
        assert!(agent.abort_controller.is_none());
        assert!(agent.in_flight_llm_messages.is_none());
        assert!(agent.in_flight_messages.is_none());
    }

    #[tokio::test]
    async fn loop_checkpoint_resume_rehydrates_custom_messages_via_registry() {
        let messages = vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![crate::types::ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })),
            AgentMessage::Custom(Box::new(Tagged {
                value: "resumed".to_string(),
            })),
        ];
        let cp = LoopCheckpoint::new("system", "mock", "mock-model", &messages);
        let json = serde_json::to_string(&cp).unwrap();
        let loaded: LoopCheckpoint = serde_json::from_str(&json).unwrap();

        let mut agent = make_agent(Some(tagged_registry()));
        agent.restore_from_loop_checkpoint(&loaded).unwrap();
        assert_eq!(agent.state.messages.len(), 2);
        let restored = agent.state.messages[1]
            .downcast_ref::<Tagged>()
            .expect("custom message should be restored via registry");
        assert_eq!(restored.value, "resumed");
    }

    #[tokio::test]
    async fn load_and_restore_checkpoint_rejects_corrupt_state_snapshot() {
        let store = TestCheckpointStore::default();
        let checkpoint = Checkpoint::new(
            "bad-state",
            "system",
            "mock",
            "mock-model",
            &[user_msg("hi")],
        )
        .with_state(serde_json::json!(["bad"]));
        store.save_checkpoint(checkpoint).await.unwrap();

        let stream_fn = Arc::new(SimpleMockStreamFn::from_text("ok"));
        let agent_options =
            AgentOptions::new_simple("system", ModelSpec::new("mock", "mock-model"), stream_fn)
                .with_checkpoint_store(store);
        let mut agent = Agent::new(agent_options);

        let err = agent
            .load_and_restore_checkpoint("bad-state")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("corrupted session state snapshot"));
    }

    #[tokio::test]
    async fn load_and_restore_checkpoint_rejects_restore_while_running() {
        let store = TestCheckpointStore::default();
        let checkpoint = Checkpoint::new(
            "running-guard",
            "system",
            "mock",
            "mock-model",
            &[user_msg("hi")],
        );
        store.save_checkpoint(checkpoint).await.unwrap();

        let stream_fn = Arc::new(SimpleMockStreamFn::from_text("ok"));
        let agent_options =
            AgentOptions::new_simple("system", ModelSpec::new("mock", "mock-model"), stream_fn)
                .with_checkpoint_store(store);
        let mut agent = Agent::new(agent_options);
        let stream = agent.prompt_stream(vec![user_msg("start")]).unwrap();

        let err = agent
            .load_and_restore_checkpoint("running-guard")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            err.to_string()
                .contains("cannot restore checkpoint while agent is running")
        );
        assert!(agent.is_running());

        drop(stream);
        agent.wait_for_idle().await;
    }

    #[tokio::test]
    async fn resume_rejects_corrupt_loop_checkpoint_state_snapshot() {
        let checkpoint = LoopCheckpoint::new("system", "mock", "mock-model", &[user_msg("hi")])
            .with_state(serde_json::json!(["bad"]));
        let mut agent = make_agent(None);

        let err = agent.resume(&checkpoint).await.unwrap_err();
        match err {
            AgentError::StreamError { source } => {
                let io = source
                    .downcast_ref::<std::io::Error>()
                    .expect("expected io::Error source");
                assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
                assert!(io.to_string().contains("corrupted session state snapshot"));
            }
            other => panic!("expected StreamError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn restore_from_checkpoint_keeps_live_state_when_snapshot_is_corrupt() {
        let checkpoint = Checkpoint::new(
            "bad-state",
            "restored-system",
            "restored",
            "restored-model",
            &[user_msg("restored")],
        )
        .with_state(serde_json::json!(["bad"]));
        let mut agent = make_agent(None);
        agent.state.messages.push(user_msg("existing"));
        agent.state.system_prompt = "live-system".to_string();
        agent.state.model = ModelSpec::new("live-provider", "live-model");
        {
            let mut state = agent
                .session_state()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.set("live", 7_i64).unwrap();
        }

        let err = agent.restore_from_checkpoint(&checkpoint).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        assert_eq!(agent.state.messages.len(), 1);
        match &agent.state.messages[0] {
            AgentMessage::Llm(LlmMessage::User(user)) => match &user.content[0] {
                crate::types::ContentBlock::Text { text } => assert_eq!(text, "existing"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
        assert_eq!(agent.state.system_prompt, "live-system");
        assert_eq!(agent.state.model.provider, "live-provider");
        assert_eq!(agent.state.model.model_id, "live-model");

        let state = agent
            .session_state()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.get::<i64>("live"), Some(7));
    }

    #[tokio::test]
    async fn restore_from_loop_checkpoint_keeps_live_state_when_snapshot_is_corrupt() {
        let checkpoint = LoopCheckpoint::new(
            "restored-system",
            "restored",
            "restored-model",
            &[user_msg("restored")],
        )
        .with_state(serde_json::json!(["bad"]));
        let mut agent = make_agent(None);
        agent.state.messages.push(user_msg("existing"));
        agent.state.system_prompt = "live-system".to_string();
        agent.state.model = ModelSpec::new("live-provider", "live-model");
        agent.follow_up(user_msg("live-follow-up"));
        agent.steer(user_msg("live-steering"));
        {
            let mut state = agent
                .session_state()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.set("live", 9_i64).unwrap();
        }

        let err = agent.resume(&checkpoint).await.unwrap_err();
        match err {
            AgentError::StreamError { source } => {
                let io = source
                    .downcast_ref::<std::io::Error>()
                    .expect("expected io::Error source");
                assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
            }
            other => panic!("expected StreamError, got {other:?}"),
        }

        assert_eq!(agent.state.messages.len(), 1);
        match &agent.state.messages[0] {
            AgentMessage::Llm(LlmMessage::User(user)) => match &user.content[0] {
                crate::types::ContentBlock::Text { text } => assert_eq!(text, "existing"),
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
        assert_eq!(agent.state.system_prompt, "live-system");
        assert_eq!(agent.state.model.provider, "live-provider");
        assert_eq!(agent.state.model.model_id, "live-model");

        let state = agent
            .session_state()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.get::<i64>("live"), Some(9));
        drop(state);

        let follow_up = agent
            .follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let steering = agent
            .steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            follow_up.len(),
            1,
            "failed restore should not clear follow-up queue"
        );
        assert_eq!(
            steering.len(),
            1,
            "failed restore should not clear steering queue"
        );
    }

    #[tokio::test]
    async fn restore_from_checkpoint_clears_session_state_when_snapshot_missing() {
        let mut source = make_agent(None);
        source
            .state
            .messages
            .push(AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![crate::types::ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })));

        let mut checkpoint = source.save_checkpoint("cp-empty-state").await.unwrap();
        checkpoint.state = None;

        let mut agent = make_agent(None);
        {
            let mut state = agent
                .session_state()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.set("stale", 42_i64).unwrap();
        }

        agent.restore_from_checkpoint(&checkpoint).unwrap();

        let state = agent
            .session_state()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            state.is_empty(),
            "missing snapshot should clear stale state"
        );
    }

    #[tokio::test]
    async fn restore_from_loop_checkpoint_clears_session_state_when_snapshot_missing() {
        use crate::checkpoint::LoopCheckpoint;

        let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![crate::types::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))];
        let mut checkpoint = LoopCheckpoint::new("system", "mock", "mock-model", &messages);
        checkpoint.state = None;

        let mut agent = make_agent(None);
        {
            let mut state = agent
                .session_state()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.set("stale", 99_i64).unwrap();
        }

        agent.restore_from_loop_checkpoint(&checkpoint).unwrap();

        let state = agent
            .session_state()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            state.is_empty(),
            "missing snapshot should clear stale state"
        );
    }

    /// Regression test for issue #557: `run_single_turn` drains pending messages
    /// into loop-local `context_messages` and then clears the shared
    /// `pending_message_snapshot`. A concurrent `pause()` in that window would
    /// previously miss those messages. The fix syncs `context_messages` to
    /// `loop_context_snapshot` immediately after the drain, and `pause()` now
    /// prefers that snapshot over `in_flight_messages`.
    #[tokio::test]
    async fn pause_captures_messages_drained_from_pending_into_loop_context() {
        let mut agent = make_agent(None);
        // Simulate the agent being mid-turn: in_flight_messages holds the
        // original messages (before any pending drain), and loop_context_snapshot
        // holds the expanded context after the drain.

        // in_flight_messages = original message only (set at loop start).
        agent.in_flight_messages = Some(vec![user_msg("original")]);
        // pending_message_snapshot is cleared (run_single_turn has already drained it).
        agent.pending_message_snapshot.clear();
        // loop_context_snapshot = original + consumed pending (synced just after drain).
        // replace() uses the internal clone_messages helper which handles AgentMessage variants.
        agent
            .loop_context_snapshot
            .replace(&[user_msg("original"), user_msg("consumed-pending")]);

        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be paused");
        let restored = checkpoint.restore_messages(agent.custom_message_registry.as_deref());

        assert_eq!(
            restored.len(),
            2,
            "pause snapshot must include messages already consumed from the pending queue \
             into loop context, not just in_flight_messages"
        );
        match &restored[0] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "original");
                }
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
        match &restored[1] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "consumed-pending");
                }
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }

    /// When `loop_context_snapshot` is not set (loop has not yet started its
    /// first turn), `pause()` must fall back to `in_flight_messages` as before.
    #[tokio::test]
    async fn pause_falls_back_to_in_flight_messages_when_context_snapshot_absent() {
        let mut agent = make_agent(None);

        // in_flight_messages = message set at loop start.
        agent.in_flight_messages = Some(vec![user_msg("in-flight")]);
        // loop_context_snapshot is empty (not yet set — pre-first-turn).
        // (default state after Agent::new)

        agent
            .loop_active
            .store(true, std::sync::atomic::Ordering::Release);

        let checkpoint = agent.pause().expect("agent should be paused");
        let restored = checkpoint.restore_messages(agent.custom_message_registry.as_deref());

        assert_eq!(
            restored.len(),
            1,
            "pause must fall back to in_flight_messages when loop_context_snapshot is absent"
        );
        match &restored[0] {
            AgentMessage::Llm(LlmMessage::User(u)) => match &u.content[0] {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "in-flight");
                }
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }
}
