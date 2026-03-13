//! Trajectory collection from agent event streams.
//!
//! [`TrajectoryCollector`] observes [`AgentEvent`]s and builds an
//! [`Invocation`] trace containing every turn, tool call, and timing metric.

use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use swink_agent::{AgentEvent, ContentBlock, Cost, ModelSpec, StopReason, Usage};

use crate::types::{Invocation, RecordedToolCall, TurnRecord};

/// In-progress builder for a single turn.
#[derive(Debug)]
struct TurnBuilder {
    turn_index: usize,
    tool_calls: Vec<RecordedToolCall>,
    start: Instant,
}

/// Builds an [`Invocation`] from a stream of [`AgentEvent`]s.
///
/// Two entry points:
/// - [`observe`](Self::observe) for incremental event processing (e.g., via subscription callback)
/// - [`collect_from_stream`](Self::collect_from_stream) for consuming an entire event stream
pub struct TrajectoryCollector {
    turns: Vec<TurnRecord>,
    current_turn: Option<TurnBuilder>,
    start_time: Option<Instant>,
    turn_counter: usize,
    model: Option<ModelSpec>,
    accumulated_usage: Usage,
    accumulated_cost: Cost,
    last_stop_reason: StopReason,
}

impl TrajectoryCollector {
    /// Create a new collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            turns: Vec::new(),
            current_turn: None,
            start_time: None,
            turn_counter: 0,
            model: None,
            accumulated_usage: Usage::default(),
            accumulated_cost: Cost::default(),
            last_stop_reason: StopReason::Stop,
        }
    }

    /// Process a single event. Call this for each event from the agent loop stream.
    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.start_time = Some(Instant::now());
            }
            AgentEvent::BeforeLlmCall { model, .. } => {
                if self.model.is_none() {
                    self.model = Some(model.clone());
                }
            }
            AgentEvent::TurnStart => {
                let idx = self.turn_counter;
                self.turn_counter += 1;
                self.current_turn = Some(TurnBuilder {
                    turn_index: idx,
                    tool_calls: Vec::new(),
                    start: Instant::now(),
                });
            }
            AgentEvent::ToolExecutionStart {
                id,
                name,
                arguments,
            } => {
                if let Some(builder) = &mut self.current_turn {
                    builder.tool_calls.push(RecordedToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
            }
            AgentEvent::TurnEnd {
                assistant_message,
                tool_results,
                ..
            } => {
                if let Some(builder) = self.current_turn.take() {
                    let duration = builder.start.elapsed();
                    self.accumulated_usage += assistant_message.usage.clone();
                    self.accumulated_cost += assistant_message.cost.clone();
                    self.last_stop_reason = assistant_message.stop_reason;

                    self.turns.push(TurnRecord {
                        turn_index: builder.turn_index,
                        assistant_message: assistant_message.clone(),
                        tool_calls: builder.tool_calls,
                        tool_results: tool_results.clone(),
                        duration,
                    });
                }
            }
            // Other events are observed but not recorded.
            _ => {}
        }
    }

    /// Finalize and return the completed [`Invocation`].
    #[must_use]
    pub fn finish(self) -> Invocation {
        let total_duration = self
            .start_time
            .map_or(Duration::ZERO, |start| start.elapsed());

        let final_response = self
            .turns
            .last()
            .map(|turn| ContentBlock::extract_text(&turn.assistant_message.content))
            .filter(|s| !s.is_empty());

        Invocation {
            turns: self.turns,
            total_usage: self.accumulated_usage,
            total_cost: self.accumulated_cost,
            total_duration,
            final_response,
            stop_reason: self.last_stop_reason,
            model: self
                .model
                .unwrap_or_else(|| ModelSpec::new("unknown", "unknown")),
        }
    }

    /// Convenience: collect from an entire event stream.
    pub async fn collect_from_stream(stream: impl Stream<Item = AgentEvent>) -> Invocation {
        let mut collector = Self::new();
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            collector.observe(&event);
        }
        collector.finish()
    }
}

impl Default for TrajectoryCollector {
    fn default() -> Self {
        Self::new()
    }
}
