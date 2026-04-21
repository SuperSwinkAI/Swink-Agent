use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info_span, warn};

use crate::policy::{PolicyContext, PolicyVerdict, TurnPolicyContext, run_post_turn_policies};
use crate::types::{
    AgentContext, AgentMessage, AssistantMessage, ContentBlock, LlmMessage, StopReason,
    ToolResultMessage, TurnSnapshot,
};
use crate::util::now_timestamp;

use super::overflow::{OverflowRecoveryResult, attempt_overflow_recovery};
use super::stream::{capability_filter_tools, stream_with_retry};
use super::tool_dispatch::execute_tools_concurrently;
use super::{
    AgentEvent, AgentLoopConfig, LoopState, StreamResult, ToolCallInfo, ToolExecOutcome,
    TurnEndReason, TurnOutcome, build_abort_message, emit,
};

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

    // Reset per-turn overflow recovery flag so each turn gets an independent
    // recovery opportunity.
    state.overflow_recovery_attempted = false;

    // i. Inject any pending messages into context.
    // Track where new messages start so PreTurn policies only see the fresh batch.
    let new_messages_start = if state.turn_index == 0 {
        state
            .context_messages
            .len()
            .saturating_sub(state.initial_new_messages_len)
    } else {
        state.context_messages.len()
    };
    if !state.pending_messages.is_empty() {
        state.context_messages.append(&mut state.pending_messages);
    }
    state.initial_new_messages_len = 0;
    clear_pending_message_snapshot(config);
    // Sync the full context (including newly consumed pending messages) to the
    // loop_context_snapshot so that a concurrent pause() call can reconstruct
    // the complete message history even before this turn's TurnEnd event is
    // processed by the agent side.
    sync_loop_context_snapshot(config, state);

    // Check cancellation
    if cancellation_token.is_cancelled() {
        return handle_cancellation(config, state, tx).await;
    }

    // Pre-turn policies: check budget, turn caps, etc. before emitting TurnStart.
    // A Stop verdict here breaks the inner loop without emitting TurnStart/TurnEnd.
    {
        use crate::policy::{PolicyContext, PolicyVerdict, run_policies};
        use tracing::info;

        let state_snapshot = {
            let guard = config
                .session_state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };
        let policy_ctx = PolicyContext {
            turn_index: state.turn_index,
            accumulated_usage: &state.accumulated_usage,
            accumulated_cost: &state.accumulated_cost,
            message_count: state.context_messages.len(),
            overflow_signal: state.overflow_signal,
            new_messages: &state.context_messages[new_messages_start..],
            state: &state_snapshot,
        };
        match run_policies(&config.pre_turn_policies, &policy_ctx) {
            PolicyVerdict::Continue => {}
            PolicyVerdict::Stop(reason) => {
                info!("pre-turn policy stopped agent: {reason}");
                // Emit AgentEnd directly — no TurnStart was emitted yet.
                let _ = super::emit(
                    tx,
                    super::AgentEvent::AgentEnd {
                        messages: Arc::new(std::mem::take(&mut state.context_messages)),
                    },
                )
                .await;
                return TurnOutcome::Return;
            }
            PolicyVerdict::Inject(msgs) => {
                state.pending_messages.extend(msgs);
                sync_pending_message_snapshot(config, state);
            }
        }
    }

    // Emit TurnStart
    if !emit(tx, AgentEvent::TurnStart).await {
        return TurnOutcome::Return;
    }

    let turn_span = info_span!(
        "agent.turn",
        agent.turn_index = state.turn_index,
        agent.stop_reason = tracing::field::Empty,
    );
    let _turn_guard = turn_span.enter();

    // ii. Run context transformers (async first, then sync)
    run_context_transformers(
        config,
        &mut state.context_messages,
        state.overflow_signal,
        tx,
    )
    .await;
    state.overflow_signal = false;

    // ii-c. Annotate context messages with cache hints if caching is configured
    if let Some(ref cache_config) = config.cache_config {
        // Scope the MutexGuard so it's dropped before any await.
        let cache_event = {
            let mut cache_state = config
                .cache_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let hint = cache_state.advance_turn(cache_config);
            let prefix_len = cache_state.cached_prefix_len;

            // Estimate prefix tokens and check min_tokens threshold
            let prefix_tokens: usize = state
                .context_messages
                .iter()
                .take(prefix_len)
                .map(crate::context::estimate_tokens)
                .sum();

            if prefix_tokens >= cache_config.min_tokens {
                // Annotate cacheable prefix messages with the hint
                for msg in state.context_messages.iter_mut().take(prefix_len) {
                    msg.set_cache_hint(hint.clone());
                }
                // Clear hints on messages beyond the prefix
                for msg in state.context_messages.iter_mut().skip(prefix_len) {
                    msg.clear_cache_hint();
                }
                cache_state.cached_prefix_len = prefix_len;
                drop(cache_state);
                Some((hint, prefix_tokens))
            } else {
                drop(cache_state);
                None
            }
        };

        // Emit CacheAction event (after guard is dropped)
        if let Some((hint, prefix_tokens)) = cache_event {
            let _ = emit(
                tx,
                AgentEvent::CacheAction {
                    hint,
                    prefix_tokens,
                },
            )
            .await;
        }
    }

    // ii-d. Inject dynamic system prompt as a user-role message (non-cacheable)
    let dynamic_prompt_injected = build_dynamic_system_prompt_message(config);

    // iii. Apply convert_to_llm to filter messages for the provider
    let llm_messages = build_llm_messages(
        config,
        &state.context_messages,
        dynamic_prompt_injected.as_ref(),
    );

    // iv. Resolve a per-call API key if configured
    let api_key = if let Some(ref get_key) = config.get_api_key {
        get_key(&config.model.provider).await
    } else {
        None
    };

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
        api_key.clone(),
        true,
        cancellation_token,
        tx,
    )
    .await;
    let llm_call_duration = llm_start.elapsed();

    // ─── Emergency in-place overflow recovery (T069/T070) ───────────────
    let stream_result = if matches!(stream_result, StreamResult::ContextOverflow) {
        match attempt_overflow_recovery(
            config,
            state,
            system_prompt,
            &agent_context,
            dynamic_prompt_injected.as_ref(),
            api_key,
            cancellation_token,
            tx,
        )
        .await
        {
            OverflowRecoveryResult::Recovered(result) => *result,
            OverflowRecoveryResult::Failed(outcome) => return outcome,
        }
    } else {
        stream_result
    };

    let Some(mut assistant_message) = handle_stream_result(stream_result, config, state, tx).await
    else {
        return TurnOutcome::Return;
    };

    // Record OTel-compatible attributes on the turn span.
    turn_span.record(
        "agent.stop_reason",
        tracing::field::debug(&assistant_message.stop_reason),
    );

    // vii. Check stop_reason for error/aborted
    if matches!(
        assistant_message.stop_reason,
        StopReason::Error | StopReason::Aborted
    ) {
        return handle_error_stop(assistant_message, state, tx).await;
    }

    // viii. Extract tool calls from assistant message content. `extract_tool_calls`
    // derives `is_incomplete` from `partial_json.is_some()`, so it must run BEFORE
    // `sanitize_incomplete_tool_calls` clears that field.
    let tool_calls = extract_tool_calls(&assistant_message);

    // Issue #619: coerce any `ToolCall` blocks with `partial_json.is_some()` or
    // non-object `arguments` into a valid empty-object call before the assistant
    // message reaches any adapter again. Pairs with `recover_incomplete_tool_calls`
    // which inserts a matching synthetic error `ToolResult` for each incomplete
    // call so the provider sees a well-formed tool_use / tool_result pair.
    let fixed = crate::stream::sanitize_incomplete_tool_calls(&mut assistant_message);
    if fixed > 0 {
        debug!(
            fixed,
            "sanitized incomplete tool-use blocks before adapter dispatch"
        );
    }

    // ix. If no tool calls: emit TurnEnd, exit inner loop
    if tool_calls.is_empty() {
        return handle_no_tool_calls(
            assistant_message,
            state,
            config,
            system_prompt,
            llm_call_duration,
            turn_start,
            tx,
        )
        .await;
    }

    // x-xiii. Process tool calls
    handle_tool_calls(
        config,
        state,
        assistant_message,
        tool_calls,
        system_prompt,
        llm_call_duration,
        turn_start,
        cancellation_token,
        tx,
    )
    .await
}

