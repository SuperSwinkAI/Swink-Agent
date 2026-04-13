use std::collections::HashMap;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;

use futures::{
    FutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, info_span};

use crate::tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
    validate_tool_arguments, validation_error_result,
};
use crate::tool_execution_policy::{ToolCallSummary, ToolExecutionPolicy};
use crate::types::{AgentMessage, ContentBlock, ToolResultMessage};
use crate::util::now_timestamp;

use crate::agent_options::ApproveToolFn;

use super::{AgentEvent, AgentLoopConfig, ToolCallInfo, ToolExecOutcome, emit};

// ─── Pre-processed tool call ─────────────────────────────────────────────────

/// A tool call that has passed approval, transformation, and validation gates.
struct PreparedToolCall {
    /// Index in the original `tool_calls` slice.
    idx: usize,
    /// Effective arguments after approval override and transformation.
    effective_arguments: serde_json::Value,
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// Build an error `ToolResultMessage` and emit `ToolExecutionEnd`.
async fn emit_error_result(
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

async fn emit_tool_execution_start(
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

fn panic_payload_message(panic_value: &(dyn std::any::Any + Send)) -> String {
    panic_value
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic_value.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string())
}

async fn forward_tool_updates(
    tool_call_id: &str,
    tool_name: &str,
    mut updates: mpsc::UnboundedReceiver<AgentToolResult>,
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

async fn emit_batch_stop_results(
    tool_calls: &[ToolCallInfo],
    stop_idx: usize,
    reason: &str,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) {
    for (idx, tc) in tool_calls.iter().enumerate().skip(stop_idx) {
        let error_result = AgentToolResult::error(format!(
            "policy stopped tool batch before dispatch: {reason}"
        ));
        emit_error_result(&tc.name, &tc.id, error_result, idx, results, tx).await;
    }
}

/// Order results to match the original `tool_calls` order, returning only
/// those whose IDs appear in the result set.
fn order_results_by_tool_calls(
    tool_calls: &[ToolCallInfo],
    all_results: &[(usize, ToolResultMessage)],
) -> Vec<ToolResultMessage> {
    let result_map: HashMap<&str, &ToolResultMessage> = all_results
        .iter()
        .map(|(_, r)| (r.tool_call_id.as_str(), r))
        .collect();
    let mut ordered: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        if let Some(result) = result_map.get(tc.id.as_str()) {
            ordered.push((*result).clone());
        }
    }
    ordered
}

// ─── Public entry point ──────────────────────────────────────────────────────

/// Execute tool calls using the configured [`ToolExecutionPolicy`].
///
/// Pre-processing (approval, transformation, validation) runs for every tool
/// call regardless of policy. The policy controls how the actual execution
/// is dispatched:
///
/// - **Concurrent** — spawn all at once via `tokio::spawn` (default).
/// - **Sequential** — execute one at a time in order.
/// - **Priority** — group by priority, execute groups sequentially (concurrent
///   within each group).
/// - **Custom** — delegate grouping to a [`ToolExecutionStrategy`](crate::tool_execution_policy::ToolExecutionStrategy).
#[allow(clippy::too_many_lines)]
pub async fn execute_tools_concurrently(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> ToolExecOutcome {
    use tokio::sync::Mutex;

    let tool_names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
    info!(
        tool_count = tool_calls.len(),
        tools = ?tool_names,
        policy = ?config.tool_execution_policy,
        "dispatching tool batch"
    );

    let batch_token = cancellation_token.child_token();
    let results: Arc<Mutex<Vec<(usize, ToolResultMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let tool_timings: Arc<Mutex<Vec<crate::metrics::ToolExecMetrics>>> =
        Arc::new(Mutex::new(Vec::new()));
    let steering_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transfer_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transfer_signal: Arc<Mutex<Option<crate::transfer::TransferSignal>>> =
        Arc::new(Mutex::new(None));

    // Pre-build a tool lookup map for O(1) dispatch by name.
    let tool_map: HashMap<&str, &Arc<dyn AgentTool>> =
        config.tools.iter().map(|t| (t.name(), t)).collect();

    // Phase 1: Pre-process all tool calls (approval, transform, validate).
    let mut prepared: Vec<PreparedToolCall> = Vec::new();
    // Messages injected by PreDispatch policies (Inject verdict). These are
    // propagated via the outcome so the loop can append them to pending_messages.
    let mut injected_messages: Vec<AgentMessage> = Vec::new();

    for (idx, tc) in tool_calls.iter().enumerate() {
        // ── PreDispatch policies ──
        let mut effective_arguments = tc.arguments.clone();
        {
            use crate::policy::{
                PreDispatchVerdict, ToolDispatchContext, run_pre_dispatch_policies,
            };

            let state_snapshot = {
                let guard = config
                    .session_state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.clone()
            };
            let mut dispatch_ctx = ToolDispatchContext {
                tool_name: &tc.name,
                tool_call_id: &tc.id,
                arguments: &mut effective_arguments,
                // Do not guess at a trust boundary. Until the runtime can
                // prove a tool-specific execution root, policies must treat it
                // as unknown rather than inheriting the host process CWD.
                execution_root: None,
                state: &state_snapshot,
            };
            match run_pre_dispatch_policies(&config.pre_dispatch_policies, &mut dispatch_ctx) {
                PreDispatchVerdict::Continue => {}
                PreDispatchVerdict::Inject(msgs) => {
                    injected_messages.extend(msgs);
                }
                PreDispatchVerdict::Stop(reason) => {
                    emit_batch_stop_results(tool_calls, idx, &reason, &results, tx).await;
                    // Return all results collected so far
                    let all_results = std::mem::take(&mut *results.lock().await);
                    let ordered = order_results_by_tool_calls(tool_calls, &all_results);
                    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
                    return ToolExecOutcome::Completed {
                        results: ordered,
                        tool_metrics: collected_timings,
                        transfer_signal: None,
                        injected_messages,
                    };
                }
                PreDispatchVerdict::Skip(error_text) => {
                    let error_result = AgentToolResult {
                        content: vec![ContentBlock::Text { text: error_text }],
                        details: serde_json::Value::Null,
                        is_error: true,
                        transfer_signal: None,
                    };
                    emit_error_result(&tc.name, &tc.id, error_result, idx, &results, tx).await;
                    continue;
                }
            }
        }

        // ── Approval gate ──
        if let Some(ref approve_fn) = config.approve_tool
            && config.approval_mode != ApprovalMode::Bypassed
        {
            let requires_approval = tool_map
                .get(tc.name.as_str())
                .is_some_and(|t| t.requires_approval());

            // In Smart mode, auto-approve tools that declare requires_approval() == false.
            // Only Enabled mode routes every tool through the callback unconditionally.
            let should_call_approval = match config.approval_mode {
                ApprovalMode::Smart => requires_approval,
                ApprovalMode::Enabled => true,
                ApprovalMode::Bypassed => unreachable!(), // filtered above
            };

            if should_call_approval {
                match check_approval(
                    approve_fn,
                    tc,
                    &effective_arguments,
                    idx,
                    requires_approval,
                    &tool_map,
                    &results,
                    tx,
                )
                .await
                {
                    ApprovalOutcome::Approved => {}
                    ApprovalOutcome::ApprovedWith(new_params) => {
                        effective_arguments = new_params;
                    }
                    ApprovalOutcome::Rejected => continue,
                    ApprovalOutcome::ChannelClosed => return ToolExecOutcome::ChannelClosed,
                }
            }
        }

        prepared.push(PreparedToolCall {
            idx,
            effective_arguments,
        });
    }

    // Phase 2: Compute execution groups based on policy.
    let groups =
        compute_execution_groups(&config.tool_execution_policy, tool_calls, &prepared).await;

    // Phase 3: Execute each group. Within a group, tools run concurrently.
    // Groups run sequentially.
    for group in groups {
        let mut handles = Vec::new();

        for &prepared_idx in &group {
            let prep = &prepared[prepared_idx];
            let tc = &tool_calls[prep.idx];

            let handle = dispatch_single_tool(
                &tool_map,
                config,
                tc,
                &prep.effective_arguments,
                prep.idx,
                &batch_token,
                &results,
                &tool_timings,
                &steering_detected,
                &transfer_detected,
                &transfer_signal,
                tx,
            )
            .await;

            match handle {
                DispatchResult::Spawned(h) => handles.push((prep.idx, h)),
                DispatchResult::Inline => {}
                DispatchResult::ChannelClosed => return ToolExecOutcome::ChannelClosed,
            }
        }

        // Collect results for this group before proceeding to the next.
        let group_outcome = collect_group_results(
            tool_calls,
            handles,
            &results,
            &steering_detected,
            &transfer_detected,
            &batch_token,
        )
        .await;

        match group_outcome {
            GroupOutcome::Continue => {}
            GroupOutcome::SteeringInterrupt => {
                return build_steering_outcome(
                    config,
                    tool_calls,
                    results,
                    tool_timings,
                    injected_messages,
                )
                .await;
            }
            GroupOutcome::TransferInterrupt => {
                return build_transfer_outcome(
                    tool_calls,
                    results,
                    tool_timings,
                    transfer_signal,
                    injected_messages,
                )
                .await;
            }
        }
    }

    // All groups completed without steering interrupts.
    let all_results = std::mem::take(&mut *results.lock().await);
    let ordered = order_results_by_tool_calls(tool_calls, &all_results);

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    let captured_transfer = transfer_signal.lock().await.take();
    ToolExecOutcome::Completed {
        results: ordered,
        tool_metrics: collected_timings,
        transfer_signal: captured_transfer,
        injected_messages,
    }
}

// ─── Execution group computation ─────────────────────────────────────────────

/// Compute execution groups from the policy. Returns groups of indices into the
/// `prepared` slice. Tools within a group execute concurrently; groups execute
/// sequentially.
async fn compute_execution_groups(
    policy: &ToolExecutionPolicy,
    tool_calls: &[ToolCallInfo],
    prepared: &[PreparedToolCall],
) -> Vec<Vec<usize>> {
    if prepared.is_empty() {
        return vec![];
    }

    match policy {
        ToolExecutionPolicy::Concurrent => {
            // Single group containing all prepared tool calls.
            vec![(0..prepared.len()).collect()]
        }
        ToolExecutionPolicy::Sequential => {
            // Each tool in its own group.
            (0..prepared.len()).map(|i| vec![i]).collect()
        }
        ToolExecutionPolicy::Priority(priority_fn) => {
            // Assign priorities, sort descending, group by equal priority.
            let mut scored: Vec<(usize, i32)> = prepared
                .iter()
                .enumerate()
                .map(|(prep_idx, prep)| {
                    let tc = &tool_calls[prep.idx];
                    let summary = ToolCallSummary {
                        id: &tc.id,
                        name: &tc.name,
                        arguments: &prep.effective_arguments,
                    };
                    (prep_idx, priority_fn(&summary))
                })
                .collect();

            // Sort by priority descending (stable sort preserves original order
            // for equal priorities).
            scored.sort_by(|a, b| b.1.cmp(&a.1));

            // Group consecutive items with the same priority.
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut current_priority = None;

            for (prep_idx, priority) in scored {
                if current_priority == Some(priority) {
                    // Safe: a new group is always pushed when priority changes,
                    // so `groups` is non-empty here.
                    if let Some(last) = groups.last_mut() {
                        last.push(prep_idx);
                    }
                } else {
                    current_priority = Some(priority);
                    groups.push(vec![prep_idx]);
                }
            }

            groups
        }
        ToolExecutionPolicy::Custom(strategy) => {
            // Build summaries for the strategy.
            let summaries: Vec<ToolCallSummary<'_>> = prepared
                .iter()
                .map(|prep| {
                    let tc = &tool_calls[prep.idx];
                    ToolCallSummary {
                        id: &tc.id,
                        name: &tc.name,
                        arguments: &prep.effective_arguments,
                    }
                })
                .collect();

            strategy.partition(&summaries).await
        }
    }
}

// ─── Group result collection ─────────────────────────────────────────────────

/// Outcome of collecting results for a single execution group.
enum GroupOutcome {
    /// All tools in the group completed; proceed to next group.
    Continue,
    /// Steering interrupt detected; abort remaining groups.
    SteeringInterrupt,
    /// Transfer detected; abort remaining groups and end the turn.
    TransferInterrupt,
}

/// Collect results for a single execution group's spawned handles.
async fn collect_group_results(
    tool_calls: &[ToolCallInfo],
    handles: Vec<(usize, tokio::task::JoinHandle<()>)>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_detected: &Arc<std::sync::atomic::AtomicBool>,
    transfer_detected: &Arc<std::sync::atomic::AtomicBool>,
    batch_token: &CancellationToken,
) -> GroupOutcome {
    let abort_handles: Vec<_> = handles
        .iter()
        .map(|(_, handle)| handle.abort_handle())
        .collect();
    let mut futs: FuturesUnordered<_> = handles
        .into_iter()
        .map(|(idx, handle)| async move { (idx, handle.await) })
        .collect();

    while let Some((idx, join_result)) = futs.next().await {
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
            // Drain remaining futures after aborting them so the group can
            // complete even when a tool ignores the cancellation token.
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

    GroupOutcome::Continue
}

/// Build a `ToolExecOutcome::SteeringInterrupt` from accumulated state.
async fn build_steering_outcome(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    results: Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
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

    let steering_messages = config
        .message_provider
        .as_ref()
        .map_or_else(Vec::new, |provider| provider.poll_steering());

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
async fn build_transfer_outcome(
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

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Result of checking the approval gate for a single tool call.
enum ApprovalOutcome {
    Approved,
    /// Approved with modified parameters.
    ApprovedWith(serde_json::Value),
    Rejected,
    ChannelClosed,
}

/// Run the approval gate for a single tool call: emit events, call callback, handle rejection.
///
/// `effective_arguments` are the post-rewrite arguments after pre-dispatch
/// policies have been applied. These are shown to the approval callback so the
/// approver sees the actual arguments that will be executed.
#[allow(clippy::too_many_arguments)]
async fn check_approval(
    approve_fn: &ApproveToolFn,
    tc: &ToolCallInfo,
    effective_arguments: &serde_json::Value,
    idx: usize,
    requires_approval: bool,
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) -> ApprovalOutcome {
    if !emit(
        tx,
        AgentEvent::ToolApprovalRequested {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: effective_arguments.clone(),
        },
    )
    .await
    {
        return ApprovalOutcome::ChannelClosed;
    }

    // Resolve approval context with panic safety.
    let approval_context = tool_map.get(tc.name.as_str()).and_then(|tool| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tool.approval_context(effective_arguments)
        }))
        .unwrap_or_else(|_| {
            tracing::warn!(tool_name = %tc.name, "approval_context() panicked — using None");
            None
        })
    });

    let request = ToolApprovalRequest {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        arguments: effective_arguments.clone(),
        requires_approval,
        context: approval_context,
    };
    let decision = match std::panic::AssertUnwindSafe(approve_fn(request))
        .catch_unwind()
        .await
    {
        Ok(decision) => decision,
        Err(panic_value) => {
            let panic_message = panic_payload_message(panic_value.as_ref());
            error!(
                tool_call_id = %tc.id,
                tool_name = %tc.name,
                "approval callback panicked: {panic_message}"
            );

            if !emit(
                tx,
                AgentEvent::ToolApprovalResolved {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    approved: false,
                },
            )
            .await
            {
                return ApprovalOutcome::ChannelClosed;
            }

            emit_error_result(
                &tc.name,
                &tc.id,
                AgentToolResult::error(format!(
                    "Tool call '{}' was rejected because the approval callback panicked: \
                     {panic_message}",
                    tc.name
                )),
                idx,
                results,
                tx,
            )
            .await;
            return ApprovalOutcome::Rejected;
        }
    };
    let approved = !matches!(decision, ToolApproval::Rejected);

    if !emit(
        tx,
        AgentEvent::ToolApprovalResolved {
            id: tc.id.clone(),
            name: tc.name.clone(),
            approved,
        },
    )
    .await
    {
        return ApprovalOutcome::ChannelClosed;
    }

    match decision {
        ToolApproval::Approved => ApprovalOutcome::Approved,
        ToolApproval::ApprovedWith(new_params) => ApprovalOutcome::ApprovedWith(new_params),
        ToolApproval::Rejected => {
            let rejection_result = AgentToolResult::error(format!(
                "Tool call '{}' was rejected by the approval gate.",
                tc.name
            ));
            emit_error_result(&tc.name, &tc.id, rejection_result, idx, results, tx).await;
            ApprovalOutcome::Rejected
        }
    }
}

/// Result of dispatching a single tool call.
enum DispatchResult {
    /// Tool was spawned as a tokio task.
    Spawned(tokio::task::JoinHandle<()>),
    /// Tool result was added inline (unknown tool).
    Inline,
    /// Event channel closed before execution could start.
    ChannelClosed,
}

/// Validate and dispatch a single tool call, returning a join handle or inline result.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn dispatch_single_tool(
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    config: &Arc<AgentLoopConfig>,
    tc: &ToolCallInfo,
    effective_arguments: &serde_json::Value,
    idx: usize,
    batch_token: &CancellationToken,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: &Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    steering_flag: &Arc<std::sync::atomic::AtomicBool>,
    transfer_flag: &Arc<std::sync::atomic::AtomicBool>,
    transfer_signal: &Arc<tokio::sync::Mutex<Option<crate::transfer::TransferSignal>>>,
    tx: &mpsc::Sender<AgentEvent>,
) -> DispatchResult {
    let tool = tool_map.get(tc.name.as_str()).copied();

    let tool_call_id = tc.id.clone();
    let tool_name = tc.name.clone();
    let arguments = effective_arguments.clone();

    let Some(tool) = tool else {
        // Unknown tool
        let error_result = crate::tool::unknown_tool_result(&tool_name);
        emit_error_result(&tool_name, &tool_call_id, error_result, idx, results, tx).await;
        return DispatchResult::Inline;
    };

    let tool = Arc::clone(tool);
    let child_token = batch_token.child_token();
    let results_clone = Arc::clone(results);
    let timings_clone = Arc::clone(tool_timings);
    let steering_clone = Arc::clone(steering_flag);
    let transfer_flag_clone = Arc::clone(transfer_flag);
    let transfer_clone = Arc::clone(transfer_signal);
    let config_clone = Arc::clone(config);
    let tx_clone = tx.clone();

    let validation = validate_tool_arguments(tool.parameters_schema(), &arguments);
    if validation.is_ok()
        && !emit_tool_execution_start(&tool_call_id, &tool_name, &arguments, tx).await
    {
        return DispatchResult::ChannelClosed;
    }

    let tool_span = info_span!(
        "agent.tool",
        agent.tool.name = %tool_name,
        tool_call_id = %tool_call_id,
    );
    let handle = tokio::spawn(
        async move {
            debug!(tool = %tool_name, id = %tool_call_id, "tool execution starting");
            let exec_start = std::time::Instant::now();
            let (result, is_error) = if let Err(errors) = validation {
                (validation_error_result(&errors), true)
            } else {
                // ── Credential resolution (zero overhead when no auth_config) ──
                match resolve_credential(&tool, &config_clone, &tool_call_id).await {
                    Err(cred_error) => (AgentToolResult::error(format!("{cred_error}")), true),
                    Ok(credential) => {
                        let (update_tx, update_rx) = mpsc::unbounded_channel();
                        let updates_tx = tx_clone.clone();
                        let updates_tool_call_id = tool_call_id.clone();
                        let updates_tool_name = tool_name.clone();
                        let update_forwarder = tokio::spawn(async move {
                            forward_tool_updates(
                                &updates_tool_call_id,
                                &updates_tool_name,
                                update_rx,
                                &updates_tx,
                            )
                            .await;
                        });
                        let result = {
                            let on_update = Box::new(move |partial: AgentToolResult| {
                                let _ = update_tx.send(partial);
                            });
                            tool.execute(
                                &tool_call_id,
                                arguments,
                                child_token,
                                Some(on_update),
                                config_clone.session_state.clone(),
                                credential,
                            )
                            .await
                        };
                        let _ = update_forwarder.await;
                        let is_error = result.is_error;
                        (result, is_error)
                    }
                }
            };
            let exec_duration = exec_start.elapsed();
            debug!(tool = %tool_name, id = %tool_call_id, is_error, "tool execution finished");

            let event_tool_name = tool_name.clone();
            timings_clone
                .lock()
                .await
                .push(crate::metrics::ToolExecMetrics {
                    tool_name,
                    duration: exec_duration,
                    success: !is_error,
                });

            // Capture transfer signal (first one wins)
            if result.is_transfer() {
                let mut guard = transfer_clone.lock().await;
                if guard.is_none() {
                    (*guard).clone_from(&result.transfer_signal);
                }
                drop(guard);
                transfer_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }

            let _ = emit(
                &tx_clone,
                AgentEvent::ToolExecutionEnd {
                    id: tool_call_id.clone(),
                    name: event_tool_name,
                    result: result.clone(),
                    is_error,
                },
            )
            .await;

            let tool_result_msg = ToolResultMessage {
                tool_call_id,
                content: result.content,
                is_error,
                timestamp: now_timestamp(),
                details: result.details,
                cache_hint: None,
            };

            results_clone.lock().await.push((idx, tool_result_msg));

            if let Some(ref provider) = config_clone.message_provider {
                let msgs = provider.poll_steering();
                if !msgs.is_empty() {
                    steering_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        .instrument(tool_span),
    );

    DispatchResult::Spawned(handle)
}

// ─── Credential resolution helper ───────────────────────────────────────────

/// Resolve credentials for a tool, if it declares an `auth_config()`.
///
/// Returns `Ok(None)` for unauthenticated tools (zero overhead path).
/// Returns `Ok(Some(credential))` on successful resolution.
/// Returns `Err(error_message)` on resolution failure (key name only, no secrets).
async fn resolve_credential(
    tool: &Arc<dyn AgentTool>,
    config: &Arc<AgentLoopConfig>,
    _tool_call_id: &str,
) -> Result<Option<crate::credential::ResolvedCredential>, crate::credential::CredentialError> {
    let Some(auth_config) = tool.auth_config() else {
        return Ok(None); // No auth required — zero overhead
    };

    let cred_resolver = config.credential_resolver.as_ref().ok_or_else(|| {
        crate::credential::CredentialError::NotFound {
            key: auth_config.credential_key.clone(),
        }
    })?;

    // Resolve with timeout (default 30s)
    let resolve_future = cred_resolver.resolve(&auth_config.credential_key);
    let credential = tokio::time::timeout(std::time::Duration::from_secs(30), resolve_future)
        .await
        .map_err(|_| crate::credential::CredentialError::Timeout {
            key: auth_config.credential_key.clone(),
        })??;

    // Type mismatch check (FR-018)
    let actual_type = match &credential {
        crate::credential::ResolvedCredential::ApiKey(_) => {
            crate::credential::CredentialType::ApiKey
        }
        crate::credential::ResolvedCredential::Bearer(_) => {
            crate::credential::CredentialType::Bearer
        }
        crate::credential::ResolvedCredential::OAuth2AccessToken(_) => {
            crate::credential::CredentialType::OAuth2
        }
    };
    if actual_type != auth_config.credential_type {
        return Err(crate::credential::CredentialError::TypeMismatch {
            key: auth_config.credential_key,
            expected: auth_config.credential_type,
            actual: actual_type,
        });
    }

    Ok(Some(credential))
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;

    use std::future::Future;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::{pin::Pin, sync::Mutex as StdSyncMutex};

    use serde_json::json;
    use tokio::sync::mpsc;

    use crate::policy::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
    use crate::testing::{MockStreamFn, MockTool, default_convert, default_model};
    use crate::{
        ApprovalMode, DefaultRetryStrategy, StreamOptions, ToolApproval, ToolExecutionPolicy,
    };

    struct BurstUpdatingTool {
        update_count: usize,
    }

    impl AgentTool for BurstUpdatingTool {
        fn name(&self) -> &str {
            "burst_tool"
        }

        fn label(&self) -> &str {
            "burst_tool"
        }

        fn description(&self) -> &'static str {
            "Emits a burst of partial updates"
        }

        fn parameters_schema(&self) -> &serde_json::Value {
            static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": true
                })
            })
        }

        fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancellation_token: CancellationToken,
            on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
            _credential: Option<crate::ResolvedCredential>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            let update_count = self.update_count;
            Box::pin(async move {
                if let Some(on_update) = on_update {
                    for idx in 0..update_count {
                        on_update(AgentToolResult::text(format!("partial-{idx}")));
                    }
                }
                AgentToolResult::text("done")
            })
        }
    }

    struct ExecutionRootRecorder {
        saw_none: Arc<AtomicBool>,
        captured_roots: Arc<StdMutex<Vec<Option<PathBuf>>>>,
    }

    struct StopBatchPolicy;

    impl PreDispatchPolicy for StopBatchPolicy {
        fn name(&self) -> &str {
            "stop-batch"
        }

        fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            PreDispatchVerdict::Stop("blocked by policy".to_string())
        }
    }

    impl PreDispatchPolicy for ExecutionRootRecorder {
        fn name(&self) -> &str {
            "execution-root-recorder"
        }

        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            self.saw_none
                .store(ctx.execution_root.is_none(), Ordering::SeqCst);
            self.captured_roots
                .lock()
                .unwrap()
                .push(ctx.execution_root.map(std::path::Path::to_path_buf));
            PreDispatchVerdict::Continue
        }
    }

    fn test_loop_config(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
    ) -> Arc<AgentLoopConfig> {
        test_loop_config_with_options(
            pre_dispatch_policies,
            vec![],
            None,
            crate::ApprovalMode::Bypassed,
        )
    }

    fn test_loop_config_with_options(
        pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>,
        tools: Vec<Arc<dyn AgentTool>>,
        approve_tool: Option<Box<crate::agent_options::ApproveToolFn>>,
        approval_mode: ApprovalMode,
    ) -> Arc<AgentLoopConfig> {
        Arc::new(AgentLoopConfig {
            agent_name: None,
            transfer_chain: None,
            model: default_model(),
            stream_options: StreamOptions::default(),
            retry_strategy: Box::new(DefaultRetryStrategy::default()),
            stream_fn: Arc::new(MockStreamFn::new(vec![])),
            tools,
            convert_to_llm: Box::new(default_convert),
            transform_context: None,
            get_api_key: None,
            message_provider: None,
            approve_tool,
            approval_mode,
            pre_turn_policies: vec![],
            pre_dispatch_policies,
            post_turn_policies: vec![],
            post_loop_policies: vec![],
            async_transform_context: None,
            metrics_collector: None,
            fallback: None,
            tool_execution_policy: ToolExecutionPolicy::Concurrent,
            session_state: Arc::new(std::sync::RwLock::new(crate::SessionState::new())),
            credential_resolver: None,
            cache_config: None,
            cache_state: std::sync::Mutex::new(crate::CacheState::default()),
            dynamic_system_prompt: None,
        })
    }

    fn drain_events(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn pre_dispatch_execution_root_is_none_when_runtime_cannot_prove_it() {
        let saw_none = Arc::new(AtomicBool::new(false));
        let captured_roots = Arc::new(StdMutex::new(Vec::new()));
        let recorder = Arc::new(ExecutionRootRecorder {
            saw_none: Arc::clone(&saw_none),
            captured_roots: Arc::clone(&captured_roots),
        });
        let config = test_loop_config(vec![recorder]);
        let tool_calls = vec![ToolCallInfo {
            id: "call_1".to_string(),
            name: "unknown_tool".to_string(),
            arguments: serde_json::json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        assert!(
            matches!(outcome, ToolExecOutcome::Completed { .. }),
            "expected completed outcome"
        );
        assert!(
            saw_none.load(Ordering::SeqCst),
            "pre-dispatch policy should see execution_root=None"
        );
        assert_eq!(
            captured_roots.lock().unwrap().as_slice(),
            &[None],
            "execution_root should remain unknown until a tool-specific root is available"
        );
    }

    #[tokio::test]
    async fn pre_dispatch_stop_preserves_result_parity_for_remaining_tool_calls() {
        let config = test_loop_config(vec![Arc::new(StopBatchPolicy)]);
        let tool_calls = vec![
            ToolCallInfo {
                id: "call_1".to_string(),
                name: "tool_one".to_string(),
                arguments: serde_json::json!({ "first": true }),
                is_incomplete: false,
            },
            ToolCallInfo {
                id: "call_2".to_string(),
                name: "tool_two".to_string(),
                arguments: serde_json::json!({ "second": true }),
                is_incomplete: false,
            },
        ];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 2, "each tool call should receive a result");
        assert_eq!(
            results
                .iter()
                .map(|result| result.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["call_1", "call_2"]
        );
        assert!(
            results.iter().all(|result| result.is_error),
            "stopped tool calls should surface as errors"
        );
        assert!(
            results.iter().all(|result| {
                matches!(
                    result.content.as_slice(),
                    [ContentBlock::Text { text }]
                        if text.contains("policy stopped tool batch before dispatch")
                )
            }),
            "synthetic results should explain the batch stop"
        );

        let mut start_ids = Vec::new();
        let mut end_ids = Vec::new();
        for event in drain_events(&mut rx) {
            match event {
                AgentEvent::ToolExecutionStart { id, .. } => start_ids.push(id),
                AgentEvent::ToolExecutionEnd { id, .. } => end_ids.push(id),
                _ => {}
            }
        }

        assert!(
            start_ids.is_empty(),
            "synthetic stop results should not emit ToolExecutionStart"
        );
        assert_eq!(end_ids, vec!["call_1".to_string(), "call_2".to_string()]);
    }

    #[tokio::test]
    async fn invalid_tool_arguments_do_not_emit_start_event() {
        let tool = Arc::new(MockTool::new("write_file").with_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"],
            "additionalProperties": false
        })));
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn AgentTool>],
            None,
            ApprovalMode::Bypassed,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_invalid".to_string(),
            name: "write_file".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert_eq!(tool.execution_count(), 0);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "schema-invalid calls must not look started");
    }

    #[tokio::test]
    async fn unknown_tools_do_not_emit_start_event() {
        let config = test_loop_config(vec![]);
        let tool_calls = vec![ToolCallInfo {
            id: "call_unknown".to_string(),
            name: "unknown_tool".to_string(),
            arguments: json!({ "path": "ghost.txt" }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "unknown tools must not look started");
    }

    #[tokio::test]
    async fn approval_rejection_does_not_emit_start_event() {
        let tool = Arc::new(MockTool::new("delete_file").with_requires_approval(true));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> =
            Box::new(|_request| Box::pin(async { ToolApproval::Rejected }));
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_rejected".to_string(),
            name: "delete_file".to_string(),
            arguments: json!({ "path": "danger.txt" }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert_eq!(tool.execution_count(), 0);

        let start_count = drain_events(&mut rx)
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .count();
        assert_eq!(start_count, 0, "approval rejection must not look started");
    }

    #[tokio::test]
    async fn tool_execution_start_uses_approved_arguments() {
        let tool = Arc::new(MockTool::new("write_file"));
        let approve_tool: Box<crate::agent_options::ApproveToolFn> = Box::new(|_request| {
            Box::pin(async {
                ToolApproval::ApprovedWith(json!({
                    "path": "rewritten.txt",
                    "content": "updated"
                }))
            })
        });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool.clone() as Arc<dyn AgentTool>],
            Some(approve_tool),
            ApprovalMode::Enabled,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_rewritten".to_string(),
            name: "write_file".to_string(),
            arguments: json!({
                "path": "original.txt",
                "content": "old"
            }),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(8);

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);
        assert_eq!(tool.execution_count(), 1);

        let start_events: Vec<_> = drain_events(&mut rx)
            .into_iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionStart {
                    id,
                    name,
                    arguments,
                } => Some((id, name, arguments)),
                _ => None,
            })
            .collect();
        assert_eq!(start_events.len(), 1);
        assert_eq!(start_events[0].0, "call_rewritten");
        assert_eq!(start_events[0].1, "write_file");
        assert_eq!(
            start_events[0].2,
            json!({
                "path": "rewritten.txt",
                "content": "updated"
            })
        );
    }

    #[tokio::test]
    async fn tool_execution_updates_include_identity_and_survive_backpressure() {
        let tool = Arc::new(BurstUpdatingTool { update_count: 32 });
        let config = test_loop_config_with_options(
            vec![],
            vec![tool as Arc<dyn AgentTool>],
            None,
            ApprovalMode::Bypassed,
        );
        let tool_calls = vec![ToolCallInfo {
            id: "call_updates".to_string(),
            name: "burst_tool".to_string(),
            arguments: json!({}),
            is_incomplete: false,
        }];
        let cancellation_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(1);
        let collected = StdArc::new(StdSyncMutex::new(Vec::new()));
        let collected_clone = StdArc::clone(&collected);
        let receiver = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                collected_clone.lock().unwrap().push(event);
            }
        });

        let outcome =
            execute_tools_concurrently(&config, &tool_calls, &cancellation_token, &tx).await;
        drop(tx);
        receiver.await.unwrap();

        let ToolExecOutcome::Completed { results, .. } = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(results.len(), 1);

        let events = collected.lock().unwrap();
        let updates: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionUpdate { id, name, partial } => Some((
                    id.clone(),
                    name.clone(),
                    ContentBlock::extract_text(&partial.content),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 32, "partial updates should not be dropped");
        assert!(
            updates
                .iter()
                .all(|(id, name, _)| id == "call_updates" && name == "burst_tool"),
            "partial updates should carry the originating tool identity"
        );
        assert_eq!(
            updates.first().map(|(_, _, text)| text.as_str()),
            Some("partial-0")
        );
        assert_eq!(
            updates.last().map(|(_, _, text)| text.as_str()),
            Some("partial-31")
        );
    }
}
