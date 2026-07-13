//! Collect phase: result ordering, interrupt outcomes, metrics assembly.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinError, JoinHandle};
use tracing::{error, warn};

use crate::tool::AgentToolResult;
use crate::types::{AgentMessage, ContentBlock, ToolResultMessage};
use crate::util::now_timestamp;

use super::shared::{emit_error_result, panic_payload_message};
use super::{AgentLoopConfig, ToolCallInfo, ToolExecOutcome};

// ─── Group outcome ──────────────────────────────────────────────────────────

/// Outcome of collecting results for a single execution group.
pub(super) enum GroupOutcome {
    /// All tools in the group completed; proceed to next group.
    Continue,
    /// Steering interrupt detected; abort remaining groups.
    SteeringInterrupt,
    /// Transfer detected; abort remaining groups and end the turn.
    TransferInterrupt,
    /// Parent cancellation aborted the batch; stop the turn immediately.
    Aborted,
}

const INTERRUPT_ABORT_GRACE: Duration = Duration::from_millis(50);

type ToolJoinResult = (usize, Result<(), JoinError>);
type ToolJoinFuture = Pin<Box<dyn Future<Output = ToolJoinResult> + Send>>;

fn tool_join_future(idx: usize, handle: JoinHandle<()>) -> ToolJoinFuture {
    Box::pin(async move { (idx, handle.await) })
}

async fn drain_completed_within_grace(
    futs: &mut FuturesUnordered<ToolJoinFuture>,
) -> Vec<ToolJoinResult> {
    let mut completed = Vec::new();
    let grace = tokio::time::sleep(INTERRUPT_ABORT_GRACE);
    tokio::pin!(grace);

    loop {
        if futs.is_empty() {
            break;
        }

        tokio::select! {
            () = &mut grace => break,
            result = futs.next() => {
                let Some(result) = result else {
                    break;
                };
                completed.push(result);
            }
        }
    }

    completed
}

async fn cancel_remaining_with_bounded_wait(
    futs: &mut FuturesUnordered<ToolJoinFuture>,
    abort_handles: &[AbortHandle],
) -> Vec<ToolJoinResult> {
    let completed = drain_completed_within_grace(futs).await;

    if !futs.is_empty() {
        for handle in abort_handles {
            handle.abort();
        }
        warn!(
            remaining = futs.len(),
            grace_ms = INTERRUPT_ABORT_GRACE.as_millis(),
            "tool tasks did not finish within cancellation grace; aborting without waiting"
        );
    }

    completed
}

async fn emit_join_failure(
    idx: usize,
    join_error: JoinError,
    tool_calls: &[ToolCallInfo],
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<super::AgentEvent>,
) {
    let panic_message = if join_error.is_panic() {
        let panic_value = join_error.into_panic();
        panic_payload_message(panic_value.as_ref())
    } else {
        format!("{join_error}")
    };

    let tc = &tool_calls[idx];
    error!(
        tool_call_id = %tc.id,
        tool_name = %tc.name,
        "tool execution panicked: {panic_message}"
    );

    emit_error_result(
        &tc.name,
        &tc.id,
        AgentToolResult::error(format!("tool execution panicked: {panic_message}")),
        idx,
        results,
        tx,
    )
    .await;
}

pub(super) async fn abort_join_handles_with_grace(handles: Vec<JoinHandle<()>>) {
    if handles.is_empty() {
        return;
    }

    let abort_handles: Vec<_> = handles.iter().map(JoinHandle::abort_handle).collect();
    let mut futs: FuturesUnordered<_> = handles
        .into_iter()
        .enumerate()
        .map(|(idx, handle)| tool_join_future(idx, handle))
        .collect();
    let _ = cancel_remaining_with_bounded_wait(&mut futs, &abort_handles).await;
}

// ─── Group result collection ────────────────────────────────────────────────

