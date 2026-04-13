//! Pre-process phase: pre-dispatch policies, approval gate, argument rewriting.

use std::collections::HashMap;
use std::sync::Arc;

use futures::FutureExt;
use tokio::sync::mpsc;
use tracing::error;

use crate::agent_options::ApproveToolFn;
use crate::policy::{PreDispatchVerdict, ToolDispatchContext, run_pre_dispatch_policies};
use crate::tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
};
use crate::types::{AgentMessage, ContentBlock};

use super::shared::{emit_batch_stop_results, emit_error_result, panic_payload_message};
use super::{
    AgentEvent, AgentLoopConfig, PreparedToolCall, ToolCallInfo, ToolExecOutcome,
    emit, order_results_by_tool_calls,
};

// ─── Pre-process result ─────────────────────────────────────────────────────

/// Result of the pre-processing phase for a tool batch.
pub(super) struct PreprocessResult {
    /// Tool calls that passed all gates and are ready for dispatch.
    pub prepared: Vec<PreparedToolCall>,
    /// Messages injected by `PreDispatch` policies (Inject verdict).
    pub injected_messages: Vec<AgentMessage>,
}

/// Result of checking the approval gate for a single tool call.
enum ApprovalOutcome {
    Approved,
    /// Approved with modified parameters.
    ApprovedWith(serde_json::Value),
    Rejected,
    ChannelClosed,
}

// ─── Pre-process entry point ────────────────────────────────────────────────

/// Run pre-dispatch policies and the approval gate for every tool call.
///
/// Returns `Ok(PreprocessResult)` when pre-processing completes (even if some
/// calls were skipped/rejected). Returns `Err(ToolExecOutcome)` for early
/// exits (policy Stop, channel closed).
pub(super) async fn preprocess_tool_calls(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, crate::types::ToolResultMessage)>>>,
    tool_timings: &Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<PreprocessResult, ToolExecOutcome> {
    let mut prepared: Vec<PreparedToolCall> = Vec::new();
    let mut injected_messages: Vec<AgentMessage> = Vec::new();

    for (idx, tc) in tool_calls.iter().enumerate() {
        // ── PreDispatch policies ──
        let mut effective_arguments = tc.arguments.clone();
        {
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
                execution_root: None,
                state: &state_snapshot,
            };
            match run_pre_dispatch_policies(&config.pre_dispatch_policies, &mut dispatch_ctx) {
                PreDispatchVerdict::Continue => {}
                PreDispatchVerdict::Inject(msgs) => {
                    injected_messages.extend(msgs);
                }
                PreDispatchVerdict::Stop(reason) => {
                    emit_batch_stop_results(tool_calls, idx, &reason, results, tx).await;
                    let all_results = std::mem::take(&mut *results.lock().await);
                    let ordered = order_results_by_tool_calls(tool_calls, &all_results);
                    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
                    return Err(ToolExecOutcome::Completed {
                        results: ordered,
                        tool_metrics: collected_timings,
                        transfer_signal: None,
                        injected_messages,
                    });
                }
                PreDispatchVerdict::Skip(error_text) => {
                    let error_result = AgentToolResult {
                        content: vec![ContentBlock::Text { text: error_text }],
                        details: serde_json::Value::Null,
                        is_error: true,
                        transfer_signal: None,
                    };
                    emit_error_result(&tc.name, &tc.id, error_result, idx, results, tx).await;
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

            let should_call_approval = match config.approval_mode {
                ApprovalMode::Smart => requires_approval,
                ApprovalMode::Enabled => true,
                ApprovalMode::Bypassed => unreachable!(),
            };

            if should_call_approval {
                match check_approval(
                    approve_fn,
                    tc,
                    &effective_arguments,
                    idx,
                    requires_approval,
                    tool_map,
                    results,
                    tx,
                )
                .await
                {
                    ApprovalOutcome::Approved => {}
                    ApprovalOutcome::ApprovedWith(new_params) => {
                        effective_arguments = new_params;
                    }
                    ApprovalOutcome::Rejected => continue,
                    ApprovalOutcome::ChannelClosed => return Err(ToolExecOutcome::ChannelClosed),
                }
            }
        }

        prepared.push(PreparedToolCall {
            idx,
            effective_arguments,
        });
    }

    Ok(PreprocessResult {
        prepared,
        injected_messages,
    })
}

// ─── Approval helper ────────────────────────────────────────────────────────

/// Run the approval gate for a single tool call.
#[allow(clippy::too_many_arguments)]
async fn check_approval(
    approve_fn: &ApproveToolFn,
    tc: &ToolCallInfo,
    effective_arguments: &serde_json::Value,
    idx: usize,
    requires_approval: bool,
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, crate::types::ToolResultMessage)>>>,
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
