use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::types::{
    AgentContext, AgentMessage, AssistantMessage, ContentBlock, LlmMessage, StopReason,
    ToolResultMessage, TurnSnapshot,
};
use crate::util::now_timestamp;

use super::{
    AgentEvent, AgentLoopConfig, LoopState, StreamResult, ToolCallInfo, ToolExecOutcome,
    TurnEndReason, TurnOutcome, CONTEXT_OVERFLOW_SENTINEL, build_abort_message, emit,
};
use super::stream::{capability_filter_tools, stream_with_retry};
use super::tool_dispatch::execute_tools_concurrently;

/// Run a single turn of the inner loop: inject pending messages, transform
/// context, stream the assistant response, handle tool calls, and emit events.
#[allow(clippy::too_many_lines)]
pub async fn run_single_turn(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    system_prompt: &str,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    debug!(
        context_messages = state.context_messages.len(),
        pending_messages = state.pending_messages.len(),
        "turn starting"
    );

    // i. Inject any pending messages into context
    if !state.pending_messages.is_empty() {
        state.context_messages.append(&mut state.pending_messages);
    }

    // Check cancellation
    if cancellation_token.is_cancelled() {
        return handle_cancellation(config, state, tx).await;
    }

    // Emit TurnStart
    if !emit(tx, AgentEvent::TurnStart).await {
        return TurnOutcome::Return;
    }

    // ii-a. Call async context transformer if set (runs before sync)
    if let Some(ref async_transformer) = config.async_transform_context
        && let Some(report) = async_transformer
            .transform(&mut state.context_messages, state.overflow_signal)
            .await
    {
        let _ = emit(tx, AgentEvent::ContextCompacted { report }).await;
    }

    // ii-b. Call sync context transformer if set
    if let Some(ref transformer) = config.transform_context
        && let Some(report) =
            transformer.transform(&mut state.context_messages, state.overflow_signal)
    {
        let _ = emit(tx, AgentEvent::ContextCompacted { report }).await;
    }
    // Reset overflow after it's been signaled
    state.overflow_signal = false;

    // iii. Apply convert_to_llm to filter messages for the provider
    let llm_messages: Vec<LlmMessage> = state
        .context_messages
        .iter()
        .filter_map(|m| (config.convert_to_llm)(m))
        .collect();

    // iv. Resolve a per-call API key if configured
    let api_key = if let Some(ref get_key) = config.get_api_key {
        get_key(&config.model.provider).await
    } else {
        None
    };

    // v-pre. Budget guard: check accumulated cost/tokens before calling the LLM
    if let Some(ref guard) = config.budget_guard
        && let Err(exceeded) = guard.check(&state.accumulated_usage, &state.accumulated_cost)
    {
        let error = crate::error::AgentError::BudgetExceeded(exceeded);
        let error_msg = super::build_error_message(&config.model, &error);
        return handle_error_stop(error_msg, state, tx).await;
    }

    // v. Build context and call StreamFn with retry logic.
    // Filter tools based on model capabilities (strip tools if the model
    // does not support tool use).
    let effective_tools = capability_filter_tools(&config.model, &config.tools);
    let agent_context = AgentContext {
        system_prompt: system_prompt.to_string(),
        messages: Vec::new(),
        tools: effective_tools,
    };

    // Emit BeforeLlmCall
    if !emit(
        tx,
        AgentEvent::BeforeLlmCall {
            system_prompt: system_prompt.to_string(),
            messages: llm_messages.clone(),
            model: config.model.clone(),
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }

    let turn_start = Instant::now();
    let llm_start = Instant::now();
    let stream_result = stream_with_retry(
        config,
        &agent_context,
        &llm_messages,
        system_prompt,
        api_key,
        cancellation_token,
        tx,
    )
    .await;
    let llm_call_duration = llm_start.elapsed();

    let Some(assistant_message) = handle_stream_result(stream_result, config, state, tx).await
    else {
        return TurnOutcome::Return;
    };

    // Check if ContextOverflow sentinel was returned
    if assistant_message.stop_reason == StopReason::Length
        && assistant_message.error_message.as_deref() == Some(CONTEXT_OVERFLOW_SENTINEL)
    {
        state.overflow_signal = true;
        return TurnOutcome::ContinueInner;
    }

    // vii. Check stop_reason for error/aborted
    if matches!(
        assistant_message.stop_reason,
        StopReason::Error | StopReason::Aborted
    ) {
        return handle_error_stop(assistant_message, state, tx).await;
    }

    // viii. Extract tool calls from assistant message content
    let tool_calls = extract_tool_calls(&assistant_message);

    // ix. If no tool calls: emit TurnEnd, exit inner loop
    if tool_calls.is_empty() {
        return handle_no_tool_calls(
            assistant_message,
            state,
            config,
            llm_call_duration,
            turn_start,
            tx,
        )
        .await;
    }

    // x–xiii. Process tool calls
    handle_tool_calls(
        config,
        state,
        assistant_message,
        tool_calls,
        llm_call_duration,
        turn_start,
        cancellation_token,
        tx,
    )
    .await
}

// ─── Snapshot builder ────────────────────────────────────────────────────

/// Build a `TurnSnapshot` from current loop state.
///
/// Extracts LLM messages from `context_messages`, using the accumulated
/// usage/cost and the given stop reason.
fn build_snapshot(state: &LoopState, stop_reason: StopReason) -> TurnSnapshot {
    let llm_messages: Vec<LlmMessage> = state
        .context_messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect();
    TurnSnapshot {
        turn_index: state.turn_index,
        messages: Arc::new(llm_messages),
        usage: state.accumulated_usage.clone(),
        cost: state.accumulated_cost.clone(),
        stop_reason,
    }
}

// ─── run_single_turn helpers ─────────────────────────────────────────────────

/// Emit cancellation events and return from the loop.
async fn handle_cancellation(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    let abort_msg = build_abort_message(&config.model);
    let abort_msg_clone = abort_msg.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(abort_msg)));
    if !emit(tx, AgentEvent::TurnStart).await {
        return TurnOutcome::Return;
    }
    if !emit(tx, AgentEvent::MessageStart).await {
        return TurnOutcome::Return;
    }
    if !emit(
        tx,
        AgentEvent::MessageEnd {
            message: abort_msg_clone.clone(),
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    let snapshot = build_snapshot(state, StopReason::Aborted);
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: abort_msg_clone,
            tool_results: vec![],
            reason: TurnEndReason::Cancelled,
            snapshot,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    let _ = emit(
        tx,
        AgentEvent::AgentEnd {
            messages: Arc::new(std::mem::take(&mut state.context_messages)),
        },
    )
    .await;
    TurnOutcome::Return
}

/// Process the `StreamResult`, returning the assistant message on success,
/// or `None` if the loop should return (overflow, abort, or channel closed).
async fn handle_stream_result(
    result: StreamResult,
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> Option<AssistantMessage> {
    match result {
        StreamResult::Message(msg) => Some(msg),
        StreamResult::ContextOverflow => {
            // Return a sentinel message that run_single_turn recognizes
            Some(AssistantMessage {
                content: vec![],
                provider: String::new(),
                model_id: String::new(),
                usage: crate::types::Usage::default(),
                cost: crate::types::Cost::default(),
                stop_reason: StopReason::Length,
                error_message: Some(CONTEXT_OVERFLOW_SENTINEL.to_string()),
                timestamp: 0,
            })
        }
        StreamResult::Aborted => {
            let abort_msg = build_abort_message(&config.model);
            let abort_msg_clone = abort_msg.clone();
            state
                .context_messages
                .push(AgentMessage::Llm(LlmMessage::Assistant(abort_msg)));
            let snapshot = build_snapshot(state, StopReason::Aborted);
            if !emit(
                tx,
                AgentEvent::TurnEnd {
                    assistant_message: abort_msg_clone,
                    tool_results: vec![],
                    reason: TurnEndReason::Aborted,
                    snapshot,
                },
            )
            .await
            {
                return None;
            }
            let _ = emit(
                tx,
                AgentEvent::AgentEnd {
                    messages: Arc::new(std::mem::take(&mut state.context_messages)),
                },
            )
            .await;
            None
        }
        StreamResult::ChannelClosed => None,
    }
}

/// Handle an error or aborted stop reason: emit `TurnEnd` + `AgentEnd` and return.
async fn handle_error_stop(
    assistant_message: AssistantMessage,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    error!(
        stop_reason = ?assistant_message.stop_reason,
        error = ?assistant_message.error_message,
        "agent loop stopping due to error/abort"
    );
    let msg_clone = assistant_message.clone();
    let stop = assistant_message.stop_reason;
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    let snapshot = build_snapshot(state, stop);
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_clone,
            tool_results: vec![],
            reason: TurnEndReason::Error,
            snapshot,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    // CRITICAL: On error/abort, exit immediately — no follow-up polling
    let _ = emit(
        tx,
        AgentEvent::AgentEnd {
            messages: Arc::new(std::mem::take(&mut state.context_messages)),
        },
    )
    .await;
    TurnOutcome::Return
}

/// Extract tool call info from the assistant message content blocks.
fn extract_tool_calls(message: &AssistantMessage) -> Vec<ToolCallInfo> {
    message
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::ToolCall {
                id,
                name,
                arguments,
                partial_json,
            } = b
            {
                Some(ToolCallInfo {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    is_incomplete: partial_json.is_some(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Handle the case where no tool calls are present: emit `TurnEnd`, break inner.
#[allow(clippy::too_many_arguments)]
async fn handle_no_tool_calls(
    assistant_message: AssistantMessage,
    state: &mut LoopState,
    config: &Arc<AgentLoopConfig>,
    llm_call_duration: Duration,
    turn_start: Instant,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    // Update accumulated usage/cost for policy tracking.
    state.accumulated_usage += assistant_message.usage.clone();
    state.accumulated_cost += assistant_message.cost.clone();
    state.last_assistant_message = Some(assistant_message.clone());

    // Clear last tool results (no tools this turn).
    state.last_tool_results = vec![];

    // Emit metrics if collector is configured.
    if let Some(ref collector) = config.metrics_collector {
        let metrics = crate::metrics::TurnMetrics {
            turn_index: state.turn_index,
            llm_call_duration,
            tool_executions: vec![],
            usage: assistant_message.usage.clone(),
            cost: assistant_message.cost.clone(),
            turn_duration: turn_start.elapsed(),
        };
        collector.on_metrics(&metrics).await;
    }

    // Clone twice: once for all_new_messages, once for TurnEnd event.
    // The original goes to context_messages.
    let msg_for_event = assistant_message.clone();
    let stop = assistant_message.stop_reason;
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    let snapshot = build_snapshot(state, stop);
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_for_event,
            tool_results: vec![],
            reason: TurnEndReason::Complete,
            snapshot,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    TurnOutcome::BreakInner
}

/// Handle tool calls: separate incomplete ones, execute the rest, collect results,
/// emit `TurnEnd`, and poll steering.
#[allow(clippy::too_many_arguments)]
async fn handle_tool_calls(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    assistant_message: AssistantMessage,
    mut tool_call_data: Vec<ToolCallInfo>,
    llm_call_duration: Duration,
    turn_start: Instant,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    // Update accumulated usage/cost for policy tracking.
    state.accumulated_usage += assistant_message.usage.clone();
    state.accumulated_cost += assistant_message.cost.clone();
    state.last_assistant_message = Some(assistant_message.clone());

    // Clone twice: once for all_new_messages, once for TurnEnd event.
    // The original goes to context_messages.
    let msg_for_turn_end = assistant_message.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));

    // Max tokens recovery: replace incomplete tool calls with error results
    let max_token_results =
        recover_incomplete_tool_calls(&mut tool_call_data, msg_for_turn_end.stop_reason);

    // Add max token error results to context
    for tr in &max_token_results {
        state
            .context_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
    }

    // xi. Execute tool calls concurrently
    let mut tool_results: Vec<ToolResultMessage> = max_token_results;
    let mut steering_interrupted = false;
    let mut collected_tool_metrics: Vec<crate::metrics::ToolExecMetrics> = Vec::new();

    if !tool_call_data.is_empty() {
        let exec_results =
            execute_tools_concurrently(config, &tool_call_data, cancellation_token, tx).await;

        match exec_results {
            ToolExecOutcome::Completed {
                results,
                tool_metrics,
            } => {
                tool_results.extend(results);
                collected_tool_metrics = tool_metrics;
            }
            ToolExecOutcome::SteeringInterrupt {
                completed,
                cancelled,
                steering_messages,
                tool_metrics,
            } => {
                tool_results.extend(completed);
                tool_results.extend(cancelled);
                steering_interrupted = true;
                collected_tool_metrics = tool_metrics;
                state.pending_messages.extend(steering_messages);
            }
            ToolExecOutcome::ChannelClosed => return TurnOutcome::Return,
        }
    }

    // Emit metrics if collector is configured.
    if let Some(ref collector) = config.metrics_collector {
        let metrics = crate::metrics::TurnMetrics {
            turn_index: state.turn_index,
            llm_call_duration,
            tool_executions: collected_tool_metrics,
            usage: msg_for_turn_end.usage.clone(),
            cost: msg_for_turn_end.cost.clone(),
            turn_duration: turn_start.elapsed(),
        };
        collector.on_metrics(&metrics).await;
    }

    // xii. Add tool result messages to context
    for tr in &tool_results {
        state
            .context_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
    }

    // Store tool results for post-turn hook access
    state.last_tool_results.clone_from(&tool_results);

    // xiii. Emit TurnEnd
    let snapshot = build_snapshot(state, msg_for_turn_end.stop_reason);
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_for_turn_end,
            tool_results,
            reason: if steering_interrupted {
                TurnEndReason::SteeringInterrupt
            } else {
                TurnEndReason::ToolsExecuted
            },
            snapshot,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }

    // Poll steering if not already interrupted
    if !steering_interrupted && let Some(ref provider) = config.message_provider {
        let msgs = provider.poll_steering();
        if !msgs.is_empty() {
            state.pending_messages.extend(msgs);
        }
    }
    // Inner loop continues — model must process tool results.
    TurnOutcome::ContinueInner
}

/// Replace incomplete tool calls (from max-tokens truncation) with error results.
/// Removes incomplete entries from `tool_call_data` and returns their error results.
fn recover_incomplete_tool_calls(
    tool_call_data: &mut Vec<ToolCallInfo>,
    stop_reason: StopReason,
) -> Vec<ToolResultMessage> {
    let mut max_token_results: Vec<ToolResultMessage> = Vec::new();
    if stop_reason == StopReason::Length {
        let mut remaining = Vec::new();
        for tc in tool_call_data.drain(..) {
            if tc.is_incomplete {
                max_token_results.push(ToolResultMessage {
                    tool_call_id: tc.id,
                    content: vec![ContentBlock::Text {
                        text: "error: tool call was incomplete due to max tokens reached"
                            .to_string(),
                    }],
                    is_error: true,
                    timestamp: now_timestamp(),
                    details: serde_json::Value::Null,
                });
            } else {
                remaining.push(tc);
            }
        }
        *tool_call_data = remaining;
    }
    max_token_results
}
