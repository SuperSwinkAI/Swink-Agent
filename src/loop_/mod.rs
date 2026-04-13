//! Core agent loop execution engine.
//!
//! Implements the nested inner/outer loop, tool dispatch, steering/follow-up
//! injection, event emission, retry integration, error/abort handling, and max
//! tokens recovery. Stateless — all state is passed in via [`AgentLoopConfig`].

mod config;
mod event;
mod overflow;
mod stream;
mod tool_dispatch;
mod turn;
mod types;

pub use config::AgentLoopConfig;
pub use event::{AgentEvent, TurnEndReason};
pub use types::*;

use std::error::Error as _;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info, info_span};

use crate::error::AgentError;
use crate::stream::StreamErrorKind;
use crate::types::{AgentMessage, AssistantMessage, ModelSpec, StopReason};
use crate::util::now_timestamp;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Sentinel value used to signal context overflow between `handle_stream_result`
/// and `run_single_turn`.
#[deprecated(
    note = "Overflow recovery now happens in-place in run_single_turn. Retained for backward compatibility."
)]
#[allow(dead_code)]
pub const CONTEXT_OVERFLOW_SENTINEL: &str = "__context_overflow__";

/// Channel capacity for agent events. Sized to handle burst streaming
/// without backpressure under normal operation.
const EVENT_CHANNEL_CAPACITY: usize = 256;

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
    let (tx, rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

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
pub async fn emit(tx: &mpsc::Sender<AgentEvent>, event: AgentEvent) -> bool {
    tx.send(event).await.is_ok()
}

// ─── run_loop_inner ──────────────────────────────────────────────────────────

/// The actual loop logic running inside the spawned task.
#[allow(clippy::too_many_lines)]
async fn run_loop_inner(
    initial_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
    tx: mpsc::Sender<AgentEvent>,
) {
    let config = Arc::new(config);
    let span = info_span!(
        "agent.run",
        model_id = %config.model.model_id,
        provider = %config.model.provider,
        tool_count = config.tools.len(),
        message_count = initial_messages.len(),
    );
    async {
        info!(
            model = %config.model.model_id,
            provider = %config.model.provider,
            tools = config.tools.len(),
            "starting agent loop"
        );
        // Build the transfer chain and push the current agent name (if known)
        // so that circular transfers back to this agent are detected.
        let mut transfer_chain = config.transfer_chain.clone().unwrap_or_default();
        if let Some(ref name) = config.agent_name {
            // Ignore the error — when resuming from a handoff chain the agent
            // name may already be present as the latest hop.
            let _ = transfer_chain.push(name.clone());
        }

        let mut state = LoopState {
            context_messages: initial_messages,
            pending_messages: Vec::new(),
            overflow_signal: false,
            overflow_recovery_attempted: false,
            turn_index: 0,
            accumulated_usage: crate::types::Usage::default(),
            accumulated_cost: crate::types::Cost::default(),
            last_assistant_message: None,
            last_tool_results: Vec::new(),
            transfer_chain,
        };

        // 1. Emit AgentStart
        if !emit(&tx, AgentEvent::AgentStart).await {
            return;
        }

        // 2. Outer loop (follow-up phase)
        'outer: loop {
            // Inner loop (turn + tool phase)
            'inner: loop {
                let turn_result = turn::run_single_turn(
                    &config,
                    &mut state,
                    &system_prompt,
                    &cancellation_token,
                    &tx,
                )
                .await;

                let should_break = match turn_result {
                    TurnOutcome::ContinueInner => {
                        state.turn_index += 1;
                        false
                    }
                    TurnOutcome::BreakInner => true,
                    TurnOutcome::Return => return,
                };

                // Post-turn policies are evaluated inside the turn handlers
                // (handle_no_tool_calls / handle_tool_calls) against the
                // committed turn snapshot before TurnEnd is emitted or transfer
                // termination is honored. This lets policies replace the
                // assistant message before listeners observe the turn.

                if should_break {
                    break 'inner;
                }
            }

            // Post-loop policies: evaluate after inner loop exits
            {
                use crate::policy::{PolicyContext, PolicyVerdict, run_post_loop_policies};

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
                    new_messages: &[], // no new messages at post-loop
                    state: &state_snapshot,
                };
                match run_post_loop_policies(&config.post_loop_policies, &policy_ctx) {
                    PolicyVerdict::Continue => {}
                    PolicyVerdict::Stop(_reason) => {
                        let _ = emit(
                            &tx,
                            AgentEvent::AgentEnd {
                                messages: Arc::new(state.context_messages),
                            },
                        )
                        .await;
                        info!("post-loop policy stopped agent");
                        return;
                    }
                    PolicyVerdict::Inject(msgs) => {
                        state.pending_messages.extend(msgs);
                        config
                            .pending_message_snapshot
                            .replace(&state.pending_messages);
                        continue 'outer;
                    }
                }
            }

            // Outer loop: poll follow-up messages
            if let Some(ref provider) = config.message_provider {
                let msgs = provider.poll_follow_up();
                if !msgs.is_empty() {
                    state.pending_messages.extend(msgs);
                    config
                        .pending_message_snapshot
                        .replace(&state.pending_messages);
                    continue 'outer;
                }
            }

            // No follow-up → emit AgentEnd and exit
            let _ = emit(
                &tx,
                AgentEvent::AgentEnd {
                    messages: Arc::new(state.context_messages),
                },
            )
            .await;
            info!("agent loop finished");
            return;
        }
    }
    .instrument(span)
    .await;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a terminal `AssistantMessage` with the given stop reason and message.
