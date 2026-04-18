//! Collect phase: result ordering, interrupt outcomes, metrics assembly.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tracing::error;

use crate::types::{AgentMessage, ContentBlock, ToolResultMessage};
use crate::util::now_timestamp;

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

// ─── Group result collection ────────────────────────────────────────────────

/// Collect results for a single execution group's spawned handles.
pub(super) async fn collect_group_results(
    tool_calls: &[ToolCallInfo],
    handles: Vec<(usize, tokio::task::JoinHandle<()>)>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_detected: &Arc<std::sync::atomic::AtomicBool>,
    transfer_detected: &Arc<std::sync::atomic::AtomicBool>,
    batch_token: &tokio_util::sync::CancellationToken,
) -> GroupOutcome {
    let abort_handles: Vec<_> = handles
        .iter()
        .map(|(_, handle)| handle.abort_handle())
        .collect();
    let mut futs: FuturesUnordered<_> = handles
        .into_iter()
        .map(|(idx, handle)| async move { (idx, handle.await) })
        .collect();

    loop {
        if futs.is_empty() {
            return GroupOutcome::Continue;
        }

        let Some((idx, join_result)) = (tokio::select! {
            biased;
            () = batch_token.cancelled() => {
                for handle in &abort_handles {
                    handle.abort();
                }
                while futs.next().await.is_some() {}
                return GroupOutcome::Aborted;
            }
            result = futs.next() => result
        }) else {
            return GroupOutcome::Continue;
        };

        if let Err(join_error) = join_result {
            let panic_message = if join_error.is_panic() {
                let panic_value = join_error.into_panic();
                panic_value
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic_value.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic payload".to_string())
            } else {
                format!("{join_error}")
            };

            let tc = &tool_calls[idx];
            error!(
                tool_call_id = %tc.id,
                tool_name = %tc.name,
                "tool execution panicked: {panic_message}"
            );

            let panic_result = ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: vec![ContentBlock::Text {
                    text: format!("tool execution panicked: {panic_message}"),
                }],
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
                cache_hint: None,
            };
            results.lock().await.push((idx, panic_result));
            continue;
        }

        if steering_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            for handle in &abort_handles {
                handle.abort();
            }
            while futs.next().await.is_some() {}
            return GroupOutcome::SteeringInterrupt;
        }

        if transfer_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            for handle in &abort_handles {
                handle.abort();
            }
            while futs.next().await.is_some() {}
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
