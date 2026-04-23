//! Shared helpers used across pre-process, execute, and collect phases.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::tool::AgentToolResult;
use crate::types::ToolResultMessage;
use crate::util::now_timestamp;

use super::{AgentEvent, ToolCallInfo, emit};

/// Bound on the per-tool partial-update buffer between the sync `on_update`
/// callback and the async forwarder task.
///
/// Partial updates are progressive by nature: downstream only needs the latest
/// state to render progress, so we treat the channel as lossy under sustained
/// backpressure (see `execute::dispatch_single_tool`). A value of 128 is large
/// enough to absorb normal bursts (typical tools emit a handful of updates)
/// without allowing unbounded memory growth when downstream lags.
pub(super) const TOOL_UPDATE_CHANNEL_CAPACITY: usize = 128;

/// Build an error `ToolResultMessage` and emit `ToolExecutionEnd`.
pub(super) async fn emit_error_result(
    tool_name: &str,
    tool_call_id: &str,
    error_result: AgentToolResult,
    idx: usize,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) {
    let _ = emit(
        tx,
        AgentEvent::ToolExecutionEnd {
            id: tool_call_id.to_string(),
            name: tool_name.to_string(),
            result: error_result.clone(),
            is_error: true,
        },
    )
    .await;

    let tool_result_msg = ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        content: error_result.content,
        is_error: true,
        timestamp: now_timestamp(),
        details: serde_json::Value::Null,
        cache_hint: None,
    };
    results.lock().await.push((idx, tool_result_msg));
}

/// Emit `ToolExecutionStart` event. Returns `true` if the channel is still open.
pub(super) async fn emit_tool_execution_start(
    tool_call_id: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
    tx: &mpsc::Sender<AgentEvent>,
) -> bool {
    emit(
        tx,
        AgentEvent::ToolExecutionStart {
            id: tool_call_id.to_string(),
            name: tool_name.to_string(),
            arguments: arguments.clone(),
        },
    )
    .await
}

/// Extract a human-readable message from a panic payload.
pub(super) fn panic_payload_message(panic_value: &(dyn std::any::Any + Send)) -> String {
    panic_value
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic_value.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string())
}

/// Forward partial tool updates from the bounded channel to the event stream.
pub(super) async fn forward_tool_updates(
    tool_call_id: &str,
    tool_name: &str,
    mut updates: mpsc::Receiver<AgentToolResult>,
    tx: &mpsc::Sender<AgentEvent>,
) {
    while let Some(partial) = updates.recv().await {
        if !emit(
            tx,
            AgentEvent::ToolExecutionUpdate {
                id: tool_call_id.to_string(),
                name: tool_name.to_string(),
                partial,
            },
        )
        .await
        {
            break;
        }
    }
}

/// Emit stop error results for every tool call that does not already have a
/// terminal result.
pub(super) async fn emit_batch_stop_results(
    tool_calls: &[ToolCallInfo],
    reason: &str,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) {
    let resolved_ids: HashSet<String> = {
        let guard = results.lock().await;
        guard
            .iter()
            .map(|(_, result)| result.tool_call_id.clone())
            .collect()
    };

    for (idx, tc) in tool_calls.iter().enumerate() {
        if resolved_ids.contains(&tc.id) {
            continue;
        }

        let error_result = AgentToolResult::error(format!(
            "policy stopped tool batch before dispatch: {reason}"
        ));
        emit_error_result(&tc.name, &tc.id, error_result, idx, results, tx).await;
    }
}
