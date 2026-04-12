//! Trajectory collection from agent event streams.
//!
//! [`TrajectoryCollector`] observes [`AgentEvent`]s and builds an
//! [`Invocation`] trace containing every turn, tool call, and timing metric.

use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use swink_agent::{AgentEvent, ContentBlock, Cost, ModelSpec, StopReason, Usage};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::types::{EvalCase, Invocation, RecordedToolCall, TurnRecord};

/// Sleep until the given deadline. Used in `tokio::select!` to enforce
/// `max_duration` proactively.
async fn sleep_until_deadline(deadline: tokio::time::Instant) {
    tokio::time::sleep_until(deadline).await;
}

/// Real-time budget guard that cancels an agent run when cost, token, turn,
/// or duration thresholds are exceeded mid-execution.
pub struct BudgetGuard {
    cancel: CancellationToken,
    max_cost: Option<f64>,
    max_tokens: Option<u64>,
    max_turns: Option<usize>,
    max_duration: Option<Duration>,
    start_time: Instant,
}

impl BudgetGuard {
    /// Create a guard with the given cancellation token and no thresholds.
    #[must_use]
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            max_cost: None,
            max_tokens: None,
            max_turns: None,
            max_duration: None,
            start_time: Instant::now(),
        }
    }

    /// Set a maximum cost threshold.
    #[must_use]
    pub const fn with_max_cost(mut self, max_cost: f64) -> Self {
        self.max_cost = Some(max_cost);
        self
    }

    /// Set a maximum token threshold.
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set a maximum turn threshold.
    #[must_use]
    pub const fn with_max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Set a maximum wall-clock duration threshold.
    #[must_use]
    pub const fn with_max_duration(mut self, max_duration: Duration) -> Self {
        self.max_duration = Some(max_duration);
        self
    }

    /// Build a guard from an eval case's budget constraints, if any thresholds
    /// are defined.
    #[must_use]
    pub fn from_case(case: &EvalCase, cancel: CancellationToken) -> Option<Self> {
        let budget = case.budget.as_ref()?;
        if budget.max_cost.is_none()
            && budget.max_tokens.is_none()
            && budget.max_turns.is_none()
            && budget.max_duration.is_none()
        {
            return None;
        }
        let mut guard = Self::new(cancel);
        guard.max_cost = budget.max_cost;
        guard.max_tokens = budget.max_tokens;
        guard.max_turns = budget.max_turns;
        guard.max_duration = budget.max_duration;
        Some(guard)
    }
}

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

    /// Check whether accumulated metrics exceed any guard threshold.
    fn exceeds_budget(&self, guard: &BudgetGuard) -> bool {
        if let Some(max_cost) = guard.max_cost
            && self.accumulated_cost.total > max_cost
        {
            return true;
        }
        if let Some(max_tokens) = guard.max_tokens
            && self.accumulated_usage.total > max_tokens
        {
            return true;
        }
        if let Some(max_turns) = guard.max_turns
            && self.turn_counter > max_turns
        {
            return true;
        }
        if let Some(max_duration) = guard.max_duration
            && guard.start_time.elapsed() >= max_duration
        {
            return true;
        }
        false
    }

    /// Collect from a stream with an optional budget guard.
    ///
    /// After each event, checks whether accumulated cost, token, and turn
    /// thresholds are exceeded. Duration is enforced proactively via a
    /// deadline: if `max_duration` is set, the guard races a timeout against
    /// the event stream so the run is cancelled even when no events arrive
    /// (e.g., the LLM or a tool call is blocking).
    pub async fn collect_with_guard(
        stream: impl Stream<Item = AgentEvent>,
        guard: Option<BudgetGuard>,
    ) -> Invocation {
        let mut collector = Self::new();
        futures::pin_mut!(stream);
        let mut cancelled = false;

        // Compute a tokio deadline from max_duration, if configured.
        let has_deadline = guard
            .as_ref()
            .and_then(|g| g.max_duration)
            .is_some();
        let deadline = guard.as_ref().and_then(|g| {
            g.max_duration.map(|d| {
                let elapsed = g.start_time.elapsed();
                tokio::time::Instant::now() + d.saturating_sub(elapsed)
            })
        });

        loop {
            let event = if has_deadline && !cancelled {
                // SAFETY: `deadline` is `Some` when `has_deadline` is true.
                let dl = deadline.unwrap();
                tokio::select! {
                    biased;
                    () = sleep_until_deadline(dl) => {
                        // Deadline fired before next event.
                        if let Some(ref g) = guard {
                            warn!(
                                cost = collector.accumulated_cost.total,
                                tokens = collector.accumulated_usage.total,
                                turns = collector.turn_counter,
                                elapsed_ms = u64::try_from(g.start_time.elapsed().as_millis()).unwrap_or(u64::MAX),
                                "budget guard triggered (deadline) — cancelling agent run"
                            );
                            g.cancel.cancel();
                        }
                        cancelled = true;
                        // Continue draining the stream so the trace is complete.
                        stream.next().await
                    }
                    next = stream.next() => next,
                }
            } else {
                stream.next().await
            };

            let Some(event) = event else {
                break;
            };

            collector.observe(&event);

            // Check non-duration thresholds after each event.
            if let Some(ref g) = guard
                && !cancelled
                && collector.exceeds_budget(g)
            {
                warn!(
                    cost = collector.accumulated_cost.total,
                    tokens = collector.accumulated_usage.total,
                    turns = collector.turn_counter,
                    elapsed_ms = u64::try_from(g.start_time.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "budget guard triggered — cancelling agent run"
                );
                g.cancel.cancel();
                cancelled = true;
            }
        }
        collector.finish()
    }
}

