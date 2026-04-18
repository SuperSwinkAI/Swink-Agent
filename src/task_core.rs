//! Shared spawned-task lifecycle core.
//!
//! [`TaskCore`] owns the cancellation token, join handle, and status mutex
//! that [`AgentHandle`](crate::AgentHandle) and
//! [`OrchestratedHandle`](crate::OrchestratedHandle) both need. Each handle
//! composes a `TaskCore` and delegates lifecycle methods to it.

use std::sync::{Arc, Mutex, PoisonError};

use futures::FutureExt;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::handle::AgentStatus;
use crate::types::AgentResult;

/// Shared lifecycle core for a spawned agent task.
///
/// Owns the cancellation token, join handle, and status storage that both
/// `AgentHandle` and `OrchestratedHandle` use identically. The status
/// transition logic (`Ok → Completed`, `Aborted → Cancelled`, `Err → Failed`)
/// lives in [`resolve_status`] so it is defined exactly once.
pub(crate) struct TaskCore {
    pub(crate) join_handle: Option<JoinHandle<Result<AgentResult, AgentError>>>,
    pub(crate) cancellation_token: CancellationToken,
    pub(crate) status: Arc<Mutex<AgentStatus>>,
}

impl TaskCore {
    /// Create a new task core with `Running` status and a fresh cancellation token.
    pub(crate) const fn new(
        join_handle: JoinHandle<Result<AgentResult, AgentError>>,
        cancellation_token: CancellationToken,
        status: Arc<Mutex<AgentStatus>>,
    ) -> Self {
        Self {
            join_handle: Some(join_handle),
            cancellation_token,
            status,
        }
    }

    /// Returns the current status of the spawned task.
    pub(crate) fn status(&self) -> AgentStatus {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns `true` if the task is no longer running.
    pub(crate) fn is_done(&self) -> bool {
        self.status() != AgentStatus::Running
    }

    /// Request cancellation of the spawned task (non-blocking).
    pub(crate) fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Consume the core and await the final result.
    pub(crate) async fn result(mut self) -> Result<AgentResult, AgentError> {
        match self.join_handle.take() {
            Some(handle) => match handle.await {
                Ok(result) => result,
                Err(join_err) => Err(AgentError::stream(join_err)),
            },
            None => Err(AgentError::Aborted),
        }
    }

    /// Check if the task is finished and, if so, return the result without
    /// blocking. Returns `None` if still running. Once returned, subsequent
    /// calls yield `None`.
    pub(crate) fn try_result(&mut self) -> Option<Result<AgentResult, AgentError>> {
        let finished = self
            .join_handle
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if finished {
            let handle = self.join_handle.take()?;
            let join_result = handle.now_or_never()?;
            Some(match join_result {
                Ok(result) => result,
                Err(join_err) => Err(AgentError::stream(join_err)),
            })
        } else {
            None
        }
    }
}

/// Map a task result to its terminal [`AgentStatus`].
///
/// This is the single source of truth for the status transition that both
/// `AgentHandle::spawn` and `run_agent_loop` apply after the spawned future
/// completes.
pub(crate) const fn resolve_status(result: &Result<AgentResult, AgentError>) -> AgentStatus {
    match result {
        Ok(_) => AgentStatus::Completed,
        Err(AgentError::Aborted) => AgentStatus::Cancelled,
        Err(_) => AgentStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Cost, StopReason, Usage};

    fn ok_result() -> Result<AgentResult, AgentError> {
        Ok(AgentResult {
            messages: Vec::new(),
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
            error: None,
            transfer_signal: None,
        })
    }

    #[test]
    fn resolve_status_completed() {
        assert_eq!(resolve_status(&ok_result()), AgentStatus::Completed);
    }

    #[test]
    fn resolve_status_cancelled() {
        assert_eq!(
            resolve_status(&Err(AgentError::Aborted)),
            AgentStatus::Cancelled,
        );
    }

    #[test]
    fn resolve_status_failed() {
        assert_eq!(
            resolve_status(&Err(AgentError::ModelThrottled)),
            AgentStatus::Failed,
        );
    }

    #[tokio::test]
    async fn task_core_lifecycle() {
        let token = CancellationToken::new();
        let status = Arc::new(Mutex::new(AgentStatus::Running));
        let status_clone = Arc::clone(&status);

        let handle = tokio::spawn(async move {
            let result = ok_result();
            *status_clone.lock().unwrap() = resolve_status(&result);
            result
        });

        let core = TaskCore::new(handle, token, status);
        let result = core.result().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn task_core_cancel() {
        let token = CancellationToken::new();
        let status = Arc::new(Mutex::new(AgentStatus::Running));
        let status_clone = Arc::clone(&status);
        let token_clone = token.clone();

        let handle = tokio::spawn(async move {
            token_clone.cancelled().await;
            let result: Result<AgentResult, AgentError> = Err(AgentError::Aborted);
            *status_clone.lock().unwrap() = resolve_status(&result);
            result
        });

        let core = TaskCore::new(handle, token.clone(), status);
        assert!(!core.is_done());
        core.cancel();
        let result = core.result().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn task_core_try_result() {
        let token = CancellationToken::new();
        let status = Arc::new(Mutex::new(AgentStatus::Running));
        let status_clone = Arc::clone(&status);

        let handle = tokio::spawn(async move {
            let result = ok_result();
            *status_clone.lock().unwrap() = resolve_status(&result);
            result
        });

        let mut core = TaskCore::new(handle, token, status);

        // Wait for the spawned task to complete.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = core.try_result();
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());

        // Subsequent call returns None.
        assert!(core.try_result().is_none());
    }
}
