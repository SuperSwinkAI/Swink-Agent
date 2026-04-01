use futures::Stream;
use std::pin::Pin;

use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::AgentError;
use crate::loop_::AgentEvent;

use super::queueing::llm_messages_from_queue;
use super::Agent;

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
    /// updates the system prompt to match. Custom messages are not restored
    /// by this method; use `restore_messages(Some(registry))` directly for
    /// full restoration including custom messages.
    pub fn restore_from_checkpoint(&mut self, checkpoint: &Checkpoint) {
        self.state.messages = checkpoint.restore_messages(None);
        self.state.system_prompt.clone_from(&checkpoint.system_prompt);
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

        self.state.is_running = false;
        self.abort_controller = None;
        self.idle_notify.notify_waiters();

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
        self.state.messages = checkpoint.restore_messages(None);
        self.state.system_prompt.clone_from(&checkpoint.system_prompt);
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