impl Default for TrajectoryCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_collector(cost: f64, tokens: u64, turns: usize) -> TrajectoryCollector {
        let mut c = TrajectoryCollector::new();
        c.accumulated_cost = Cost {
            total: cost,
            ..Default::default()
        };
        c.accumulated_usage = Usage {
            total: tokens,
            ..Default::default()
        };
        c.turn_counter = turns;
        c
    }

    #[test]
    fn exceeds_cost() {
        let c = make_collector(5.0, 100, 2);
        let guard = BudgetGuard::new(CancellationToken::new()).with_max_cost(3.0);
        assert!(c.exceeds_budget(&guard));
    }

    #[test]
    fn exceeds_tokens() {
        let c = make_collector(1.0, 5000, 2);
        let guard = BudgetGuard::new(CancellationToken::new()).with_max_tokens(1000);
        assert!(c.exceeds_budget(&guard));
    }

    #[test]
    fn exceeds_turns() {
        let c = make_collector(1.0, 100, 10);
        let guard = BudgetGuard::new(CancellationToken::new()).with_max_turns(5);
        assert!(c.exceeds_budget(&guard));
    }

    #[test]
    fn within_budget() {
        let c = make_collector(1.0, 100, 2);
        let guard = BudgetGuard::new(CancellationToken::new())
            .with_max_cost(5.0)
            .with_max_tokens(1000)
            .with_max_turns(10);
        assert!(!c.exceeds_budget(&guard));
    }

    #[test]
    fn exceeds_duration() {
        let c = make_collector(1.0, 100, 2);
        // Create a guard with a zero-duration limit — already exceeded.
        let guard =
            BudgetGuard::new(CancellationToken::new()).with_max_duration(Duration::ZERO);
        assert!(c.exceeds_budget(&guard));
    }

    #[test]
    fn within_duration() {
        let c = make_collector(1.0, 100, 2);
        // 1 hour should be well within bounds for this test.
        let guard = BudgetGuard::new(CancellationToken::new())
            .with_max_duration(Duration::from_secs(3600));
        assert!(!c.exceeds_budget(&guard));
    }

    fn make_eval_case(budget: Option<crate::types::BudgetConstraints>) -> EvalCase {
        EvalCase {
            id: "test".to_string(),
            name: "test".to_string(),
            description: None,
            system_prompt: String::new(),
            user_messages: vec!["hi".to_string()],
            expected_trajectory: None,
            expected_response: None,
            evaluators: vec![],
            budget,
            metadata: Default::default(),
        }
    }

    #[test]
    fn from_case_includes_duration() {
        use crate::types::BudgetConstraints;

        let case = make_eval_case(Some(BudgetConstraints {
            max_cost: None,
            max_tokens: None,
            max_turns: None,
            max_duration: Some(Duration::from_secs(30)),
        }));
        let guard = BudgetGuard::from_case(&case, CancellationToken::new());
        assert!(guard.is_some());
        let g = guard.unwrap();
        assert_eq!(g.max_duration, Some(Duration::from_secs(30)));
    }

    #[test]
    fn from_case_none_when_all_empty() {
        use crate::types::BudgetConstraints;

        let case = make_eval_case(Some(BudgetConstraints {
            max_cost: None,
            max_tokens: None,
            max_turns: None,
            max_duration: None,
        }));
        let guard = BudgetGuard::from_case(&case, CancellationToken::new());
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn deadline_cancels_token_proactively() {
        // Simulate a stream that emits one event, then delays longer than the
        // deadline before the second event. The deadline should fire and cancel
        // the token even though the stream hasn't yielded the second event yet.
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let guard = BudgetGuard::new(cancel).with_max_duration(Duration::from_millis(50));

        let stream = futures::stream::unfold(0u8, |state| async move {
            match state {
                0 => Some((AgentEvent::AgentStart, 1)),
                1 => {
                    // Delay longer than the guard's deadline.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Some((
                        AgentEvent::AgentEnd {
                            messages: std::sync::Arc::new(vec![]),
                        },
                        2,
                    ))
                }
                _ => None,
            }
        });

        let _invocation = TrajectoryCollector::collect_with_guard(stream, Some(guard)).await;

        // The token must have been cancelled by the proactive deadline.
        assert!(
            cancel_clone.is_cancelled(),
            "cancellation token should be cancelled by deadline"
        );
    }

    #[tokio::test]
    async fn no_deadline_does_not_cancel() {
        // Without a duration limit, the token should NOT be cancelled.
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let guard = BudgetGuard::new(cancel)
            .with_max_cost(100.0)
            .with_max_tokens(100_000);

        let stream = futures::stream::iter(vec![
            AgentEvent::AgentStart,
            AgentEvent::AgentEnd {
                messages: std::sync::Arc::new(vec![]),
            },
        ]);

        let _invocation = TrajectoryCollector::collect_with_guard(stream, Some(guard)).await;

        assert!(
            !cancel_clone.is_cancelled(),
            "cancellation token should not be cancelled when within budget"
        );
    }
}