pub(super) fn build_dynamic_system_prompt_message(
    config: &Arc<AgentLoopConfig>,
) -> Option<LlmMessage> {
    config
        .dynamic_system_prompt
        .as_ref()
        .and_then(|dynamic_fn| {
            let dynamic_text = dynamic_fn();
            if dynamic_text.is_empty() {
                None
            } else {
                Some(LlmMessage::User(crate::types::UserMessage {
                    content: vec![ContentBlock::Text { text: dynamic_text }],
                    timestamp: now_timestamp(),
                    cache_hint: None,
                }))
            }
        })
}

pub(super) fn build_llm_messages(
    config: &AgentLoopConfig,
    context_messages: &[AgentMessage],
    dynamic_prompt_injected: Option<&LlmMessage>,
) -> Vec<LlmMessage> {
    let mut llm_messages: Vec<LlmMessage> = context_messages
        .iter()
        .filter_map(|message| (config.convert_to_llm)(message))
        .collect();

    if let Some(dynamic_prompt) = dynamic_prompt_injected {
        llm_messages.insert(0, dynamic_prompt.clone());
    }

    llm_messages
}

// ─── Context transformer runner ─────────────────────────────────────────

/// Run async and sync context transformers in sequence, emitting
/// `ContextCompacted` events for each. Returns whether any compaction occurred.
pub(super) async fn run_context_transformers(
    config: &AgentLoopConfig,
    messages: &mut Vec<crate::types::AgentMessage>,
    overflow: bool,
    tx: &mpsc::Sender<AgentEvent>,
) -> bool {
    let mut any_compacted = false;

    // Async transformer runs first
    if let Some(ref async_transformer) = config.async_transform_context
        && let Some(report) = async_transformer.transform(messages, overflow).await
    {
        any_compacted = true;
        let _ = emit(tx, AgentEvent::ContextCompacted { report }).await;
    }

    // Sync transformer runs second
    if let Some(ref transformer) = config.transform_context
        && let Some(report) = transformer.transform(messages, overflow)
    {
        any_compacted = true;
        let _ = emit(tx, AgentEvent::ContextCompacted { report }).await;
    }

    any_compacted
}