/// Collect results for a single execution group's spawned handles.
pub(super) async fn collect_group_results(
    tool_calls: &[ToolCallInfo],
    handles: Vec<(usize, tokio::task::JoinHandle<()>)>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_detected: &Arc<std::sync::atomic::AtomicBool>,
    transfer_detected: &Arc<std::sync::atomic::AtomicBool>,
    batch_token: &tokio_util::sync::CancellationToken,
    tx: &mpsc::Sender<super::AgentEvent>,
) -> GroupOutcome {
    let abort_handles: Vec<_> = handles
        .iter()
        .map(|(_, handle)| handle.abort_handle())
        .collect();
    let mut futs: FuturesUnordered<ToolJoinFuture> = handles
        .into_iter()
        .map(|(idx, handle)| tool_join_future(idx, handle))
        .collect();

    loop {
        if futs.is_empty() {
            return GroupOutcome::Continue;
        }

        let Some((idx, join_result)) = (tokio::select! {
            biased;
            () = batch_token.cancelled() => {
                let completed =
                    cancel_remaining_with_bounded_wait(&mut futs, &abort_handles).await;
                for (idx, join_result) in completed {
                    if let Err(join_error) = join_result {
                        emit_join_failure(idx, join_error, tool_calls, results, tx).await;
                    }
                }
                return GroupOutcome::Aborted;
            }
            result = futs.next() => result
        }) else {
            return GroupOutcome::Continue;
        };

        if let Err(join_error) = join_result {
            emit_join_failure(idx, join_error, tool_calls, results, tx).await;
            continue;
        }

        if steering_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            let completed = cancel_remaining_with_bounded_wait(&mut futs, &abort_handles).await;
            for (idx, join_result) in completed {
                if let Err(join_error) = join_result {
                    emit_join_failure(idx, join_error, tool_calls, results, tx).await;
                }
            }
            return GroupOutcome::SteeringInterrupt;
        }

        if transfer_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            let completed = cancel_remaining_with_bounded_wait(&mut futs, &abort_handles).await;
            for (idx, join_result) in completed {
                if let Err(join_error) = join_result {
                    emit_join_failure(idx, join_error, tool_calls, results, tx).await;
                }
            }
            return GroupOutcome::TransferInterrupt;
        }
    }
}

// ─── Outcome builders ───────────────────────────────────────────────────────

/// Build a `ToolExecOutcome::SteeringInterrupt` from accumulated state.
pub(super) async fn build_steering_outcome(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    results: Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    steering_messages: Arc<tokio::sync::Mutex<Vec<AgentMessage>>>,
    injected_messages: Vec<AgentMessage>,
) -> ToolExecOutcome {
    let all_results = std::mem::take(&mut *results.lock().await);
    let result_map: HashMap<&str, &ToolResultMessage> = all_results
        .iter()
        .map(|(_, r)| (r.tool_call_id.as_str(), r))
        .collect();
    let mut completed: Vec<ToolResultMessage> = Vec::new();
    let mut cancelled: Vec<ToolResultMessage> = Vec::new();

    for tc in tool_calls {
        if let Some(result) = result_map.get(tc.id.as_str()) {
            completed.push((*result).clone());
        } else {
            cancelled.push(ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: vec![ContentBlock::Text {
                    text: "tool call cancelled: user requested steering interrupt".to_string(),
                }],
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
                cache_hint: None,
            });
        }
    }

    let mut steering_messages = std::mem::take(&mut *steering_messages.lock().await);
    if let Some(provider) = config.message_provider.as_ref() {
        steering_messages.extend(provider.poll_steering());
    }

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    ToolExecOutcome::SteeringInterrupt {
        completed,
        cancelled,
        steering_messages,
        tool_metrics: collected_timings,
        injected_messages,
    }
}

/// Build a completed outcome after a transfer signal cancels the remaining batch.
pub(super) async fn build_transfer_outcome(
    tool_calls: &[ToolCallInfo],
    results: Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    transfer_signal: Arc<tokio::sync::Mutex<Option<crate::transfer::TransferSignal>>>,
    injected_messages: Vec<AgentMessage>,
) -> ToolExecOutcome {
    let all_results = std::mem::take(&mut *results.lock().await);
    let result_map: HashMap<&str, &ToolResultMessage> = all_results
        .iter()
        .map(|(_, result)| (result.tool_call_id.as_str(), result))
        .collect();
    let mut ordered: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());

    for tc in tool_calls {
        if let Some(result) = result_map.get(tc.id.as_str()) {
            ordered.push((*result).clone());
        } else {
            ordered.push(ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: vec![ContentBlock::Text {
                    text: "tool call cancelled: transfer initiated".to_string(),
                }],
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
                cache_hint: None,
            });
        }
    }

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    let captured_transfer = transfer_signal.lock().await.take();
    ToolExecOutcome::Completed {
        results: ordered,
        tool_metrics: collected_timings,
        transfer_signal: captured_transfer,
        injected_messages,
    }
}

/// Build an aborted outcome after parent cancellation interrupts a tool batch.
pub(super) async fn build_aborted_outcome(
    tool_calls: &[ToolCallInfo],
    results: Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    injected_messages: Vec<AgentMessage>,
) -> ToolExecOutcome {
    let all_results = std::mem::take(&mut *results.lock().await);
    let result_map: HashMap<&str, &ToolResultMessage> = all_results
        .iter()
        .map(|(_, result)| (result.tool_call_id.as_str(), result))
        .collect();
    let mut ordered: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());

    for tc in tool_calls {
        if let Some(result) = result_map.get(tc.id.as_str()) {
            ordered.push((*result).clone());
        } else {
            ordered.push(ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: vec![ContentBlock::Text {
                    text: "tool call cancelled: operation aborted".to_string(),
                }],
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
                cache_hint: None,
            });
        }
    }

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    ToolExecOutcome::Aborted {
        results: ordered,
        tool_metrics: collected_timings,
        injected_messages,
    }
}
