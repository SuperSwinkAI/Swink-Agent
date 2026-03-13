//! Loop policy system for controlling agent loop continuation.
//!
//! Provides the [`LoopPolicy`] trait and built-in policies for limiting agent
//! loop execution by turn count, cost, or custom predicates. Policies are
//! composable via [`ComposedPolicy`].

use crate::types::{AssistantMessage, Cost, StopReason, Usage};

// ─── PolicyContext ──────────────────────────────────────────────────────────

/// Snapshot of loop state provided to policy decisions.
#[derive(Debug)]
pub struct PolicyContext<'a> {
    /// Zero-based index of the completed turn.
    pub turn_index: usize,
    /// Accumulated token usage across all turns so far.
    pub accumulated_usage: Usage,
    /// Accumulated cost across all turns so far.
    pub accumulated_cost: Cost,
    /// The assistant message from the just-completed turn.
    pub assistant_message: &'a AssistantMessage,
    /// The stop reason from the just-completed turn.
    pub stop_reason: StopReason,
}

// ─── LoopPolicy Trait ───────────────────────────────────────────────────────

/// Controls whether the agent loop continues after each turn.
///
/// Implement this trait to create custom stop conditions for the agent loop.
/// Return `true` from [`should_continue`](Self::should_continue) to allow the
/// loop to proceed, or `false` to terminate it.
pub trait LoopPolicy: Send + Sync {
    /// Decide whether the loop should continue after the current turn.
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool;
}

/// Blanket implementation so closures can be used directly as policies.
impl<F: Fn(&PolicyContext<'_>) -> bool + Send + Sync> LoopPolicy for F {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool {
        self(ctx)
    }
}

// ─── MaxTurnsPolicy ─────────────────────────────────────────────────────────

/// Limits the agent loop to a maximum number of turns.
#[derive(Debug, Clone)]
pub struct MaxTurnsPolicy {
    max_turns: usize,
}

impl MaxTurnsPolicy {
    /// Create a policy that stops after `max_turns` turns.
    #[must_use]
    pub const fn new(max_turns: usize) -> Self {
        Self { max_turns }
    }
}

impl LoopPolicy for MaxTurnsPolicy {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool {
        ctx.turn_index < self.max_turns
    }
}

// ─── CostCapPolicy ─────────────────────────────────────────────────────────

/// Limits the agent loop by total accumulated cost.
#[derive(Debug, Clone)]
pub struct CostCapPolicy {
    max_cost: f64,
}

impl CostCapPolicy {
    /// Create a policy that stops when accumulated cost exceeds `max_cost`.
    #[must_use]
    pub const fn new(max_cost: f64) -> Self {
        Self { max_cost }
    }
}

impl LoopPolicy for CostCapPolicy {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool {
        ctx.accumulated_cost.total <= self.max_cost
    }
}

// ─── ComposedPolicy ─────────────────────────────────────────────────────────

/// Composes multiple policies with AND semantics.
///
/// All inner policies must return `true` for the loop to continue.
pub struct ComposedPolicy {
    policies: Vec<Box<dyn LoopPolicy>>,
}

impl ComposedPolicy {
    /// Create a composed policy from a list of inner policies.
    #[must_use]
    pub fn new(policies: Vec<Box<dyn LoopPolicy>>) -> Self {
        Self { policies }
    }
}

impl LoopPolicy for ComposedPolicy {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool {
        self.policies.iter().all(|p| p.should_continue(ctx))
    }
}

impl std::fmt::Debug for ComposedPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposedPolicy")
            .field("policy_count", &self.policies.len())
            .finish_non_exhaustive()
    }
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MaxTurnsPolicy>();
    assert_send_sync::<CostCapPolicy>();
    assert_send_sync::<ComposedPolicy>();
    assert_send_sync::<PolicyContext<'_>>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AssistantMessage, Cost, StopReason, Usage};

    fn test_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    fn make_ctx(turn: usize, cost_total: f64) -> (AssistantMessage, PolicyContext<'static>) {
        // We need to return the message with the context referencing it.
        // Use a leaked box for test convenience.
        let msg = Box::leak(Box::new(test_message()));
        let cost = Cost {
            total: cost_total,
            ..Cost::default()
        };
        let ctx = PolicyContext {
            turn_index: turn,
            accumulated_usage: Usage::default(),
            accumulated_cost: cost,
            assistant_message: msg,
            stop_reason: StopReason::Stop,
        };
        // Return a dummy message (unused) and the context
        (test_message(), ctx)
    }

    #[test]
    fn max_turns_allows_under_limit() {
        let policy = MaxTurnsPolicy::new(3);
        let (_, ctx) = make_ctx(2, 0.0);
        assert!(policy.should_continue(&ctx));
    }

    #[test]
    fn max_turns_stops_at_limit() {
        let policy = MaxTurnsPolicy::new(3);
        let (_, ctx) = make_ctx(3, 0.0);
        assert!(!policy.should_continue(&ctx));
    }

    #[test]
    fn cost_cap_allows_under_budget() {
        let policy = CostCapPolicy::new(1.0);
        let (_, ctx) = make_ctx(0, 0.5);
        assert!(policy.should_continue(&ctx));
    }

    #[test]
    fn cost_cap_stops_over_budget() {
        let policy = CostCapPolicy::new(1.0);
        let (_, ctx) = make_ctx(0, 1.5);
        assert!(!policy.should_continue(&ctx));
    }

    #[test]
    fn composed_policy_and_semantics() {
        let policy = ComposedPolicy::new(vec![
            Box::new(MaxTurnsPolicy::new(5)),
            Box::new(CostCapPolicy::new(1.0)),
        ]);
        // Under both limits
        let (_, ctx) = make_ctx(2, 0.5);
        assert!(policy.should_continue(&ctx));

        // Over turn limit
        let (_, ctx) = make_ctx(5, 0.5);
        assert!(!policy.should_continue(&ctx));

        // Over cost limit
        let (_, ctx) = make_ctx(2, 1.5);
        assert!(!policy.should_continue(&ctx));
    }

    #[test]
    fn closure_as_policy() {
        let policy = |ctx: &PolicyContext<'_>| ctx.turn_index < 2;
        let (_, ctx) = make_ctx(1, 0.0);
        assert!(policy.should_continue(&ctx));
        let (_, ctx) = make_ctx(2, 0.0);
        assert!(!policy.should_continue(&ctx));
    }
}