fn clear_pending_message_snapshot(config: &AgentLoopConfig) {
    config.pending_message_snapshot.clear();
}

fn sync_pending_message_snapshot(config: &AgentLoopConfig, state: &LoopState) {
    config
        .pending_message_snapshot
        .replace(&state.pending_messages);
}

fn mark_assistant_message_aborted(message: &AssistantMessage) -> AssistantMessage {
    let mut aborted = message.clone();
    aborted.stop_reason = StopReason::Aborted;
    aborted.error_message = Some("operation aborted via cancellation token".to_string());
    aborted.error_kind = None;
    aborted.timestamp = now_timestamp();
    aborted
}

/// Sync the full loop context to the shared `loop_context_snapshot` so that
/// `Agent::pause()` can recover messages already drained from the shared pending
/// queue but not yet reflected in `in_flight_messages`.
fn sync_loop_context_snapshot(config: &AgentLoopConfig, state: &LoopState) {
    config
        .loop_context_snapshot
        .replace(&state.context_messages);
}

// ─── Shared helpers ──────────────────────────────────────────────────────

/// Update accumulated usage/cost and track the last assistant message.
fn accumulate_turn_state(state: &mut LoopState, message: &AssistantMessage) {
    state.accumulated_usage += message.usage.clone();
    state.accumulated_cost += message.cost.clone();
    state.last_assistant_message = Some(message.clone());
}

/// Emit turn metrics if a collector is configured.
async fn emit_turn_metrics(
    config: &Arc<AgentLoopConfig>,
    state: &LoopState,
    message: &AssistantMessage,
    llm_call_duration: Duration,
    tool_executions: Vec<crate::metrics::ToolExecMetrics>,
    turn_start: Instant,
) {
    if let Some(ref collector) = config.metrics_collector {
        let metrics = crate::metrics::TurnMetrics {
            turn_index: state.turn_index,
            llm_call_duration,
            tool_executions,
            usage: message.usage.clone(),
            cost: message.cost.clone(),
            turn_duration: turn_start.elapsed(),
        };
        collector.on_metrics(&metrics).await;
    }
}

