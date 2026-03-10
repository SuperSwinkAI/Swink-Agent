//! Core agent loop execution engine.
//!
//! Implements the nested inner/outer loop, tool dispatch, steering/follow-up
//! injection, event emission, retry integration, error/abort handling, and max
//! tokens recovery. Stateless — all state is passed in via [`AgentLoopConfig`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::error::HarnessError;
use crate::retry::RetryStrategy;
use crate::stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamFn, StreamOptions, accumulate_message,
};
use crate::tool::{AgentTool, AgentToolResult, validate_tool_arguments, validation_error_result};
use crate::types::{
    AgentContext, AgentMessage, AssistantMessage, ContentBlock, LlmMessage, ModelSpec, StopReason,
    ToolResultMessage,
};

// ─── Type Aliases ────────────────────────────────────────────────────────────

/// Converts an `AgentMessage` to an optional `LlmMessage` for the provider.
type ConvertToLlmFn = dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync;

/// Context transformation hook with overflow signal.
type TransformContextFn = dyn Fn(&mut Vec<AgentMessage>, bool) + Send + Sync;

/// Async API key resolution callback.
type GetApiKeyFn =
    dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync;

/// Callback that returns steering or follow-up messages.
type MessageProviderFn = dyn Fn() -> Vec<AgentMessage> + Send + Sync;

// ─── AgentEvent ──────────────────────────────────────────────────────────────

/// Fine-grained lifecycle event emitted by the agent loop.
///
/// Consumers subscribe to these events for observability, UI updates, and
/// logging. The harness never calls back into application logic for display
/// concerns.
#[derive(Debug)]
pub enum AgentEvent {
    /// Emitted once when the loop begins.
    AgentStart,

    /// Emitted once when the loop exits, carrying all new messages produced.
    AgentEnd { messages: Arc<Vec<AgentMessage>> },

    /// Emitted at the beginning of each assistant turn.
    TurnStart,

    /// Emitted at the end of each turn with the assistant message and tool results.
    TurnEnd {
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
    },

    /// Emitted when a message begins streaming.
    MessageStart,

    /// Emitted for each incremental delta during assistant streaming.
    MessageUpdate { delta: AssistantMessageDelta },

    /// Emitted when a message is complete.
    MessageEnd { message: AssistantMessage },

    /// Emitted when a tool call begins execution.
    ToolExecutionStart {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Emitted for intermediate partial results from a streaming tool.
    ToolExecutionUpdate { partial: AgentToolResult },

    /// Emitted when a tool call completes.
    ToolExecutionEnd {
        result: AgentToolResult,
        is_error: bool,
    },
}

// ─── AgentLoopConfig ─────────────────────────────────────────────────────────

/// Configuration for the agent loop.
///
/// Carries the model spec, stream options, retry strategy, stream function,
/// tools, and all the hooks that the loop calls at various points.
pub struct AgentLoopConfig {
    /// Model specification passed through to `StreamFn`.
    pub model: ModelSpec,

    /// Stream options passed through to `StreamFn`.
    pub stream_options: StreamOptions,

    /// Retry strategy applied to model calls.
    pub retry_strategy: Box<dyn RetryStrategy>,

    /// The pluggable streaming function that calls the LLM provider.
    pub stream_fn: Arc<dyn StreamFn>,

    /// Available tools for the agent to call.
    pub tools: Vec<Arc<dyn AgentTool>>,

    /// Converts an `AgentMessage` to an `LlmMessage` for the provider.
    /// Returns `None` to filter out custom or UI-only messages.
    pub convert_to_llm: Box<ConvertToLlmFn>,

    /// Optional hook called before `convert_to_llm`; used for context pruning,
    /// token budget enforcement, or external context injection.
    /// The second parameter is the overflow signal — when true, the hook should
    /// prune more aggressively.
    pub transform_context: Option<Box<TransformContextFn>>,

    /// Optional async callback for dynamic API key resolution.
    pub get_api_key: Option<Box<GetApiKeyFn>>,

    /// Optional callback polled after each tool execution for steering messages.
    pub get_steering_messages: Option<Box<MessageProviderFn>>,

    /// Optional callback polled when the agent would otherwise stop.
    pub get_follow_up_messages: Option<Box<MessageProviderFn>>,
}

impl std::fmt::Debug for AgentLoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopConfig")
            .field("model", &self.model)
            .field("stream_options", &self.stream_options)
            .field("tools", &format_args!("[{} tool(s)]", self.tools.len()))
            .finish_non_exhaustive()
    }
}

