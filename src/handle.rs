//! Spawn-and-continue agent handles.
//!
//! [`AgentHandle`] wraps a spawned agent task, providing status polling,
//! cancellation, and result retrieval without blocking the caller.

use std::sync::{Arc, Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::AgentError;
use crate::task_core::{TaskCore, resolve_status};
use crate::types::{AgentMessage, AgentResult, ContentBlock, LlmMessage, UserMessage};
use crate::util::now_timestamp;

/// The lifecycle status of a spawned agent task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// The agent task is still executing.
    Running,
    /// The agent task completed successfully.
    Completed,
    /// The agent task failed with an error.
    Failed,
    /// The agent task was cancelled via [`AgentHandle::cancel`].
    Cancelled,
}

/// A handle to a spawned agent task.
///
/// Created via [`AgentHandle::spawn`] or [`AgentHandle::spawn_text`], which
/// move an [`Agent`] into a background tokio task. The handle allows the caller
/// to poll status, cancel, and retrieve the final result.
pub struct AgentHandle {
    core: TaskCore,
}

impl AgentHandle {
    /// Spawn an agent task with the given input messages.
    ///
    /// Takes ownership of the `Agent` and moves it into a tokio task.
    /// Returns a handle that can be used to poll status, cancel, or await
    /// the result.
    pub fn spawn(mut agent: Agent, input: Vec<AgentMessage>) -> Self {
        let cancellation_token = CancellationToken::new();
        let status = Arc::new(Mutex::new(AgentStatus::Running));
        let status_clone = Arc::clone(&status);
        let token_clone = cancellation_token.clone();

        let join_handle = tokio::spawn(async move {
            let result = tokio::select! {
                result = agent.prompt_async(input) => result,
                () = token_clone.cancelled() => {
                    agent.abort();
                    Err(AgentError::Aborted)
                }
            };
            *status_clone.lock().unwrap_or_else(PoisonError::into_inner) =
                resolve_status(&result);
            result
        });

        Self {
            core: TaskCore::new(join_handle, cancellation_token, status),
        }
    }

    /// Convenience wrapper that spawns an agent with a single text message.
    ///
    /// Equivalent to calling [`spawn`](Self::spawn) with a single
    /// [`UserMessage`] containing the given text.
    pub fn spawn_text(agent: Agent, text: impl Into<String>) -> Self {
        let msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: now_timestamp(),
            cache_hint: None,
        }));
        Self::spawn(agent, vec![msg])
    }

    /// Returns the current status of the spawned agent task.
    pub fn status(&self) -> AgentStatus {
        self.core.status()
    }

    /// Returns `true` if the agent task is no longer running.
    pub fn is_done(&self) -> bool {
        self.core.is_done()
    }

    /// Request cancellation of the spawned agent task.
    ///
    /// This is non-blocking. The task will transition to `Cancelled` status
    /// asynchronously.
    pub fn cancel(&self) {
        self.core.cancel();
    }

    /// Consume the handle and await the final result.
    ///
    /// If the task panicked, returns an [`AgentError::StreamError`] wrapping
    /// the panic message.
    pub async fn result(self) -> Result<AgentResult, AgentError> {
        self.core.result().await
    }

    /// Check if the task is finished and, if so, return the result without
    /// blocking.
    ///
    /// Returns `None` if the task is still running. Once a result is returned,
    /// subsequent calls will return `None`.
    pub fn try_result(&mut self) -> Option<Result<AgentResult, AgentError>> {
        self.core.try_result()
    }
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("status", &self.status())
            .field("join_handle", &self.core.join_handle)
            .field("cancellation_token", &self.core.cancellation_token)
            .finish()
    }
}