/// Emit `TurnEnd` followed by `AgentEnd`, returning `TurnOutcome::Return`.
///
/// Consolidates the repeated pattern of emitting these two terminal events
/// and draining `context_messages` into the `AgentEnd` payload.
pub(super) async fn emit_turn_end_and_agent_end(
    assistant_message: AssistantMessage,
    tool_results: Vec<ToolResultMessage>,
    reason: TurnEndReason,
    snapshot: TurnSnapshot,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message,
            tool_results,
            reason,
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

/// Emit only `AgentEnd`, returning `TurnOutcome::Return`.
async fn emit_agent_end(state: &mut LoopState, tx: &mpsc::Sender<AgentEvent>) -> TurnOutcome {
    let _ = emit(
        tx,
        AgentEvent::AgentEnd {
            messages: Arc::new(std::mem::take(&mut state.context_messages)),
        },
    )
    .await;
    TurnOutcome::Return
}

// ─── Snapshot builder ────────────────────────────────────────────────────

/// Build a `TurnSnapshot` from current loop state.
///
/// Extracts LLM messages from `context_messages`, using the accumulated
/// usage/cost and the given stop reason.
pub(super) fn build_snapshot(
    state: &LoopState,
    stop_reason: StopReason,
    state_delta: Option<crate::StateDelta>,
) -> TurnSnapshot {
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
        state_delta,
    }
}

/// Flush the session state delta and emit a `StateChanged` event if non-empty.
async fn flush_state_delta(
    config: &AgentLoopConfig,
    tx: &mpsc::Sender<AgentEvent>,
) -> Option<crate::StateDelta> {
    let delta = {
        let mut s = config
            .session_state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.flush_delta()
    };
    if delta.is_empty() {
        None
    } else {
        let _ = emit(
            tx,
            AgentEvent::StateChanged {
                delta: delta.clone(),
            },
        )
        .await;
        Some(delta)
    }
}

// ─── run_single_turn helpers ─────────────────────────────────────────────────

async fn emit_cancellation_for_turn(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
    emit_turn_start: bool,
    emit_message_start: bool,
) -> TurnOutcome {
    let abort_msg = build_abort_message(&config.model);
    let msg_for_event = abort_msg.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(abort_msg)));
    if emit_turn_start && !emit(tx, AgentEvent::TurnStart).await {
        return TurnOutcome::Return;
    }
    if emit_message_start && !emit(tx, AgentEvent::MessageStart).await {
        return TurnOutcome::Return;
    }
    if !emit(
        tx,
        AgentEvent::MessageEnd {
            message: msg_for_event.clone(),
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    let snapshot = build_snapshot(state, StopReason::Aborted, None);
    emit_turn_end_and_agent_end(
        msg_for_event,
        vec![],
        TurnEndReason::Cancelled,
        snapshot,
        state,
        tx,
    )
    .await
}

/// Emit cancellation events and return before the turn has started.
pub(super) async fn handle_cancellation(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    emit_cancellation_for_turn(config, state, tx, true, true).await
}

/// Emit cancellation events for a turn that already emitted `TurnStart`.
pub(super) async fn handle_started_turn_cancellation(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    emit_cancellation_for_turn(config, state, tx, false, false).await
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
            // Context overflow is now handled in-place by attempt_overflow_recovery
            // before reaching this function. If we get here, it means recovery
            // was not attempted (should not happen in normal flow).
            debug!("unexpected ContextOverflow in handle_stream_result");
            None
        }
        StreamResult::Aborted => {
            let abort_msg = build_abort_message(&config.model);
            let msg_for_event = abort_msg.clone();
            state
                .context_messages
                .push(AgentMessage::Llm(LlmMessage::Assistant(abort_msg)));
            let snapshot = build_snapshot(state, StopReason::Aborted, None);
            emit_turn_end_and_agent_end(
                msg_for_event,
                vec![],
                TurnEndReason::Aborted,
                snapshot,
                state,
                tx,
            )
            .await;
            None
        }
        StreamResult::ChannelClosed => None,
    }
}