// ─── Entry Points ────────────────────────────────────────────────────────────

/// Start a new agent loop with prompt messages.
///
/// Creates an initial context with the prompt messages, then runs the loop.
/// Returns a stream of `AgentEvent` values.
#[must_use]
pub fn agent_loop(
    prompt_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    run_loop(prompt_messages, system_prompt, config, cancellation_token)
}

/// Resume an agent loop from existing messages.
///
/// Resumes from existing messages (no new prompt), calls the loop.
/// Returns a stream of `AgentEvent` values.
#[must_use]
pub fn agent_loop_continue(
    messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    run_loop(messages, system_prompt, config, cancellation_token)
}

// ─── Internal Loop ───────────────────────────────────────────────────────────

/// The core loop implementation. Spawns a task that drives the loop and sends
/// events through an mpsc channel, returning a stream of events.
fn run_loop(
    initial_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    tokio::spawn(async move {
        run_loop_inner(
            initial_messages,
            system_prompt,
            config,
            cancellation_token,
            tx,
        )
        .await;
    });

    Box::pin(ReceiverStream::new(rx))
}

/// Send an event through the channel. Returns false if the receiver is dropped.
async fn emit(tx: &mpsc::Sender<AgentEvent>, event: AgentEvent) -> bool {
    tx.send(event).await.is_ok()
}

// ─── Loop State ──────────────────────────────────────────────────────────────

/// Mutable state threaded through the loop iterations.
struct LoopState {
    context_messages: Vec<AgentMessage>,
    all_new_messages: Vec<AgentMessage>,
    pending_messages: Vec<AgentMessage>,
    overflow_signal: bool,
}

// ─── run_loop_inner ──────────────────────────────────────────────────────────

/// The actual loop logic running inside the spawned task.
async fn run_loop_inner(
    initial_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
    tx: mpsc::Sender<AgentEvent>,
) {
    let config = Arc::new(config);
    let mut state = LoopState {
        context_messages: initial_messages,
        all_new_messages: Vec::new(),
        pending_messages: Vec::new(),
        overflow_signal: false,
    };

    // 1. Emit AgentStart
    if !emit(&tx, AgentEvent::AgentStart).await {
        return;
    }

    // 2. Outer loop (follow-up phase)
    'outer: loop {
        // Inner loop (turn + tool phase)
        'inner: loop {
            let turn_result = run_single_turn(
                &config,
                &mut state,
                &system_prompt,
                &cancellation_token,
                &tx,
            )
            .await;

            match turn_result {
                TurnOutcome::ContinueInner => {}
                TurnOutcome::BreakInner => break 'inner,
                TurnOutcome::Return => return,
            }
        }

        // Outer loop: poll get_follow_up_messages
        if let Some(ref get_follow_up) = config.get_follow_up_messages {
            let msgs = get_follow_up();
            if !msgs.is_empty() {
                state.pending_messages.extend(msgs);
                continue 'outer;
            }
        }

        // No follow-up → emit AgentEnd and exit
        let _ = emit(
            &tx,
            AgentEvent::AgentEnd {
                messages: Arc::new(state.all_new_messages),
            },
        )
        .await;
        return;
    }
}

/// Outcome of a single turn execution within the inner loop.
enum TurnOutcome {
    /// Continue to the next inner-loop iteration (tool results need processing).
    ContinueInner,
    /// Break out of the inner loop (no tool calls, check follow-ups).
    BreakInner,
    /// Return from the entire loop (channel closed, error, or abort).
    Return,
}

