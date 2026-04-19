use std::borrow::Cow;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, info_span, warn};

use crate::error::AgentError;
use crate::stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamFn, StreamOptions, accumulate_message,
};
use crate::types::{
    AgentContext, AgentMessage, AssistantMessage, LlmMessage, ModelSpec, StopReason, ThinkingLevel,
};

use super::{
    AgentEvent, AgentLoopConfig, StreamResult, build_abort_message, build_error_message,
    classify_stream_error, emit,
};

/// Stream an assistant response with retry logic and optional model fallback,
/// emitting message events.
///
/// When the primary model exhausts its retry budget on a retryable error and a
/// [`ModelFallback`](crate::fallback::ModelFallback) chain is configured, each
/// fallback model is tried in order (with its own fresh retry budget) before
/// the error is surfaced.
pub async fn stream_with_retry(
    config: &Arc<AgentLoopConfig>,
    agent_context: &AgentContext,
    llm_messages: &[LlmMessage],
    system_prompt: &str,
    api_key: Option<String>,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    // Try the primary model first.
    let primary_result = stream_with_retry_single(
        &config.model,
        &config.stream_fn,
        config,
        agent_context,
        llm_messages,
        system_prompt,
        api_key.clone(),
        cancellation_token,
        tx,
    )
    .await;

    // If the primary model succeeded or hit a non-fallback-eligible condition,
    // return immediately.
    let last_error_msg = match &primary_result {
        StreamResult::Message(msg)
            if msg.stop_reason != StopReason::Error || !is_fallback_eligible_error(msg) =>
        {
            return primary_result;
        }
        StreamResult::ContextOverflow | StreamResult::Aborted | StreamResult::ChannelClosed => {
            return primary_result;
        }
        StreamResult::Message(msg) => msg.clone(),
    };

    // Try each fallback model in order.
    let fallback = match config.fallback {
        Some(ref fb) if !fb.is_empty() => fb,
        _ => return StreamResult::Message(last_error_msg),
    };

    let mut last_result = StreamResult::Message(last_error_msg);

    for (fb_model, fb_stream_fn) in fallback.models() {
        // Emit the fallback event.
        if !emit(
            tx,
            AgentEvent::ModelFallback {
                from_model: config.model.clone(),
                to_model: fb_model.clone(),
            },
        )
        .await
        {
            return StreamResult::ChannelClosed;
        }

        warn!(
            from = %config.model.model_id,
            to = %fb_model.model_id,
            "falling back to alternate model"
        );

        let fb_result = stream_with_retry_single(
            fb_model,
            fb_stream_fn,
            config,
            agent_context,
            llm_messages,
            system_prompt,
            api_key.clone(),
            cancellation_token,
            tx,
        )
        .await;

        match &fb_result {
            StreamResult::Message(msg)
                if msg.stop_reason != StopReason::Error || !is_fallback_eligible_error(msg) =>
            {
                return fb_result;
            }
            StreamResult::ContextOverflow | StreamResult::Aborted | StreamResult::ChannelClosed => {
                return fb_result;
            }
            StreamResult::Message(_) => {
                // This fallback also failed with a retryable error; try next.
                last_result = fb_result;
            }
        }
    }

    // All fallbacks exhausted.
    last_result
}

/// Returns `true` if the error indicates a retryable failure that
/// should trigger model fallback (throttled or network errors).
///
/// Uses structural `error_kind` when available, falling back to
/// string-based classification for external adapters.
fn is_fallback_eligible_error(msg: &AssistantMessage) -> bool {
    let Some(error_message) = msg.error_message.as_deref() else {
        return false;
    };
    let harness_error = classify_stream_error(error_message, StopReason::Error, msg.error_kind);
    harness_error.is_retryable()
}

