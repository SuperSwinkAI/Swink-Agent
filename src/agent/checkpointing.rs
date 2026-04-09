use futures::Stream;
use std::pin::Pin;

use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::AgentError;
use crate::loop_::AgentEvent;

use super::Agent;
use super::queueing::llm_messages_from_queue;

impl Agent {
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
    /// updates the system prompt to match. Persisted custom messages are
    /// restored when a [`CustomMessageRegistry`](crate::types::CustomMessageRegistry)
    /// has been configured on [`AgentOptions`](crate::AgentOptions) via
    /// [`with_custom_message_registry`](crate::AgentOptions::with_custom_message_registry);
    /// otherwise they are dropped.
    pub fn restore_from_checkpoint(&mut self, checkpoint: &Checkpoint) {
        self.state.messages = checkpoint.restore_messages(self.custom_message_registry.as_deref());
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
        self.state.model.provider.clone_from(&checkpoint.provider);
        self.state.model.model_id.clone_from(&checkpoint.model_id);

        if let Some(ref state_val) = checkpoint.state {
            let restored = crate::SessionState::restore_from_snapshot(state_val.clone());
            let mut s = self
                .session_state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *s = restored;
        }
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
            self.restore_from_checkpoint(checkpoint);
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
    /// The agent remains in the **running** state until the loop fully drains
    /// and emits `AgentEnd`. Callers should either consume the remaining stream
    /// events (via [`handle_stream_event`](Self::handle_stream_event) or by
    /// awaiting the `prompt_async` future) or call [`wait_for_idle`](Self::wait_for_idle)
    /// before starting a new run.
    ///
    /// Returns `None` if the agent is not currently running.
    pub fn pause(&mut self) -> Option<crate::checkpoint::LoopCheckpoint> {
        if !self.state.is_running {
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
        .with_pending_messages(llm_messages_from_queue(&self.follow_up_queue));

        let s = self
            .session_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !s.is_empty() {
            checkpoint.state = Some(s.snapshot());
        }
        drop(s);

        // Do NOT mark idle here. The spawned loop task is still running
        // asynchronously and may emit events until it reaches `AgentEnd`.
        // The `is_running` flag and idle notification are handled by the
        // normal loop-completion paths (`collect_stream`, `handle_stream_event`,
        // and `update_state_from_event`).

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

        if let Some(ref state_val) = checkpoint.state {
            let restored = crate::SessionState::restore_from_snapshot(state_val.clone());
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

        tracing::info!(
            messages = self.state.messages.len(),
            "resuming agent loop from checkpoint"
        );

        Ok(())
    }
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use std::sync::Arc;

    use crate::agent::Agent;
    use crate::agent_options::AgentOptions;
    use crate::testing::SimpleMockStreamFn;
    use crate::types::{
        AgentMessage, CustomMessage, CustomMessageRegistry, LlmMessage, ModelSpec, UserMessage,
    };

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
        no_reg.restore_from_checkpoint(&loaded);
        assert_eq!(no_reg.state.messages.len(), 1);

        // With a registry configured on AgentOptions, the custom message
        // survives restoration through the public API.
        let mut with_reg = make_agent(Some(tagged_registry()));
        with_reg.restore_from_checkpoint(&loaded);
        assert_eq!(with_reg.state.messages.len(), 2);
        let restored = with_reg.state.messages[1]
            .downcast_ref::<Tagged>()
            .expect("custom message should be restored via registry");
        assert_eq!(restored.value, "preserved");
    }

    #[tokio::test]
    async fn loop_checkpoint_resume_rehydrates_custom_messages_via_registry() {
        use crate::checkpoint::LoopCheckpoint;

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
}