/// Handle an error or aborted stop reason: emit `TurnEnd` + `AgentEnd` and return.
async fn handle_error_stop(
    mut assistant_message: AssistantMessage,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    // Issue #619: scrub any incomplete tool-use blocks before we persist the
    // message into `context_messages` — even on terminal error paths a resumed
    // session (e.g. continuation) could replay this history to an adapter.
    crate::stream::sanitize_incomplete_tool_calls(&mut assistant_message);

    let is_abort = assistant_message.stop_reason == StopReason::Aborted;
    if is_abort {
        warn!(
            error = ?assistant_message.error_message,
            "agent loop stopping due to abort"
        );
    } else {
        error!(
            stop_reason = ?assistant_message.stop_reason,
            error = ?assistant_message.error_message,
            "agent loop stopping due to error"
        );
    }
    let msg_for_event = assistant_message.clone();
    let stop = assistant_message.stop_reason;
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    let snapshot = build_snapshot(state, stop, None);
    let reason = if is_abort {
        TurnEndReason::Aborted
    } else {
        TurnEndReason::Error
    };
    // CRITICAL: On error/abort, exit immediately — no follow-up polling
    emit_turn_end_and_agent_end(msg_for_event, vec![], reason, snapshot, state, tx).await
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

/// Run post-turn policies and return the (possibly replaced) assistant message.
///
/// If a policy returns `Inject` with an `AssistantMessage`, it replaces the
/// original — the replacement is what gets committed to context and emitted in
/// `TurnEnd`. Non-assistant injected messages go to `pending_messages`.
///
/// Returns `(final_assistant_message, Option<stop_reason>)`.
fn run_post_turn_policy_check(
    assistant_message: &AssistantMessage,
    tool_results: &[ToolResultMessage],
    state: &mut LoopState,
    config: &Arc<AgentLoopConfig>,
    system_prompt: &str,
) -> (AssistantMessage, Option<String>) {
    if config.post_turn_policies.is_empty() {
        return (assistant_message.clone(), None);
    }

    let state_snapshot = {
        let guard = config
            .session_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    };
    let policy_ctx = PolicyContext {
        turn_index: state.turn_index,
        accumulated_usage: &state.accumulated_usage,
        accumulated_cost: &state.accumulated_cost,
        message_count: state.context_messages.len(),
        overflow_signal: state.overflow_signal,
        new_messages: &[], // current-turn data is in TurnPolicyContext
        state: &state_snapshot,
    };
    let turn_ctx = TurnPolicyContext {
        assistant_message,
        tool_results,
        stop_reason: assistant_message.stop_reason,
        system_prompt,
        model_spec: &config.model,
        context_messages: &state.context_messages,
    };
    match run_post_turn_policies(&config.post_turn_policies, &policy_ctx, &turn_ctx) {
        PolicyVerdict::Continue => (assistant_message.clone(), None),
        PolicyVerdict::Stop(reason) => (assistant_message.clone(), Some(reason)),
        PolicyVerdict::Inject(msgs) => {
            // If the injection includes an assistant message, use the last one
            // as a replacement. All other injected messages go to pending.
            let mut replaced = assistant_message.clone();
            for msg in msgs {
                match msg {
                    AgentMessage::Llm(LlmMessage::Assistant(new_msg)) => {
                        if assistant_replacement_preserves_tool_calls(assistant_message, &new_msg) {
                            replaced = new_msg;
                        } else {
                            tracing::warn!(
                                "ignoring post-turn assistant replacement that would change tool calls"
                            );
                        }
                    }
                    other => {
                        state.pending_messages.push(other);
                        sync_pending_message_snapshot(config, state);
                    }
                }
            }
            // Update last_assistant_message to reflect the replacement.
            state.last_assistant_message = Some(replaced.clone());
            (replaced, None)
        }
    }
}

fn assistant_replacement_preserves_tool_calls(
    original: &AssistantMessage,
    replacement: &AssistantMessage,
) -> bool {
    let original_tool_calls: Vec<ContentBlock> = original
        .content
        .iter()
        .filter(|block| matches!(block, ContentBlock::ToolCall { .. }))
        .cloned()
        .collect();

    let replacement_tool_calls: Vec<ContentBlock> = replacement
        .content
        .iter()
        .filter(|block| matches!(block, ContentBlock::ToolCall { .. }))
        .cloned()
        .collect();

    original_tool_calls == replacement_tool_calls
}

/// Handle the case where no tool calls are present: commit the assistant,
/// run post-turn policies against the committed snapshot, emit `TurnEnd`,
/// and break inner.
#[allow(clippy::too_many_arguments)]
async fn handle_no_tool_calls(
    assistant_message: AssistantMessage,
    state: &mut LoopState,
    config: &Arc<AgentLoopConfig>,
    system_prompt: &str,
    llm_call_duration: Duration,
    turn_start: Instant,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    accumulate_turn_state(state, &assistant_message);
    state.last_tool_results = vec![];

    emit_turn_metrics(
        config,
        state,
        &assistant_message,
        llm_call_duration,
        vec![],
        turn_start,
    )
    .await;

    let assistant_ctx_index = state.context_messages.len();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(
            assistant_message.clone(),
        )));

    // Run post-turn policies against the committed turn snapshot so text-only,
    // tool, and transfer turns expose the same history shape.
    let (assistant_message, policy_stop) =
        run_post_turn_policy_check(&assistant_message, &[], state, config, system_prompt);

    state.context_messages[assistant_ctx_index] =
        AgentMessage::Llm(LlmMessage::Assistant(assistant_message.clone()));

    let stop = assistant_message.stop_reason;
    let state_delta = flush_state_delta(config, tx).await;
    let snapshot = build_snapshot(state, stop, state_delta);
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message,
            tool_results: vec![],
            reason: TurnEndReason::Complete,
            snapshot,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }

    if let Some(reason) = policy_stop {
        tracing::info!("post-turn policy stopped agent: {reason}");
        return emit_agent_end(state, tx).await;
    }

    if state.pending_messages.is_empty() {
        TurnOutcome::BreakInner
    } else {
        TurnOutcome::ContinueInner
    }
}

