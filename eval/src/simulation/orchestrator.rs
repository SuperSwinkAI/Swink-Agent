//! Multi-turn simulation orchestrator (US4, FR-026).
//!
//! Drives an [`Agent`] ↔ [`ActorSimulator`] dialogue, optionally routing
//! emitted tool calls through a [`ToolSimulator`]. Returns a fully-populated
//! [`Invocation`] plus a [`SimulationOutcome`]. Cancellation is honored
//! cooperatively at every `await` point.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use futures::StreamExt;
use swink_agent::{
    Agent, AgentEvent, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, ToolResultMessage,
    Usage, UserMessage, now_timestamp,
};
use tokio_util::sync::CancellationToken;

use super::actor::{ActorSimulator, ActorTurn};
use super::tool::{ToolSimulationError, ToolSimulator};
use crate::judge::JudgeError;
use crate::trajectory::TrajectoryCollector;
use crate::types::{Invocation, RecordedToolCall};

/// Outcome classification emitted alongside the [`Invocation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationOutcome {
    GoalCompleted,
    MaxTurnsReached,
    AgentStopped,
}

/// Errors surfaced by [`run_multiturn_simulation`].
#[derive(Debug, thiserror::Error)]
pub enum SimulationError {
    #[error("actor error: {0}")]
    Actor(#[source] JudgeError),
    #[error("tool error: {0}")]
    Tool(#[source] ToolSimulationError),
    #[error("simulation cancelled")]
    Cancelled,
    #[error("agent error: {0}")]
    Agent(String),
    /// Tool response body failed JSON-schema validation (FR-025).
    #[error("schema validation failed: {0}")]
    SchemaValidation(String),
}

impl From<ToolSimulationError> for SimulationError {
    fn from(err: ToolSimulationError) -> Self {
        match err {
            ToolSimulationError::SchemaValidation(reason) => Self::SchemaValidation(reason),
            other => Self::Tool(other),
        }
    }
}

/// Orchestrate a multi-turn dialogue between `agent` and `actor`.
#[allow(clippy::too_many_lines)]
pub async fn run_multiturn_simulation(
    agent: &mut Agent,
    actor: &ActorSimulator,
    tool_sim: Option<&ToolSimulator>,
    max_turns: u32,
    cancel: CancellationToken,
) -> Result<(Invocation, SimulationOutcome), SimulationError> {
    let overall_start = Instant::now();
    let mut outcome = SimulationOutcome::AgentStopped;
    let mut collector = TrajectoryCollector::new();
    let mut next_user: ActorTurn = actor.greeting();
    let mut turn_count: u32 = 0;

    while turn_count < max_turns {
        if cancel.is_cancelled() {
            return Err(SimulationError::Cancelled);
        }
        if next_user.goal_completed.is_some() {
            outcome = SimulationOutcome::GoalCompleted;
            break;
        }

        let conversation = vec![swink_agent::AgentMessage::Llm(LlmMessage::User(
            UserMessage {
                content: vec![ContentBlock::Text {
                    text: next_user.message.clone(),
                }],
                timestamp: now_timestamp(),
                cache_hint: None,
            },
        ))];
        let stream = agent
            .prompt_stream(conversation)
            .map_err(|err| SimulationError::Agent(err.to_string()))?;
        tokio::pin!(stream);

        let mut pending_tool_calls: Vec<RecordedToolCall> = Vec::new();
        let mut last_assistant_text = String::new();

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(SimulationError::Cancelled),
                next = stream.next() => match next {
                    None => break,
                    Some(event) => {
                        if let AgentEvent::TurnEnd { assistant_message, .. } = &event {
                            last_assistant_text =
                                ContentBlock::extract_text(&assistant_message.content);
                            for block in &assistant_message.content {
                                if let ContentBlock::ToolCall {
                                    id, name, arguments, ..
                                } = block
                                {
                                    pending_tool_calls.push(RecordedToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        arguments: arguments.clone(),
                                    });
                                }
                            }
                        }
                        collector.observe(&event);
                    }
                },
            }
        }

        // Optionally attach simulated tool results to the most recent turn.
        if let (Some(sim), false) = (tool_sim, pending_tool_calls.is_empty()) {
            let last_idx = collector.turns_len_hint().checked_sub(1);
            for call in std::mem::take(&mut pending_tool_calls) {
                let value = sim
                    .invoke(&call.name, &call.arguments, &call.id)
                    .await
                    .map_err(SimulationError::from)?;
                if let Some(idx) = last_idx {
                    collector.append_tool_result(
                        idx,
                        ToolResultMessage {
                            tool_call_id: call.id.clone(),
                            content: vec![ContentBlock::Text {
                                text: value.to_string(),
                            }],
                            is_error: false,
                            timestamp: now_timestamp(),
                            details: serde_json::Value::Null,
                            cache_hint: None,
                        },
                    );
                }
            }
        }

        turn_count += 1;
        if turn_count >= max_turns {
            outcome = SimulationOutcome::MaxTurnsReached;
            break;
        }

        let assistant_text = if last_assistant_text.is_empty() {
            "…".to_string()
        } else {
            last_assistant_text
        };
        let produced = actor
            .next_turn(&assistant_text)
            .await
            .map_err(SimulationError::Actor)?;
        if produced.goal_completed.is_some() {
            outcome = SimulationOutcome::GoalCompleted;
            break;
        }
        next_user = produced;
    }

    let mut invocation = collector.finish();
    if invocation.total_duration == Duration::ZERO {
        invocation.total_duration = overall_start.elapsed();
    }
    if invocation.model == ModelSpec::new("unknown", "unknown") {
        invocation.model = ModelSpec::new("simulated", actor.model_id());
    }
    if invocation.turns.is_empty() {
        invocation.total_usage = Usage::default();
        invocation.total_cost = Cost::default();
        invocation.stop_reason = StopReason::Stop;
    }

    Ok((invocation, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_error_wraps_schema_variant() {
        let err: SimulationError = ToolSimulationError::SchemaValidation("boom".into()).into();
        assert!(matches!(err, SimulationError::SchemaValidation(_)));
    }
}
