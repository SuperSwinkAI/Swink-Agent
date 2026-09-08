//! Tests for `task_core`.
#![cfg(test)]

use super::*;
use crate::types::{Cost, StopReason, Usage};

fn ok_result() -> AgentResult {
    AgentResult {
        messages: Vec::new(),
        stop_reason: StopReason::Stop,
        usage: Usage::default(),
        cost: Cost::default(),
        error: None,
        transfer_signal: None,
    }
}

#[test]
fn resolve_status_completed() {
    assert_eq!(resolve_status(&Ok(ok_result())), AgentStatus::Completed);
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
        let result = Ok(ok_result());
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
    let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();

    let handle = tokio::spawn(async move {
        let result = Ok(ok_result());
        *status_clone.lock().unwrap() = resolve_status(&result);
        let _ = completed_tx.send(());
        result
    });

    let mut core = TaskCore::new(handle, token, status);

    completed_rx.await.expect("spawned task should complete");

    let result = core.try_result();
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());

    // Subsequent call returns None.
    assert!(core.try_result().is_none());
}