/// Handle tool calls: separate incomplete ones, execute the rest, collect results,
/// run post-turn policies, emit `TurnEnd`, and poll steering.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_tool_calls(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    assistant_message: AssistantMessage,
    mut tool_call_data: Vec<ToolCallInfo>,
    system_prompt: &str,
    llm_call_duration: Duration,
    turn_start: Instant,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    accumulate_turn_state(state, &assistant_message);

    // Record the index where we insert the assistant message so we can replace
    // it later if a post-turn policy returns a mutated version.
    let assistant_ctx_index = state.context_messages.len();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(
            assistant_message.clone(),
        )));
    let msg_for_turn_end = assistant_message;

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
    let mut detected_transfer_signal: Option<crate::transfer::TransferSignal> = None;
    let mut dispatch_stop_reason: Option<String> = None;

    if !tool_call_data.is_empty() {
        let exec_results =
            execute_tools_concurrently(config, &tool_call_data, cancellation_token, tx).await;

        match exec_results {
            ToolExecOutcome::Completed {
                results,
                tool_metrics,
                transfer_signal,
                injected_messages,
            } => {
                tool_results.extend(results);
                collected_tool_metrics = tool_metrics;
                detected_transfer_signal = transfer_signal;
                state.pending_messages.extend(injected_messages);
                sync_pending_message_snapshot(config, state);
            }
            ToolExecOutcome::Stopped {
                results,
                tool_metrics,
                reason,
                injected_messages,
            } => {
                tool_results.extend(results);
                collected_tool_metrics = tool_metrics;
                dispatch_stop_reason = Some(reason);
                state.pending_messages.extend(injected_messages);
                sync_pending_message_snapshot(config, state);
            }
            ToolExecOutcome::SteeringInterrupt {
                completed,
                cancelled,
                steering_messages,
                tool_metrics,
                injected_messages,
            } => {
                tool_results.extend(completed);
                tool_results.extend(cancelled);
                steering_interrupted = true;
                collected_tool_metrics = tool_metrics;
                state.pending_messages.extend(injected_messages);
                state.pending_messages.extend(steering_messages);
                sync_pending_message_snapshot(config, state);
            }
            ToolExecOutcome::Aborted {
                results,
                tool_metrics,
                injected_messages,
            } => {
                tool_results.extend(results);
                emit_turn_metrics(
                    config,
                    state,
                    &msg_for_turn_end,
                    llm_call_duration,
                    tool_metrics,
                    turn_start,
                )
                .await;

                state.pending_messages.extend(injected_messages);
                sync_pending_message_snapshot(config, state);

                for tr in &tool_results {
                    state
                        .context_messages
                        .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                }

                state.last_tool_results.clone_from(&tool_results);

                let aborted_turn_end = mark_assistant_message_aborted(&msg_for_turn_end);
                let (aborted_turn_end, _) = run_post_turn_policy_check(
                    &aborted_turn_end,
                    &tool_results,
                    state,
                    config,
                    system_prompt,
                );
                state.context_messages[assistant_ctx_index] =
                    AgentMessage::Llm(LlmMessage::Assistant(aborted_turn_end.clone()));

                let state_delta = flush_state_delta(config, tx).await;
                let snapshot = build_snapshot(state, StopReason::Aborted, state_delta);
                return emit_turn_end_and_agent_end(
                    aborted_turn_end,
                    tool_results,
                    TurnEndReason::Cancelled,
                    snapshot,
                    state,
                    tx,
                )
                .await;
            }
            ToolExecOutcome::ChannelClosed => return TurnOutcome::Return,
        }
    }

    emit_turn_metrics(
        config,
        state,
        &msg_for_turn_end,
        llm_call_duration,
        collected_tool_metrics,
        turn_start,
    )
    .await;

    // xii. Add tool result messages to context
    for tr in &tool_results {
        state
            .context_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
    }

    // Store tool results for post-turn hook access
    state.last_tool_results.clone_from(&tool_results);

    // xiii. Run post-turn policies against the committed tool-turn snapshot
    // before emitting TurnEnd or honoring transfer termination.
    let (msg_for_turn_end, policy_stop) = run_post_turn_policy_check(
        &msg_for_turn_end,
        &tool_results,
        state,
        config,
        system_prompt,
    );

    // Update the assistant message in context in case a policy replaced it.
    state.context_messages[assistant_ctx_index] =
        AgentMessage::Llm(LlmMessage::Assistant(msg_for_turn_end.clone()));

    // xiii-a. Transfer signal detection: if a tool signaled a transfer,
    // validate against the transfer chain for safety, then enrich and exit.
    if let Some(mut signal) = detected_transfer_signal {
        // Enforce transfer chain safety: check for circular transfers and
        // max-depth violations before honoring the transfer.
        match state.transfer_chain.push(signal.target_agent()) {
            Ok(()) => {
                // Chain check passed — proceed with the transfer.
            }
            Err(chain_err) => {
                // Chain check failed — convert transfer to an error tool result
                // so the LLM can retry or take a different action.
                tracing::warn!(
                    target_agent = %signal.target_agent(),
                    error = %chain_err,
                    "transfer chain safety check failed, rejecting transfer"
                );

                // The transfer is rejected; continue the inner loop instead
                // of terminating.
                let state_delta = flush_state_delta(config, tx).await;
                let snapshot = build_snapshot(state, msg_for_turn_end.stop_reason, state_delta);
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

                if let Some(reason) = policy_stop {
                    tracing::info!("post-turn policy stopped agent: {reason}");
                    return emit_agent_end(state, tx).await;
                }

                return TurnOutcome::ContinueInner;
            }
        }

        let llm_history: Vec<LlmMessage> = state
            .context_messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();
        signal = signal
            .with_conversation_history(llm_history)
            .with_transfer_chain(state.transfer_chain.clone());

        tracing::info!(
            target_agent = %signal.target_agent(),
            reason = %signal.reason(),
            "transfer signal detected, terminating turn"
        );

        let _ = emit(
            tx,
            AgentEvent::TransferInitiated {
                signal: signal.clone(),
            },
        )
        .await;

        let state_delta = flush_state_delta(config, tx).await;
        let snapshot = build_snapshot(state, StopReason::Transfer, state_delta);
        return emit_turn_end_and_agent_end(
            msg_for_turn_end,
            tool_results,
            TurnEndReason::Transfer,
            snapshot,
            state,
            tx,
        )
        .await;
    }

    // xiv. Emit TurnEnd
    let state_delta = flush_state_delta(config, tx).await;
    let snapshot = build_snapshot(state, msg_for_turn_end.stop_reason, state_delta);
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

    if let Some(reason) = dispatch_stop_reason {
        tracing::info!("tool dispatch stopped agent: {reason}");
        return emit_agent_end(state, tx).await;
    }

    if let Some(reason) = policy_stop {
        tracing::info!("post-turn policy stopped agent: {reason}");
        return emit_agent_end(state, tx).await;
    }

    // Poll steering if not already interrupted
    if !steering_interrupted && let Some(ref provider) = config.message_provider {
        let msgs = provider.poll_steering();
        if !msgs.is_empty() {
            state.pending_messages.extend(msgs);
            sync_pending_message_snapshot(config, state);
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
                    cache_hint: None,
                });
            } else {
                remaining.push(tc);
            }
        }
        *tool_call_data = remaining;
    }
    max_token_results
}
