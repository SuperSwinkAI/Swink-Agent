use std::pin::Pin;
use std::sync::atomic::Ordering;

use futures::Stream;

use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::AgentError;
use crate::loop_::AgentEvent;

use super::Agent;
use super::queueing::llm_messages_from_queue;

fn invalid_state_snapshot(error: serde_json::Error) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("corrupted session state snapshot: {error}"),
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
            store.save_checkpoint(&checkpoint).await?;
        }

        Ok(checkpoint)
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
    /// otherwise they are dropped.
    pub fn restore_from_checkpoint(
        &mut self,
        checkpoint: &Checkpoint,
    ) -> Result<(), std::io::Error> {
        self.state.messages = checkpoint.restore_messages(self.custom_message_registry.as_deref());
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
        self.state.model.provider.clone_from(&checkpoint.provider);
        self.state.model.model_id.clone_from(&checkpoint.model_id);
        self.rebind_stream_fn_for_current_model();

        if let Some(ref state_val) = checkpoint.state {
            let restored = crate::SessionState::restore_from_snapshot(state_val.clone())
                .map_err(invalid_state_snapshot)?;
            let mut s = self
                .session_state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *s = restored;
        }

        Ok(())
    }

    /// Load a checkpoint from the configured store and restore state from it.
    ///
    /// Returns the loaded checkpoint, or `None` if not found.
    /// Returns an error if no checkpoint store is configured.
    pub async fn load_and_restore_checkpoint(
        &mut self,
        id: &str,
    ) -> Result<Option<Checkpoint>, std::io::Error> {
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

        let mut checkpoint = crate::checkpoint::LoopCheckpoint::new(
            &self.state.system_prompt,
            &self.state.model.provider,
            &self.state.model.model_id,
            &self.state.messages,
        )
        .with_pending_messages(llm_messages_from_queue(&self.follow_up_queue))
        .with_pending_steering_messages(llm_messages_from_queue(&self.steering_queue));

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
        self.state.messages = checkpoint.restore_messages(self.custom_message_registry.as_deref());
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
        self.state.model.provider.clone_from(&checkpoint.provider);
        self.state.model.model_id.clone_from(&checkpoint.model_id);
        self.rebind_stream_fn_for_current_model();

        if let Some(ref state_val) = checkpoint.state {
            let restored = crate::SessionState::restore_from_snapshot(state_val.clone())
                .map_err(invalid_state_snapshot)
                .map_err(AgentError::stream)?;
            let mut s = self
                .session_state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *s = restored;
        }

        if self.state.messages.is_empty() {
            return Err(AgentError::NoMessages);
        }

        for msg in checkpoint.restore_pending_messages() {
            self.follow_up(msg);
        }
        for msg in checkpoint.restore_pending_steering_messages() {
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

    #[derive(Default)]
    struct TestCheckpointStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl CheckpointStore for TestCheckpointStore {
        fn save_checkpoint(&self, checkpoint: &Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(checkpoint).unwrap();
            let id = checkpoint.id.clone();
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
        let steering = agent
            .steering_queue
            .lock()
            .unwrap();
        let follow_up = agent
            .follow_up_queue
            .lock()
            .unwrap();

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
        let source_opts =
            AgentOptions::new_simple("system", model_b.clone(), stream_b.clone());
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
        agent.restore_from_checkpoint(&checkpoint);

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
        store.save_checkpoint(&checkpoint).await.unwrap();

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
}