/// Run a single turn of the inner loop: inject pending messages, transform
/// context, stream the assistant response, handle tool calls, and emit events.
async fn run_single_turn(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    system_prompt: &str,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
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

    // ii. Call transform_context if set
    if let Some(ref transform) = config.transform_context {
        transform(&mut state.context_messages, state.overflow_signal);
    }
    // Reset overflow after it's been signaled
    state.overflow_signal = false;

    // iii. Apply convert_to_llm to filter messages for the provider
    let llm_messages: Vec<LlmMessage> = state
        .context_messages
        .iter()
        .filter_map(|m| (config.convert_to_llm)(m))
        .collect();

    // iv. Call get_api_key if set
    if let Some(ref get_key) = config.get_api_key {
        let _api_key = get_key(&config.model.provider).await;
    }

    // v. Build context and call StreamFn with retry logic
    let agent_context = AgentContext {
        system_prompt: system_prompt.to_string(),
        messages: Vec::new(),
        tools: config.tools.clone(),
    };

    let stream_result = stream_with_retry(
        config,
        &agent_context,
        &llm_messages,
        system_prompt,
        cancellation_token,
        tx,
    )
    .await;

    let Some(assistant_message) = handle_stream_result(stream_result, config, state, tx).await
    else {
        return TurnOutcome::Return;
    };

    // Check if ContextOverflow sentinel was returned
    if assistant_message.stop_reason == StopReason::Length
        && assistant_message.error_message.as_deref() == Some("__context_overflow__")
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
        return handle_no_tool_calls(assistant_message, state, tx).await;
    }

    // x–xiii. Process tool calls
    handle_tool_calls(
        config,
        state,
        assistant_message,
        tool_calls,
        cancellation_token,
        tx,
    )
    .await
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
        .all_new_messages
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
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: abort_msg_clone,
            tool_results: vec![],
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }
    let _ = emit(
        tx,
        AgentEvent::AgentEnd {
            messages: Arc::new(std::mem::take(&mut state.all_new_messages)),
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
                error_message: Some("__context_overflow__".to_string()),
                timestamp: 0,
            })
        }
        StreamResult::Aborted => {
            let abort_msg = build_abort_message(&config.model);
            let abort_msg_clone = abort_msg.clone();
            state
                .all_new_messages
                .push(AgentMessage::Llm(LlmMessage::Assistant(abort_msg)));
            if !emit(
                tx,
                AgentEvent::TurnEnd {
                    assistant_message: abort_msg_clone,
                    tool_results: vec![],
                },
            )
            .await
            {
                return None;
            }
            let _ = emit(
                tx,
                AgentEvent::AgentEnd {
                    messages: Arc::new(std::mem::take(&mut state.all_new_messages)),
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
    let msg_clone = assistant_message.clone();
    state
        .all_new_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_clone,
            tool_results: vec![],
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
            messages: Arc::new(std::mem::take(&mut state.all_new_messages)),
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
async fn handle_no_tool_calls(
    assistant_message: AssistantMessage,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    let msg_clone = assistant_message.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    state
        .all_new_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(msg_clone.clone())));
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_clone,
            tool_results: vec![],
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
async fn handle_tool_calls(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    assistant_message: AssistantMessage,
    mut tool_call_data: Vec<ToolCallInfo>,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> TurnOutcome {
    // Add assistant message to context
    let msg_for_turn_end = assistant_message.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
    state
        .all_new_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(
            msg_for_turn_end.clone(),
        )));

    // Max tokens recovery: replace incomplete tool calls with error results
    let max_token_results =
        recover_incomplete_tool_calls(&mut tool_call_data, msg_for_turn_end.stop_reason);

    // Add max token error results to context
    for tr in &max_token_results {
        state
            .context_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
        state
            .all_new_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
    }

    // xi. Execute tool calls concurrently
    let mut tool_results: Vec<ToolResultMessage> = max_token_results;
    let mut steering_interrupted = false;

    if !tool_call_data.is_empty() {
        let exec_results =
            execute_tools_concurrently(config, &tool_call_data, cancellation_token, tx).await;

        match exec_results {
            ToolExecOutcome::Completed(results) => {
                tool_results.extend(results);
            }
            ToolExecOutcome::SteeringInterrupt {
                completed,
                cancelled,
                steering_messages,
            } => {
                tool_results.extend(completed);
                tool_results.extend(cancelled);
                steering_interrupted = true;
                state.pending_messages.extend(steering_messages);
            }
            ToolExecOutcome::ChannelClosed => return TurnOutcome::Return,
        }
    }

    // xii. Add tool result messages to context
    for tr in &tool_results {
        state
            .context_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
        state
            .all_new_messages
            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
    }

    // xiii. Emit TurnEnd
    if !emit(
        tx,
        AgentEvent::TurnEnd {
            assistant_message: msg_for_turn_end,
            tool_results,
        },
    )
    .await
    {
        return TurnOutcome::Return;
    }

    // Poll steering if not already interrupted
    if !steering_interrupted {
        if let Some(ref get_steering) = config.get_steering_messages {
            let msgs = get_steering();
            if !msgs.is_empty() {
                state.pending_messages.extend(msgs);
            }
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
                });
            } else {
                remaining.push(tc);
            }
        }
        *tool_call_data = remaining;
    }
    max_token_results
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Info about a tool call extracted from the assistant message.
struct ToolCallInfo {
    id: String,
    name: String,
    arguments: serde_json::Value,
    is_incomplete: bool,
}

