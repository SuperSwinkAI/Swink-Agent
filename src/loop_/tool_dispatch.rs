use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, info_span};

use crate::tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
    validate_tool_arguments, validation_error_result,
};
use crate::types::{ContentBlock, ToolResultMessage};
use crate::util::now_timestamp;

use super::{
    AgentEvent, AgentLoopConfig, ApproveToolFn, ToolCallInfo, ToolExecOutcome, emit,
};

/// Execute tool calls concurrently, checking for steering interrupts after each.
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
        "dispatching tool batch"
    );

    let batch_token = cancellation_token.child_token();
    let results: Arc<Mutex<Vec<(usize, ToolResultMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let steering_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Pre-build a tool lookup map for O(1) dispatch by name.
    let tool_map: HashMap<&str, &Arc<dyn AgentTool>> =
        config.tools.iter().map(|t| (t.name(), t)).collect();

    let mut handles = Vec::new();

    for (idx, tc) in tool_calls.iter().enumerate() {
        // Emit ToolExecutionStart
        if !emit(
            tx,
            AgentEvent::ToolExecutionStart {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        )
        .await
        {
            return ToolExecOutcome::ChannelClosed;
        }

        // ── Approval gate ──
        let mut arguments_override: Option<serde_json::Value> = None;
        if let Some(ref approve_fn) = config.approve_tool
            && config.approval_mode != ApprovalMode::Bypassed
        {
            let requires_approval = tool_map
                .get(tc.name.as_str())
                .is_some_and(|t| t.requires_approval());
            match check_approval(approve_fn, tc, idx, requires_approval, &results, tx).await {
                ApprovalOutcome::Approved => {} // proceed to dispatch
                ApprovalOutcome::ApprovedWith(new_params) => {
                    arguments_override = Some(new_params);
                }
                ApprovalOutcome::Rejected => continue,
                ApprovalOutcome::ChannelClosed => return ToolExecOutcome::ChannelClosed,
            }
        }

        // Resolve effective arguments (may be overridden by ApprovedWith)
        let mut effective_arguments = arguments_override.unwrap_or_else(|| tc.arguments.clone());

        // ── Tool call transformer ──
        if let Some(ref transformer) = config.tool_call_transformer {
            transformer.transform(&tc.name, &mut effective_arguments);
        }

        // ── Custom validator check ──
        if let Some(ref validator) = config.tool_validator
            && let Err(msg) = validator.validate(&tc.name, &effective_arguments)
        {
            let tool_result_msg = ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: vec![ContentBlock::Text { text: msg }],
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
            };
            let _ = emit(
                tx,
                AgentEvent::ToolExecutionEnd {
                    result: AgentToolResult {
                        content: tool_result_msg.content.clone(),
                        details: serde_json::Value::Null,
                        is_error: true,
                    },
                    is_error: true,
                },
            )
            .await;
            results.lock().await.push((idx, tool_result_msg));
            continue;
        }

        let handle = dispatch_single_tool(
            &tool_map,
            config,
            tc,
            &effective_arguments,
            idx,
            &batch_token,
            &results,
            &steering_detected,
            tx,
        )
        .await;

        match handle {
            DispatchResult::Spawned(h) => handles.push((idx, h)),
            DispatchResult::Inline => {}
        }
    }

    collect_tool_results(
        config,
        tool_calls,
        handles,
        results,
        steering_detected,
        batch_token,
    )
    .await
}

// ─── execute_tools_concurrently helpers ──────────────────────────────────────

/// Result of checking the approval gate for a single tool call.
enum ApprovalOutcome {
    Approved,
    /// Approved with modified parameters.
    ApprovedWith(serde_json::Value),
    Rejected,
    ChannelClosed,
}

/// Run the approval gate for a single tool call: emit events, call callback, handle rejection.
async fn check_approval(
    approve_fn: &ApproveToolFn,
    tc: &ToolCallInfo,
    idx: usize,
    requires_approval: bool,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) -> ApprovalOutcome {
    if !emit(
        tx,
        AgentEvent::ToolApprovalRequested {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        },
    )
    .await
    {
        return ApprovalOutcome::ChannelClosed;
    }

    let request = ToolApprovalRequest {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        arguments: tc.arguments.clone(),
        requires_approval,
    };
    let decision = approve_fn(request).await;
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
            if !emit(
                tx,
                AgentEvent::ToolExecutionEnd {
                    result: rejection_result.clone(),
                    is_error: true,
                },
            )
            .await
            {
                return ApprovalOutcome::ChannelClosed;
            }
            let tool_result_msg = ToolResultMessage {
                tool_call_id: tc.id.clone(),
                content: rejection_result.content,
                is_error: true,
                timestamp: now_timestamp(),
                details: serde_json::Value::Null,
            };
            results.lock().await.push((idx, tool_result_msg));
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
}