/// Run the retry loop for a single model/stream-fn pair.
#[allow(clippy::too_many_arguments)]
async fn stream_with_retry_single(
    model: &ModelSpec,
    stream_fn: &Arc<dyn StreamFn>,
    config: &Arc<AgentLoopConfig>,
    agent_context: &AgentContext,
    llm_messages: &[LlmMessage],
    system_prompt: &str,
    api_key: Option<String>,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    let llm_span = info_span!(
        "agent.llm_call",
        agent.model = %model.model_id,
        agent.tokens.input = tracing::field::Empty,
        agent.tokens.output = tracing::field::Empty,
        agent.cost.total = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    );
    let _llm_guard = llm_span.enter();

    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        debug!(attempt, model_id = %model.model_id, "starting stream attempt");

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
        let mut stream_options = config.stream_options.clone();
        stream_options.api_key = api_key.clone();

        // Emit MessageStart
        if !emit(tx, AgentEvent::MessageStart).await {
            return StreamResult::ChannelClosed;
        }

        // Stream from the provider and collect events + emit deltas
        let attempt_result = stream_single_attempt(
            model,
            stream_fn,
            &call_context,
            &stream_options,
            cancellation_token,
            tx,
        )
        .await;

        let (events, had_error) = match attempt_result {
            StreamAttemptResult::EarlyExit(result) => return result,
            StreamAttemptResult::Collected { events, error } => (events, error),
        };

        // Handle error events
        if let Some((stop_reason, error_message, _usage, error_kind)) = had_error {
            let retry_result = handle_stream_error(
                model,
                config,
                &stop_reason,
                &error_message,
                error_kind,
                attempt,
                tx,
            )
            .await;
            match retry_result {
                StreamErrorAction::ContextOverflow => return StreamResult::ContextOverflow,
                StreamErrorAction::Retry(delay) => {
                    tokio::time::sleep(delay).await;
                    continue;
                }
                StreamErrorAction::FatalError(msg) => {
                    llm_span.record("otel.status_code", "ERROR");
                    return msg;
                }
                StreamErrorAction::ChannelClosed => return StreamResult::ChannelClosed,
            }
        }

        // Success: accumulate and emit
        let result = finalize_stream_message(model, events, tx).await;
        if let StreamResult::Message(ref msg) = result {
            llm_span.record("agent.tokens.input", msg.usage.input);
            llm_span.record("agent.tokens.output", msg.usage.output);
            llm_span.record("agent.cost.total", msg.cost.total);
        }
        return result;
    }
}

// ─── stream_with_retry helpers ───────────────────────────────────────────────

/// Possible outcomes when handling a stream error.
#[allow(clippy::large_enum_variant)]
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
        error: Option<(
            StopReason,
            String,
            Option<crate::types::Usage>,
            Option<crate::stream::StreamErrorKind>,
        )>,
    },
    /// Early exit due to cancellation or channel close.
    EarlyExit(StreamResult),
}

/// Stream a single attempt from the provider, emitting delta events.
/// Collects all events and captures any error info for the caller.
async fn stream_single_attempt(
    model: &ModelSpec,
    stream_fn: &Arc<dyn StreamFn>,
    call_context: &AgentContext,
    stream_options: &StreamOptions,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamAttemptResult {
    // Apply capability overrides (e.g. disable thinking for models that
    // don't support it) before handing the spec to the provider.
    let effective_model = apply_capability_overrides(model);

    let mut stream = stream_fn.stream(
        &effective_model,
        call_context,
        stream_options,
        cancellation_token.clone(),
    );

    let mut events: Vec<AssistantMessageEvent> = Vec::new();
    let mut had_error: Option<(
        StopReason,
        String,
        Option<crate::types::Usage>,
        Option<crate::stream::StreamErrorKind>,
    )> = None;

    while let Some(event) = stream.next().await {
        if cancellation_token.is_cancelled() {
            let abort_msg = build_abort_message(model);
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
            error_kind,
        } = &event
        {
            had_error = Some((
                *stop_reason,
                error_message.clone(),
                usage.clone(),
                *error_kind,
            ));
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
            delta: Cow::Owned(delta.clone()),
        }),
        AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta,
        } => Some(AssistantMessageDelta::Thinking {
            content_index: *content_index,
            delta: Cow::Owned(delta.clone()),
        }),
        AssistantMessageEvent::ToolCallDelta {
            content_index,
            delta,
        } => Some(AssistantMessageDelta::ToolCall {
            content_index: *content_index,
            delta: Cow::Owned(delta.clone()),
        }),
        _ => None,
    };

    if let Some(d) = delta
        && !emit(tx, AgentEvent::MessageUpdate { delta: d }).await
    {
        return Some(StreamResult::ChannelClosed);
    }
    None
}