/// Result of streaming an assistant response.
enum StreamResult {
    Message(AssistantMessage),
    ContextOverflow,
    Aborted,
    ChannelClosed,
}

/// Outcome of concurrent tool execution.
enum ToolExecOutcome {
    Completed(Vec<ToolResultMessage>),
    SteeringInterrupt {
        completed: Vec<ToolResultMessage>,
        cancelled: Vec<ToolResultMessage>,
        steering_messages: Vec<AgentMessage>,
    },
    ChannelClosed,
}

/// Get the current Unix timestamp in seconds.
fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Build an aborted `AssistantMessage`.
fn build_abort_message(model: &ModelSpec) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        provider: model.provider.clone(),
        model_id: model.model_id.clone(),
        usage: crate::types::Usage::default(),
        cost: crate::types::Cost::default(),
        stop_reason: StopReason::Aborted,
        error_message: Some("operation aborted via cancellation token".to_string()),
        timestamp: now_timestamp(),
    }
}

/// Build an error `AssistantMessage` from a `HarnessError`.
fn build_error_message(model: &ModelSpec, error: &HarnessError) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        provider: model.provider.clone(),
        model_id: model.model_id.clone(),
        usage: crate::types::Usage::default(),
        cost: crate::types::Cost::default(),
        stop_reason: StopReason::Error,
        error_message: Some(error.to_string()),
        timestamp: now_timestamp(),
    }
}

/// Classify an `AssistantMessageEvent::Error` into a `HarnessError`.
fn classify_stream_error(error_message: &str, stop_reason: StopReason) -> HarnessError {
    let lower = error_message.to_lowercase();
    if lower.contains("context window") || lower.contains("context_length_exceeded") {
        return HarnessError::ContextWindowOverflow {
            model: String::new(),
        };
    }
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("throttl") {
        return HarnessError::ModelThrottled;
    }
    if stop_reason == StopReason::Aborted {
        return HarnessError::Aborted;
    }
    HarnessError::StreamError {
        source: Box::new(std::io::Error::other(error_message.to_string())),
    }
}

// ─── stream_with_retry ───────────────────────────────────────────────────────

/// Stream an assistant response with retry logic, emitting message events.
async fn stream_with_retry(
    config: &Arc<AgentLoopConfig>,
    agent_context: &AgentContext,
    llm_messages: &[LlmMessage],
    system_prompt: &str,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;

        // Check cancellation before each attempt
        if cancellation_token.is_cancelled() {
            return StreamResult::Aborted;
        }

        // Build the context with LLM messages for this call
        let call_context = AgentContext {
            system_prompt: system_prompt.to_string(),
            messages: llm_messages
                .iter()
                .map(|m| AgentMessage::Llm(m.clone()))
                .collect(),
            tools: agent_context.tools.clone(),
        };

        // Emit MessageStart
        if !emit(tx, AgentEvent::MessageStart).await {
            return StreamResult::ChannelClosed;
        }

        // Stream from the provider and collect events + emit deltas
        let attempt_result =
            stream_single_attempt(config, &call_context, cancellation_token, tx).await;

        let (events, had_error) = match attempt_result {
            StreamAttemptResult::EarlyExit(result) => return result,
            StreamAttemptResult::Collected { events, error } => (events, error),
        };

        // Handle error events
        if let Some((stop_reason, error_message, _usage)) = had_error {
            let retry_result =
                handle_stream_error(config, &stop_reason, &error_message, attempt, tx).await;
            match retry_result {
                StreamErrorAction::ContextOverflow => return StreamResult::ContextOverflow,
                StreamErrorAction::Retry(delay) => {
                    tokio::time::sleep(delay).await;
                    continue;
                }
                StreamErrorAction::FatalError(msg) => return msg,
                StreamErrorAction::ChannelClosed => return StreamResult::ChannelClosed,
            }
        }

        // Success: accumulate and emit
        return finalize_stream_message(config, events, tx).await;
    }
}

// ─── stream_with_retry helpers ───────────────────────────────────────────────

/// Possible outcomes when handling a stream error.
enum StreamErrorAction {
    ContextOverflow,
    Retry(std::time::Duration),
    FatalError(StreamResult),
    ChannelClosed,
}

