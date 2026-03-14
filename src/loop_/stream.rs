use std::borrow::Cow;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::error::AgentError;
use crate::stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamOptions, accumulate_message,
};
use crate::types::{AgentContext, AgentMessage, LlmMessage, StopReason};

use super::{
    AgentEvent, AgentLoopConfig, StreamResult, build_abort_message, build_error_message,
    classify_stream_error, emit,
};

/// Stream an assistant response with retry logic, emitting message events.
pub async fn stream_with_retry(
    config: &Arc<AgentLoopConfig>,
    agent_context: &AgentContext,
    llm_messages: &[LlmMessage],
    system_prompt: &str,
    api_key: Option<String>,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamResult {
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        debug!(attempt, model_id = %config.model.model_id, "starting stream attempt");

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
            config,
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
    stream_options: &StreamOptions,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamAttemptResult {
    let mut stream = config.stream_fn.stream(
        &config.model,
        call_context,
        stream_options,
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
            ..
        } = &event
        {
            had_error = Some((*stop_reason, error_message.clone(), usage.clone()));
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
    config: &Arc<AgentLoopConfig>,
    stop_reason: &StopReason,
    error_message: &str,
    attempt: u32,
    tx: &mpsc::Sender<AgentEvent>,
) -> StreamErrorAction {
    let harness_error = classify_stream_error(error_message, *stop_reason);

    // Context window overflow — signal and retry
    if matches!(harness_error, AgentError::ContextWindowOverflow { .. }) {
        warn!("context window overflow, signaling prune");
        let _ = emit(
            tx,
            AgentEvent::MessageEnd {
                message: build_error_message(&config.model, &harness_error),
            },
        )
        .await;
        return StreamErrorAction::ContextOverflow;
    }

    // Check if retryable — RetryStrategy is the sole decision point
    if config.retry_strategy.should_retry(&harness_error, attempt) {
        let delay = config.retry_strategy.delay(attempt);
        warn!(attempt, ?delay, error = %harness_error, "retrying after transient error");
        return StreamErrorAction::Retry(delay);
    }

    // Non-retryable error
    error!(error = %harness_error, "non-retryable stream error");
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
            let err = AgentError::StreamError {
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
