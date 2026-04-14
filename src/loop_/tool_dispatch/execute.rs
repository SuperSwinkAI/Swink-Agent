//! Execute phase: tool dispatch, credential resolution, execution grouping.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info_span};

use crate::tool::{AgentTool, AgentToolResult, validate_tool_arguments, validation_error_result};
use crate::tool_execution_policy::{ToolCallSummary, ToolExecutionPolicy};
use crate::types::{AgentMessage, ToolResultMessage};
use crate::util::now_timestamp;

use super::shared::{emit_error_result, emit_tool_execution_start, forward_tool_updates};
use super::{AgentEvent, AgentLoopConfig, PreparedToolCall, ToolCallInfo, emit};

// ─── Dispatch result ────────────────────────────────────────────────────────

/// Result of dispatching a single tool call.
pub(super) enum DispatchResult {
    /// Tool was spawned as a tokio task.
    Spawned(tokio::task::JoinHandle<()>),
    /// Tool result was added inline (unknown tool).
    Inline,
    /// Event channel closed before execution could start.
    ChannelClosed,
}

// ─── Execution group computation ────────────────────────────────────────────

/// Compute execution groups from the policy. Returns groups of indices into the
/// `prepared` slice. Tools within a group execute concurrently; groups execute
/// sequentially.
pub(super) async fn compute_execution_groups(
    policy: &ToolExecutionPolicy,
    tool_calls: &[ToolCallInfo],
    prepared: &[PreparedToolCall],
) -> Vec<Vec<usize>> {
    if prepared.is_empty() {
        return vec![];
    }

    match policy {
        ToolExecutionPolicy::Concurrent => {
            vec![(0..prepared.len()).collect()]
        }
        ToolExecutionPolicy::Sequential => (0..prepared.len()).map(|i| vec![i]).collect(),
        ToolExecutionPolicy::Priority(priority_fn) => {
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

            scored.sort_by(|a, b| b.1.cmp(&a.1));

            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut current_priority = None;

            for (prep_idx, priority) in scored {
                if current_priority == Some(priority) {
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

// ─── Single tool dispatch ───────────────────────────────────────────────────

/// Validate and dispatch a single tool call, returning a join handle or inline result.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn dispatch_single_tool(
    tool_map: &HashMap<&str, &Arc<dyn AgentTool>>,
    config: &Arc<AgentLoopConfig>,
    tc: &ToolCallInfo,
    effective_arguments: &serde_json::Value,
    idx: usize,
    batch_token: &CancellationToken,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    tool_timings: &Arc<tokio::sync::Mutex<Vec<crate::metrics::ToolExecMetrics>>>,
    _steering_messages: &Arc<tokio::sync::Mutex<Vec<AgentMessage>>>,
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

            if let Some(ref provider) = config_clone.message_provider
                && provider.has_steering()
            {
                steering_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        .instrument(tool_span),
    );

    DispatchResult::Spawned(handle)
}

// ─── Credential resolution ──────────────────────────────────────────────────

/// Resolve credentials for a tool, if it declares an `auth_config()`.
///
/// Returns `Ok(None)` for unauthenticated tools (zero overhead path).
async fn resolve_credential(
    tool: &Arc<dyn AgentTool>,
    config: &Arc<AgentLoopConfig>,
    _tool_call_id: &str,
) -> Result<Option<crate::credential::ResolvedCredential>, crate::credential::CredentialError> {
    let Some(auth_config) = tool.auth_config() else {
        return Ok(None);
    };

    let cred_resolver = config.credential_resolver.as_ref().ok_or_else(|| {
        crate::credential::CredentialError::NotFound {
            key: auth_config.credential_key.clone(),
        }
    })?;

    let resolve_future = cred_resolver.resolve(&auth_config.credential_key);
    let credential = tokio::time::timeout(std::time::Duration::from_secs(30), resolve_future)
        .await
        .map_err(|_| crate::credential::CredentialError::Timeout {
            key: auth_config.credential_key.clone(),
        })??;

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