/// Result of streaming a single attempt from the provider.
enum StreamAttemptResult {
    /// Events were collected successfully (may include an error event).
    Collected {
        events: Vec<AssistantMessageEvent>,
        error: Option<(StopReason, String, Option<crate::types::Usage>)>,
    },
    /// Early exit due to cancellation or channel close.
    EarlyExit(StreamResult),
}

/// Stream a single attempt from the provider, emitting delta events.
/// Collects all events and captures any error info for the caller.
async fn stream_single_attempt(
    config: &Arc<AgentLoopConfig>,
    call_context: &AgentContext,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamAttemptResult {
    let mut stream = config.stream_fn.stream(
        &config.model,
        call_context,
        &config.stream_options,
        cancellation_token.clone(),
    );

    let mut events: Vec<AssistantMessageEvent> = Vec::new();
    let mut had_error: Option<(StopReason, String, Option<crate::types::Usage>)> = None;

    while let Some(event) = stream.next().await {
        if cancellation_token.is_cancelled() {
            let abort_msg = build_abort_message(&config.model);
            let _ = emit(tx, AgentEvent::MessageEnd { message: abort_msg }).await;
            return StreamAttemptResult::EarlyExit(StreamResult::Aborted);
        }

        if let Some(early_exit) = emit_delta_event(&event, tx).await {
            return StreamAttemptResult::EarlyExit(early_exit);
        }

        if let AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
        } = &event
        {
            had_error = Some((*stop_reason, error_message.clone(), *usage));
        }

        events.push(event);
    }

    StreamAttemptResult::Collected {
        events,
        error: had_error,
    }
}

/// Emit a delta event for a single stream event. Returns `Some(StreamResult)`
/// if the channel is closed, otherwise `None`.
async fn emit_delta_event(
    event: &AssistantMessageEvent,
    tx: &mpsc::Sender<AgentEvent>,
) -> Option<StreamResult> {
    let delta = match event {
        AssistantMessageEvent::TextDelta {
            content_index,
            delta,
        } => Some(AssistantMessageDelta::Text {
            content_index: *content_index,
            delta: delta.clone(),
        }),
        AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta,
        } => Some(AssistantMessageDelta::Thinking {
            content_index: *content_index,
            delta: delta.clone(),
        }),
        AssistantMessageEvent::ToolCallDelta {
            content_index,
            delta,
        } => Some(AssistantMessageDelta::ToolCall {
            content_index: *content_index,
            delta: delta.clone(),
        }),
        _ => None,
    };

    if let Some(d) = delta {
        if !emit(tx, AgentEvent::MessageUpdate { delta: d }).await {
            return Some(StreamResult::ChannelClosed);
        }
    }
    None
}

