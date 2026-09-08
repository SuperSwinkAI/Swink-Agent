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
#[path = "max_turns_tests.rs"]
mod tests;
