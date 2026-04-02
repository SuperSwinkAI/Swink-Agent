//! Maximum turns policy — stops the loop after a configured number of turns.
#![forbid(unsafe_code)]

use swink_agent::{PolicyContext, PolicyVerdict, PostTurnPolicy, PreTurnPolicy, TurnPolicyContext};

/// Stops the agent loop after a configured number of turns.
///
/// Can be used as either a [`PreTurnPolicy`] (checked before each LLM call)
/// or a [`PostTurnPolicy`] (checked after each turn). The consumer chooses
/// which slot to place it in.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::MaxTurnsPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_pre_turn_policy(MaxTurnsPolicy::new(10));
/// ```
#[derive(Debug, Clone)]
pub struct MaxTurnsPolicy {
    max_turns: usize,
}

impl MaxTurnsPolicy {
    /// Create a new `MaxTurnsPolicy` with the given turn limit.
    pub const fn new(max_turns: usize) -> Self {
        Self { max_turns }
    }
}

impl PreTurnPolicy for MaxTurnsPolicy {
    fn name(&self) -> &'static str {
        "max_turns"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        if ctx.turn_index >= self.max_turns {
            PolicyVerdict::Stop(format!(
                "max turns reached: {} >= {}",
                ctx.turn_index, self.max_turns
            ))
        } else {
            PolicyVerdict::Continue
        }
    }
}

impl PostTurnPolicy for MaxTurnsPolicy {
    fn name(&self) -> &'static str {
        "max_turns"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        if ctx.turn_index >= self.max_turns {
            PolicyVerdict::Stop(format!(
                "max turns reached: {} >= {}",
                ctx.turn_index, self.max_turns
            ))
        } else {
            PolicyVerdict::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{Cost, Usage};

    fn make_ctx_at_turn<'a>(
        turn: usize,
        usage: &'a Usage,
        cost: &'a Cost,
        state: &'a swink_agent::SessionState,
    ) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: turn,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    #[test]
    fn stops_at_max() {
        let policy = MaxTurnsPolicy::new(5);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx_at_turn(5, &usage, &cost, &state);
        assert!(matches!(
            PreTurnPolicy::evaluate(&policy, &ctx),
            PolicyVerdict::Stop(_)
        ));
    }

    #[test]
    fn continues_below_max() {
        let policy = MaxTurnsPolicy::new(5);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx_at_turn(4, &usage, &cost, &state);
        assert!(matches!(
            PreTurnPolicy::evaluate(&policy, &ctx),
            PolicyVerdict::Continue
        ));
    }

    #[test]
    fn boundary_at_max() {
        let policy = MaxTurnsPolicy::new(3);
        let usage = Usage::default();
        let cost = Cost::default();

        // At turn 2 (0-indexed), still below max of 3
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx_at_turn(2, &usage, &cost, &state);
        assert!(matches!(
            PreTurnPolicy::evaluate(&policy, &ctx),
            PolicyVerdict::Continue
        ));

        // At turn 3, reaches max
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx_at_turn(3, &usage, &cost, &state);
        assert!(matches!(
            PreTurnPolicy::evaluate(&policy, &ctx),
            PolicyVerdict::Stop(_)
        ));
    }
}