/// Handle a stream error: classify it, check retryability, return action.
async fn handle_stream_error(
    config: &Arc<AgentLoopConfig>,
    stop_reason: &StopReason,
    error_message: &str,
    attempt: u32,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamErrorAction {
    let harness_error = classify_stream_error(error_message, *stop_reason);

    // Context window overflow — signal and retry
    if matches!(harness_error, HarnessError::ContextWindowOverflow { .. }) {
        let _ = emit(
            tx,
            AgentEvent::MessageEnd {
                message: build_error_message(&config.model, &harness_error),
            },
        )
        .await;
        return StreamErrorAction::ContextOverflow;
    }

    // Check if retryable
    if harness_error.is_retryable() && config.retry_strategy.should_retry(&harness_error, attempt) {
        let delay = config.retry_strategy.delay(attempt);
        return StreamErrorAction::Retry(delay);
    }

    // Non-retryable error
    let error_msg = build_error_message(&config.model, &harness_error);
    if !emit(
        tx,
        AgentEvent::MessageEnd {
            message: error_msg.clone(),
        },
    )
    .await
    {
        return StreamErrorAction::ChannelClosed;
    }
    StreamErrorAction::FatalError(StreamResult::Message(error_msg))
}

/// Accumulate collected stream events into a final message and emit `MessageEnd`.
async fn finalize_stream_message(
    config: &Arc<AgentLoopConfig>,
    events: Vec<AssistantMessageEvent>,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    let message = match accumulate_message(events, &config.model.provider, &config.model.model_id) {
        Ok(msg) => msg,
        Err(e) => {
            let err = HarnessError::StreamError {
                source: Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            };
            let error_msg = build_error_message(&config.model, &err);
            let _ = emit(
                tx,
                AgentEvent::MessageEnd {
                    message: error_msg.clone(),
                },
            )
            .await;
            return StreamResult::Message(error_msg);
        }
    };

    // Emit MessageEnd
    if !emit(
        tx,
        AgentEvent::MessageEnd {
            message: message.clone(),
        },
    )
    .await
    {
        return StreamResult::ChannelClosed;
    }

    StreamResult::Message(message)
}

// ─── execute_tools_concurrently ──────────────────────────────────────────────

/// Execute tool calls concurrently, checking for steering interrupts after each.
async fn execute_tools_concurrently(
    config: &Arc<AgentLoopConfig>,
    tool_calls: &[ToolCallInfo],
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> ToolExecOutcome {
    use tokio::sync::Mutex;

    let batch_token = cancellation_token.child_token();
    let results: Arc<Mutex<Vec<(usize, ToolResultMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let steering_detected: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));

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

        let handle = dispatch_single_tool(
            config,
            tc,
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

/// Result of dispatching a single tool call.
enum DispatchResult {
    /// Tool was spawned as a tokio task.
    Spawned(tokio::task::JoinHandle<()>),
    /// Tool result was added inline (unknown tool).
    Inline,
}

/// Validate and dispatch a single tool call, returning a join handle or inline result.
async fn dispatch_single_tool(
    config: &Arc<AgentLoopConfig>,
    tc: &ToolCallInfo,
    idx: usize,
    batch_token: &CancellationToken,
    results: &Arc<tokio::sync::Mutex<Vec<(usize, ToolResultMessage)>>>,
    steering_flag: &Arc<std::sync::atomic::AtomicBool>,
    tx: &mpsc::Sender<AgentEvent>,
) -> DispatchResult {
    let tool = config.tools.iter().find(|t| t.name() == tc.name);

    let tool_call_id = tc.id.clone();
    let tool_name = tc.name.clone();
    let arguments = tc.arguments.clone();

    if let Some(tool) = tool {
        let tool = Arc::clone(tool);
        let child_token = batch_token.child_token();
        let results_clone = Arc::clone(results);
        let steering_clone = Arc::clone(steering_flag);
        let config_clone = Arc::clone(config);
        let tx_clone = tx.clone();

        let validation = validate_tool_arguments(tool.parameters_schema(), &arguments);

        let handle = tokio::spawn(async move {
            let (result, is_error) = if let Err(errors) = validation {
                (validation_error_result(&errors), true)
            } else {
                let result = tool
                    .execute(&tool_call_id, arguments, child_token, None)
                    .await;
                let is_error = result
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.starts_with("error")));
                (result, is_error)
            };

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
            };

            results_clone.lock().await.push((idx, tool_result_msg));

            if let Some(ref get_steering) = config_clone.get_steering_messages {
                let msgs = get_steering();
                if !msgs.is_empty() {
                    steering_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });

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

    while let Some((_, join_result)) = futs.next().await {
        if join_result.is_err() {
            continue;
        }

        if steering_detected.load(std::sync::atomic::Ordering::SeqCst) {
            batch_token.cancel();
            while futs.next().await.is_some() {}

            let all_results = results.lock().await;
            let mut completed: Vec<ToolResultMessage> = Vec::new();
            let mut cancelled: Vec<ToolResultMessage> = Vec::new();

            for tc in tool_calls {
                if let Some((_, result)) = all_results.iter().find(|(_, r)| r.tool_call_id == tc.id)
                {
                    completed.push(result.clone());
                } else {
                    cancelled.push(ToolResultMessage {
                        tool_call_id: tc.id.clone(),
                        content: vec![ContentBlock::Text {
                            text: "tool call cancelled: user requested steering interrupt"
                                .to_string(),
                        }],
                        is_error: true,
                        timestamp: now_timestamp(),
                    });
                }
            }

            let steering_messages = config
                .get_steering_messages
                .as_ref()
                .map_or_else(Vec::new, |get_steering| get_steering());

            return ToolExecOutcome::SteeringInterrupt {
                completed,
                cancelled,
                steering_messages,
            };
        }
    }

    // All tools completed without steering
    let all_results = results.lock().await;
    let mut ordered: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        if let Some((_, result)) = all_results.iter().find(|(_, r)| r.tool_call_id == tc.id) {
            ordered.push(result.clone());
        }
    }

    ToolExecOutcome::Completed(ordered)
}
