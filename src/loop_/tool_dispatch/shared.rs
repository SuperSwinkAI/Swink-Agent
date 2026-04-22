//! Shared helpers used across pre-process, execute, and collect phases.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::{Notify, mpsc};

use crate::tool::AgentToolResult;
use crate::types::ToolResultMessage;
use crate::util::now_timestamp;

use super::{AgentEvent, ToolCallInfo, emit};

pub(super) const TOOL_UPDATE_BUFFER_CAPACITY: usize = 16;

#[derive(Default)]
struct ToolUpdateRelayState {
    pending: VecDeque<AgentToolResult>,
    latest_overflow: Option<AgentToolResult>,
    closed: bool,
}

pub(super) struct ToolUpdateRelay {
    state: std::sync::Mutex<ToolUpdateRelayState>,
    notify: Notify,
}

impl ToolUpdateRelay {
    pub(super) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(ToolUpdateRelayState::default()),
            notify: Notify::new(),
        }
    }

    pub(super) fn push(&self, partial: AgentToolResult) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }

        if state.pending.len() < TOOL_UPDATE_BUFFER_CAPACITY {
            state.pending.push_back(partial);
        } else {
            // Once the buffer is full, retain only the latest overflow update so
            // tools cannot grow memory without bound while downstream observers
            // are backpressured.
            state.latest_overflow = Some(partial);
        }
        drop(state);
        self.notify.notify_one();
    }

    pub(super) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        drop(state);
        self.notify.notify_waiters();
    }

    async fn recv(&self) -> Option<AgentToolResult> {
        loop {
            let notified = self.notify.notified();
            let next = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.pending.pop_front().map_or_else(
                    || {
                        state.latest_overflow.take().map_or_else(
                            || if state.closed { Some(None) } else { None },
                            |partial| Some(Some(partial)),
                        )
                    },
                    |partial| Some(Some(partial)),
                )
            };

            if let Some(partial) = next {
                return partial;
            }

            notified.await;
        }
    }
}

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

/// Forward partial tool updates from the bounded relay to the event stream.
pub(super) async fn forward_tool_updates(
    tool_call_id: &str,
    tool_name: &str,
    updates: Arc<ToolUpdateRelay>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentBlock;

    #[tokio::test]
    async fn tool_update_relay_coalesces_latest_overflow_update() {
        let relay = ToolUpdateRelay::new();
        for idx in 0..(TOOL_UPDATE_BUFFER_CAPACITY + 8) {
            relay.push(AgentToolResult::text(format!("partial-{idx}")));
        }
        relay.close();

        let mut drained = Vec::new();
        while let Some(update) = relay.recv().await {
            drained.push(ContentBlock::extract_text(&update.content));
        }

        assert_eq!(drained.len(), TOOL_UPDATE_BUFFER_CAPACITY + 1);
        assert_eq!(drained.first().map(String::as_str), Some("partial-0"));
        assert_eq!(
            drained.last().map(String::as_str),
            Some("partial-23"),
            "the relay should preserve the latest overflow update"
        );
    }
}
