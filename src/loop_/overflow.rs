//! Emergency context overflow detection and recovery.
//!
//! When the LLM rejects a request with a context-window overflow, this module
//! re-runs context transformers with `overflow=true`, then retries the LLM call
//! with the compacted context. If recovery is not possible (no transformers, no
//! compaction, second overflow, or cancellation), it surfaces an error.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::types::{AgentMessage, LlmMessage, StopReason};

use super::stream::stream_with_retry;
use super::turn::{
    build_snapshot, emit_turn_end_and_agent_end, handle_cancellation, run_context_transformers,
};
use super::{
    AgentEvent, AgentLoopConfig, LoopState, StreamResult, TurnEndReason, TurnOutcome,
    build_error_message, emit,
};

/// Outcome of an in-place overflow recovery attempt.
pub(super) enum OverflowRecoveryResult {
    /// Recovery succeeded — the retry produced a new `StreamResult`.
    Recovered(Box<StreamResult>),
    /// Recovery failed — the turn should exit with this outcome.
    Failed(TurnOutcome),
}

/// Attempt emergency in-place overflow recovery.
///
/// When the LLM rejects a request with a context-window overflow, this function
/// re-runs context transformers with `overflow=true`, then retries the LLM call
/// with the compacted context. If recovery is not possible (no transformers, no
/// compaction, second overflow, or cancellation), it surfaces an error.
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_overflow_recovery(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    system_prompt: &str,
    agent_context: &crate::types::AgentContext,
    api_key: Option<String>,
    cancellation_token: &CancellationToken,
    tx: &mpsc::Sender<AgentEvent>,
) -> OverflowRecoveryResult {
    // Guard 1: Already attempted recovery this turn — surface error.
    if state.overflow_recovery_attempted {
        debug!("second overflow in same turn — surfacing error");
        return overflow_error(config, state, tx).await;
    }

    // Guard 2: No transformer configured — cannot compact.
    if config.async_transform_context.is_none() && config.transform_context.is_none() {
        debug!("no context transformer configured — cannot recover from overflow");
        return overflow_error(config, state, tx).await;
    }

    // Mark recovery as attempted for this turn.
    state.overflow_recovery_attempted = true;
    state.overflow_signal = true;

    // Re-run transformers with overflow=true
    let any_compacted =
        run_context_transformers(config, &mut state.context_messages, true, tx).await;
    state.overflow_signal = false;

    // Guard 3: Transformers ran but neither reported compaction — no point retrying.
    if !any_compacted {
        debug!("transformers ran but no compaction occurred — surfacing error");
        return overflow_error(config, state, tx).await;
    }

    // Check cancellation before retrying.
    if cancellation_token.is_cancelled() {
        return OverflowRecoveryResult::Failed(handle_cancellation(config, state, tx).await);
    }

    // Re-run convert-to-LLM pipeline with compacted context.
    let llm_messages: Vec<LlmMessage> = state
        .context_messages
        .iter()
        .filter_map(|m| (config.convert_to_llm)(m))
        .collect();

    // Retry the stream call with compacted context.
    let retry_result = stream_with_retry(
        config,
        agent_context,
        &llm_messages,
        system_prompt,
        api_key,
        cancellation_token,
        tx,
    )
    .await;

    // If the retry also overflows, surface the error — no further recovery.
    if matches!(retry_result, StreamResult::ContextOverflow) {
        debug!("retry after compaction still overflowed — surfacing error");
        return overflow_error(config, state, tx).await;
    }

    OverflowRecoveryResult::Recovered(Box::new(retry_result))
}

/// Build an overflow error message and emit `TurnEnd` + `AgentEnd`.
pub(super) async fn overflow_error(
    config: &Arc<AgentLoopConfig>,
    state: &mut LoopState,
    tx: &mpsc::Sender<AgentEvent>,
) -> OverflowRecoveryResult {
    let error = crate::error::AgentError::ContextWindowOverflow {
        model: config.model.model_id.clone(),
    };
    let error_msg = build_error_message(&config.model, &error);
    let msg_for_event = error_msg.clone();
    state
        .context_messages
        .push(AgentMessage::Llm(LlmMessage::Assistant(error_msg)));

    let _ = emit(
        tx,
        AgentEvent::MessageEnd {
            message: msg_for_event.clone(),
        },
    )
    .await;

    let snapshot = build_snapshot(state, StopReason::Error, None);
    OverflowRecoveryResult::Failed(
        emit_turn_end_and_agent_end(
            msg_for_event,
            vec![],
            TurnEndReason::Error,
            snapshot,
            state,
            tx,
        )
        .await,
    )
}