/// Validate and dispatch a single tool call, returning a join handle or inline result.
#[allow(clippy::too_many_arguments)]
async fn dispatch_single_tool(
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    config: &Arc<AgentLoopConfig>,
    tc: &ToolCallInfo,
    effective_arguments: &serde_json::Value,
    idx: usize,
    batch_token: &CancellationToken,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_flag: &Arc<std::sync::atomic::AtomicBool>,
    tx: &mpsc::Sender<AgentEvent>,
) -> DispatchResult {
    let tool = tool_map.get(tc.name.as_str()).copied();

    let tool_call_id = tc.id.clone();
    let tool_name = tc.name.clone();
    let arguments = effective_arguments.clone();

    if let Some(tool) = tool {
        let tool = Arc::clone(tool);
        let child_token = batch_token.child_token();
        let results_clone = Arc::clone(results);
        let steering_clone = Arc::clone(steering_flag);
        let config_clone = Arc::clone(config);
        let tx_clone = tx.clone();
        let on_update_tx = tx.clone();

        let validation = validate_tool_arguments(tool.parameters_schema(), &arguments);

        let tool_span = info_span!(
            "tool_execute",
            tool_name = %tool_name,
            tool_call_id = %tool_call_id,
        );
        let handle = tokio::spawn(
            async move {
                debug!(tool = %tool_name, id = %tool_call_id, "tool execution starting");
                let (result, is_error) = if let Err(errors) = validation {
                    (validation_error_result(&errors), true)
                } else {
                    let on_update = Box::new(move |partial: AgentToolResult| {
                        let _ = on_update_tx.try_send(AgentEvent::ToolExecutionUpdate { partial });
                    });
                    let result = tool
                        .execute(&tool_call_id, arguments, child_token, Some(on_update))
                        .await;
                    let is_error = result.is_error;
                    (result, is_error)
                };
                debug!(tool = %tool_name, id = %tool_call_id, is_error, "tool execution finished");

                let _ = emit(
                    &tx_clone,
                    AgentEvent::ToolExecutionEnd {
                        result: result.clone(),
                        is_error,
                    },
                )
                .await;

                let tool_result_msg = ToolResultMessage {
                    tool_call_id: tool_call_id.clone(),
                    content: result.content,
                    is_error,
                    timestamp: now_timestamp(),
                    details: result.details,
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
    } else {
        // Unknown tool
        let error_result = crate::tool::unknown_tool_result(&tool_name);
        let _ = emit(
            tx,
            AgentEvent::ToolExecutionEnd {
                result: error_result.clone(),
                is_error: true,
            },
        )
        .await;

        let tool_result_msg = ToolResultMessage {
            tool_call_id,
            content: error_result.content,
            is_error: true,
            timestamp: now_timestamp(),
            details: serde_json::Value::Null,
        };
        results.lock().await.push((idx, tool_result_msg));
        DispatchResult::Inline
    }
}

/// Wait for all spawned tool tasks, checking for steering interrupts.
async fn collect_tool_results(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    handles: Vec<(usize, tokio::task::JoinHandle<()>)>,
    results: Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_detected: Arc<std::sync::atomic::AtomicBool>,
    batch_token: CancellationToken,
) -> ToolExecOutcome {
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
            };
            results.lock().await.push((idx, panic_result));
            continue;
        }

        if steering_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            while futs.next().await.is_some() {}

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
                            text: "tool call cancelled: user requested steering interrupt"
                                .to_string(),
                        }],
                        is_error: true,
                        timestamp: now_timestamp(),
                        details: serde_json::Value::Null,
                    });
                }
            }

            let steering_messages = config
                .message_provider
                .as_ref()
                .map_or_else(Vec::new, |provider| provider.poll_steering());

            return ToolExecOutcome::SteeringInterrupt {
                completed,
                cancelled,
                steering_messages,
            };
        }
    }

    // All tools completed without steering
    let all_results = std::mem::take(&mut *results.lock().await);
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

    ToolExecOutcome::Completed(ordered)
}