fn build_terminal_message(
    model: &ModelSpec,
    stop_reason: StopReason,
    error_message: String,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        provider: model.provider.clone(),
        model_id: model.model_id.clone(),
        usage: crate::types::Usage::default(),
        cost: crate::types::Cost::default(),
        stop_reason,
        error_message: Some(error_message),
        error_kind: None,
        timestamp: now_timestamp(),
        cache_hint: None,
    }
}

/// Build an aborted `AssistantMessage`.
pub fn build_abort_message(model: &ModelSpec) -> AssistantMessage {
    build_terminal_message(
        model,
        StopReason::Aborted,
        "operation aborted via cancellation token".to_string(),
    )
}

/// Build an error `AssistantMessage` from a `AgentError`.
pub fn build_error_message(model: &ModelSpec, error: &AgentError) -> AssistantMessage {
    build_terminal_message(model, StopReason::Error, format_error_with_sources(error))
}

pub fn format_error_with_sources(error: &AgentError) -> String {
    let mut message = error.to_string();
    let mut source = error.source();

    while let Some(err) = source {
        let source_message = err.to_string();
        if !source_message.is_empty() && !message.contains(&source_message) {
            message.push_str(": ");
            message.push_str(&source_message);
        }
        source = err.source();
    }

    message
}

/// Classify an `AssistantMessageEvent::Error` into a `AgentError`.
///
/// When `error_kind` is present, structural classification takes priority
/// over string matching on the error message.
pub fn classify_stream_error(
    error_message: &str,
    stop_reason: StopReason,
    error_kind: Option<StreamErrorKind>,
) -> AgentError {
    // Prefer structural classification when the adapter provides it
    if let Some(kind) = error_kind {
        return match kind {
            StreamErrorKind::Throttled => AgentError::ModelThrottled,
            StreamErrorKind::ContextWindowExceeded => AgentError::ContextWindowOverflow {
                model: String::new(),
            },
            StreamErrorKind::Auth => AgentError::StreamError {
                source: Box::new(std::io::Error::other(error_message.to_string())),
            },
            StreamErrorKind::Network => {
                AgentError::network(std::io::Error::other(error_message.to_string()))
            }
            StreamErrorKind::ContentFiltered => AgentError::ContentFiltered,
        };
    }

    // Fallback to string matching for adapters that don't set error_kind
    let lower = error_message.to_lowercase();
    if lower.contains("context window") || lower.contains("context_length_exceeded") {
        return AgentError::ContextWindowOverflow {
            model: String::new(),
        };
    }
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("throttl") {
        return AgentError::ModelThrottled;
    }
    if lower.contains("cache miss")
        || lower.contains("cache not found")
        || lower.contains("cache_miss")
    {
        return AgentError::CacheMiss;
    }
    if lower.contains("content filter") || lower.contains("content_filter") {
        return AgentError::ContentFiltered;
    }
    if stop_reason == StopReason::Aborted {
        return AgentError::Aborted;
    }
    AgentError::StreamError {
        source: Box::new(std::io::Error::other(error_message.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_cache_miss_variants() {
        let cases = [
            "cache miss",
            "Cache Miss detected",
            "provider cache_miss",
            "cache not found",
        ];
        for msg in cases {
            let err = classify_stream_error(msg, StopReason::Error, None);
            assert!(
                matches!(err, AgentError::CacheMiss),
                "expected CacheMiss for \"{msg}\", got {err:?}"
            );
            assert!(err.is_retryable());
        }
    }

    #[test]
    fn classify_non_cache_miss() {
        let err = classify_stream_error("internal server error", StopReason::Error, None);
        assert!(!matches!(err, AgentError::CacheMiss));
    }

    #[test]
    fn classify_content_filtered_by_kind() {
        let err = classify_stream_error(
            "response blocked",
            StopReason::Error,
            Some(StreamErrorKind::ContentFiltered),
        );
        assert!(matches!(err, AgentError::ContentFiltered));
        assert!(!err.is_retryable());
    }

    #[test]
    fn classify_content_filtered_by_string() {
        let err =
            classify_stream_error("content filter violation detected", StopReason::Error, None);
        assert!(matches!(err, AgentError::ContentFiltered));
        assert!(!err.is_retryable());
    }

    #[test]
    fn classify_throttled_by_kind() {
        let err = classify_stream_error(
            "some error",
            StopReason::Error,
            Some(StreamErrorKind::Throttled),
        );
        assert!(matches!(err, AgentError::ModelThrottled));
    }

    #[test]
    fn classify_network_by_kind() {
        let err = classify_stream_error(
            "connection reset",
            StopReason::Error,
            Some(StreamErrorKind::Network),
        );
        assert!(matches!(err, AgentError::NetworkError { .. }));
        assert!(err.is_retryable());
    }

    #[test]
    fn classify_auth_by_kind() {
        let err = classify_stream_error(
            "invalid api key",
            StopReason::Error,
            Some(StreamErrorKind::Auth),
        );
        assert!(matches!(err, AgentError::StreamError { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn classify_context_overflow_by_kind() {
        let err = classify_stream_error(
            "too many tokens",
            StopReason::Error,
            Some(StreamErrorKind::ContextWindowExceeded),
        );
        assert!(matches!(err, AgentError::ContextWindowOverflow { .. }));
    }

    #[test]
    fn structured_kind_takes_priority_over_string() {
        // Message says "rate limit" but kind says Network — kind wins
        let err = classify_stream_error(
            "rate limit exceeded",
            StopReason::Error,
            Some(StreamErrorKind::Network),
        );
        assert!(
            matches!(err, AgentError::NetworkError { .. }),
            "structured kind should override string matching, got {err:?}"
        );
    }

    #[test]
    fn string_fallback_for_unclassified_errors() {
        // No error_kind — string matching should still work for external adapters
        let err = classify_stream_error("rate limit (429)", StopReason::Error, None);
        assert!(matches!(err, AgentError::ModelThrottled));
    }

    #[test]
    fn string_fallback_context_overflow() {
        let err =
            classify_stream_error("context_length_exceeded: too long", StopReason::Error, None);
        assert!(matches!(err, AgentError::ContextWindowOverflow { .. }));
    }

    #[test]
    fn aborted_stop_reason_without_kind() {
        let err = classify_stream_error("operation cancelled", StopReason::Aborted, None);
        assert!(matches!(err, AgentError::Aborted));
    }
}