/// Handle a stream error: classify it, check retryability, return action.
async fn handle_stream_error(
    model: &ModelSpec,
    config: &Arc<AgentLoopConfig>,
    stop_reason: &StopReason,
    error_message: &str,
    error_kind: Option<crate::stream::StreamErrorKind>,
    attempt: u32,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamErrorAction {
    let harness_error = classify_stream_error(error_message, *stop_reason, error_kind);

    // Context window overflow — signal and retry
    if matches!(harness_error, AgentError::ContextWindowOverflow { .. }) {
        warn!("context window overflow, signaling prune");
        return StreamErrorAction::ContextOverflow;
    }

    // Cache miss — reset cache state so next attempt re-sends with Write hint
    let mut retry_strategy_consulted = false;
    if matches!(harness_error, AgentError::CacheMiss) {
        warn!("provider cache miss, resetting cache state for retry");
        {
            let mut cache_state = config
                .cache_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache_state.reset();
        }
        retry_strategy_consulted = true;
        if config.retry_strategy.should_retry(&harness_error, attempt) {
            let delay = config.retry_strategy.delay(attempt);
            warn!(
                attempt,
                ?delay,
                error = %harness_error,
                "retrying after cache miss"
            );
            return StreamErrorAction::Retry(delay);
        }
    }

    // Aborted — preserve as StreamResult::Aborted so the turn emits
    // TurnEndReason::Aborted instead of TurnEndReason::Error (#438).
    if matches!(harness_error, AgentError::Aborted) {
        let abort_msg = build_abort_message(model);
        if !emit(tx, AgentEvent::MessageEnd { message: abort_msg }).await {
            return StreamErrorAction::ChannelClosed;
        }
        return StreamErrorAction::FatalError(StreamResult::Aborted);
    }

    // Check if retryable — RetryStrategy is the sole decision point
    if !retry_strategy_consulted && config.retry_strategy.should_retry(&harness_error, attempt) {
        let delay = config.retry_strategy.delay(attempt);
        warn!(attempt, ?delay, error = %harness_error, "retrying after transient error");
        return StreamErrorAction::Retry(delay);
    }

    // Non-retryable error
    error!(error = %harness_error, "non-retryable stream error");
    let error_msg = build_error_message(model, &harness_error);
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

/// Return a model spec with capability-based overrides applied.
///
/// When the model declares capabilities, this enforces them:
/// - If `supports_thinking` is false, `thinking_level` is forced to `Off`.
///
/// When no capabilities are set (manual `ModelSpec::new`), the spec is passed
/// through unchanged — the caller opted out of capability gating.
fn apply_capability_overrides(model: &ModelSpec) -> Cow<'_, ModelSpec> {
    let Some(ref caps) = model.capabilities else {
        return Cow::Borrowed(model);
    };

    let mut changed = false;
    let mut overridden = model.clone();

    if !caps.supports_thinking && overridden.thinking_level != ThinkingLevel::Off {
        debug!(
            model_id = %model.model_id,
            "model does not support thinking — forcing thinking_level to Off"
        );
        overridden.thinking_level = ThinkingLevel::Off;
        changed = true;
    }

    if changed {
        Cow::Owned(overridden)
    } else {
        Cow::Borrowed(model)
    }
}

/// Filter the tool list based on model capabilities.
///
/// When `supports_tool_use` is explicitly false, returns an empty list so the
/// provider is not offered any tool schemas. When capabilities are absent
/// (manual `ModelSpec`), tools pass through unchanged.
pub fn capability_filter_tools(
    model: &ModelSpec,
    tools: &[Arc<dyn crate::tool::AgentTool>],
) -> Vec<Arc<dyn crate::tool::AgentTool>> {
    if let Some(ref caps) = model.capabilities
        && !caps.supports_tool_use
        && !tools.is_empty()
    {
        debug!(
            model_id = %model.model_id,
            tool_count = tools.len(),
            "model does not support tool use — stripping tools from context"
        );
        return Vec::new();
    }
    tools.to_vec()
}

/// Accumulate collected stream events into a final message and emit `MessageEnd`.
async fn finalize_stream_message(
    model: &ModelSpec,
    events: Vec<AssistantMessageEvent>,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    let message = match accumulate_message(events, &model.provider, &model.model_id) {
        Ok(msg) => msg,
        Err(e) => {
            let err = AgentError::StreamError {
                source: Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            };
            let error_msg = build_error_message(model, &err);
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

    info!(
        input_tokens = message.usage.input,
        output_tokens = message.usage.output,
        total_tokens = message.usage.total,
        stop_reason = ?message.stop_reason,
        "stream completed"
    );

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
