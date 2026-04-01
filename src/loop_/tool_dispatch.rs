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
use crate::tool_execution_policy::{ToolCallSummary, ToolExecutionPolicy};
use crate::types::{ContentBlock, ToolResultMessage};
use crate::util::now_timestamp;

use super::{AgentEvent, AgentLoopConfig, ApproveToolFn, ToolCallInfo, ToolExecOutcome, emit};

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
    tool_call_id: &str,
    error_result: AgentToolResult,
    idx: usize,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tx: &mpsc::Sender<AgentEvent>,
) {
    let _ = emit(
        tx,
        AgentEvent::ToolExecutionEnd {
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

    // Pre-build a tool lookup map for O(1) dispatch by name.
    let tool_map: HashMap<&str, &Arc<dyn AgentTool>> =
        config.tools.iter().map(|t| (t.name(), t)).collect();

    // Phase 1: Pre-process all tool calls (approval, transform, validate).
    let mut prepared: Vec<PreparedToolCall> = Vec::new();

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

        // ── PreDispatch policies ──
        let mut effective_arguments = tc.arguments.clone();
        {
            use crate::policy::{
                PolicyContext, PreDispatchVerdict, ToolPolicyContext, run_pre_dispatch_policies,
            };

            let state_snapshot = {
                let guard = config.session_state.read().unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.clone()
            };
            let policy_ctx = PolicyContext {
                turn_index: 0, // tool dispatch doesn't track turn index
                accumulated_usage: &crate::types::Usage::default(),
                accumulated_cost: &crate::types::Cost::default(),
                message_count: 0,
                overflow_signal: false,
                new_messages: &[],
                state: &state_snapshot,
            };
            let mut tool_ctx = ToolPolicyContext {
                tool_name: &tc.name,
                tool_call_id: &tc.id,
                arguments: &mut effective_arguments,
            };
            match run_pre_dispatch_policies(
                &config.pre_dispatch_policies,
                &policy_ctx,
                &mut tool_ctx,
            ) {
                PreDispatchVerdict::Continue | PreDispatchVerdict::Inject(_) => {}
                PreDispatchVerdict::Stop(reason) => {
                    let error_result = AgentToolResult {
                        content: vec![ContentBlock::Text {
                            text: format!("policy stopped tool batch: {reason}"),
                        }],
                        details: serde_json::Value::Null,
                        is_error: true,
                    };
                    emit_error_result(&tc.id, error_result, idx, &results, tx).await;
                    // Return all results collected so far
                    let all_results = std::mem::take(&mut *results.lock().await);
                    let ordered = order_results_by_tool_calls(tool_calls, &all_results);
                    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
                    return ToolExecOutcome::Completed {
                        results: ordered,
                        tool_metrics: collected_timings,
                    };
                }
                PreDispatchVerdict::Skip(error_text) => {
                    let error_result = AgentToolResult {
                        content: vec![ContentBlock::Text { text: error_text }],
                        details: serde_json::Value::Null,
                        is_error: true,
                    };
                    emit_error_result(&tc.id, error_result, idx, &results, tx).await;
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
            match check_approval(approve_fn, tc, idx, requires_approval, &tool_map, &results, tx).await {
                ApprovalOutcome::Approved => {}
                ApprovalOutcome::ApprovedWith(new_params) => {
                    effective_arguments = new_params;
                }
                ApprovalOutcome::Rejected => continue,
                ApprovalOutcome::ChannelClosed => return ToolExecOutcome::ChannelClosed,
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
                tx,
            )
            .await;

            match handle {
                DispatchResult::Spawned(h) => handles.push((prep.idx, h)),
                DispatchResult::Inline => {}
            }
        }

        // Collect results for this group before proceeding to the next.
        let group_outcome = collect_group_results(
            tool_calls,
            handles,
            &results,
            &steering_detected,
            &batch_token,
        )
        .await;

        match group_outcome {
            GroupOutcome::Continue => {}
            GroupOutcome::SteeringInterrupt => {
                return build_steering_outcome(config, tool_calls, results, tool_timings).await;
            }
        }
    }

    // All groups completed without steering interrupts.
    let all_results = std::mem::take(&mut *results.lock().await);
    let ordered = order_results_by_tool_calls(tool_calls, &all_results);

    let collected_timings = std::mem::take(&mut *tool_timings.lock().await);
    ToolExecOutcome::Completed {
        results: ordered,
        tool_metrics: collected_timings,
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
}

/// Collect results for a single execution group's spawned handles.
async fn collect_group_results(
    tool_calls: &[ToolCallInfo],
    handles: Vec<(usize, tokio::task::JoinHandle<()>)>,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_detected: &Arc<std::sync::atomic::AtomicBool>,
    batch_token: &CancellationToken,
) -> GroupOutcome {
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
            // Drain remaining futures in this group.
            while futs.next().await.is_some() {}
            return GroupOutcome::SteeringInterrupt;
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
async fn check_approval(
    approve_fn: &ApproveToolFn,
    tc: &ToolCallInfo,
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
            arguments: tc.arguments.clone(),
        },
    )
    .await
    {
        return ApprovalOutcome::ChannelClosed;
    }

    // Resolve approval context with panic safety.
    let approval_context = tool_map
        .get(tc.name.as_str())
        .and_then(|tool| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tool.approval_context(&tc.arguments)
            }))
            .unwrap_or_else(|_| {
                tracing::warn!(tool_name = %tc.name, "approval_context() panicked — using None");
                None
            })
        });

    let request = ToolApprovalRequest {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        arguments: tc.arguments.clone(),
        requires_approval,
        context: approval_context,
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
            emit_error_result(&tc.id, rejection_result, idx, results, tx).await;
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
    tool_timings: &Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    steering_flag: &Arc<std::sync::atomic::AtomicBool>,
    tx: &mpsc::Sender<AgentEvent>,
) -> DispatchResult {
    let tool = tool_map.get(tc.name.as_str()).copied();

    let tool_call_id = tc.id.clone();
    let tool_name = tc.name.clone();
    let arguments = effective_arguments.clone();

    let Some(tool) = tool else {
        // Unknown tool
        let error_result = crate::tool::unknown_tool_result(&tool_name);
        emit_error_result(&tool_call_id, error_result, idx, results, tx).await;
        return DispatchResult::Inline;
    };

    let tool = Arc::clone(tool);
    let child_token = batch_token.child_token();
    let results_clone = Arc::clone(results);
    let timings_clone = Arc::clone(tool_timings);
    let steering_clone = Arc::clone(steering_flag);
    let config_clone = Arc::clone(config);
    let tx_clone = tx.clone();

    let validation = validate_tool_arguments(tool.parameters_schema(), &arguments);

    let tool_span = info_span!(
        "tool_execute",
        tool_name = %tool_name,
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
                    Err(cred_error) => {
                        (AgentToolResult::error(format!("{cred_error}")), true)
                    }
                    Ok(credential) => {
                        let on_update_tx = tx_clone.clone();
                        let on_update = Box::new(move |partial: AgentToolResult| {
                            let _ = on_update_tx.try_send(AgentEvent::ToolExecutionUpdate { partial });
                        });
                        let result = tool
                            .execute(&tool_call_id, arguments, child_token, Some(on_update), config_clone.session_state.clone(), credential)
                            .await;
                        let is_error = result.is_error;
                        (result, is_error)
                    }
                }
            };
            let exec_duration = exec_start.elapsed();
            debug!(tool = %tool_name, id = %tool_call_id, is_error, "tool execution finished");

            timings_clone
                .lock()
                .await
                .push(crate::metrics::ToolExecMetrics {
                    tool_name,
                    duration: exec_duration,
                    success: !is_error,
                });

            let _ = emit(
                &tx_clone,
                AgentEvent::ToolExecutionEnd {
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
        crate::credential::ResolvedCredential::ApiKey(_) => crate::credential::CredentialType::ApiKey,
        crate::credential::ResolvedCredential::Bearer(_) => crate::credential::CredentialType::Bearer,
        crate::credential::ResolvedCredential::OAuth2AccessToken(_) => crate::credential::CredentialType::OAuth2,
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
