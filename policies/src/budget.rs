//! Budget enforcement policy — stops the loop when cost or token limits are exceeded.
#![forbid(unsafe_code)]

use swink_agent::{PolicyContext, PolicyVerdict, PreTurnPolicy};

/// Stops the agent loop when accumulated cost or tokens exceed configured limits.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::BudgetPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_pre_turn_policy(BudgetPolicy::new().max_cost(5.0));
/// ```
#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)]
pub struct BudgetPolicy {
    max_cost: Option<f64>,
    max_input: Option<u64>,
    max_output: Option<u64>,
}

impl BudgetPolicy {
    /// Create a new `BudgetPolicy` with no limits.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_cost: None,
            max_input: None,
            max_output: None,
        }
    }

    /// Set the maximum total cost.
    #[must_use]
    pub const fn max_cost(mut self, limit: f64) -> Self {
        self.max_cost = Some(limit);
        self
    }

    /// Set the maximum input tokens.
    #[must_use]
    pub const fn max_input(mut self, limit: u64) -> Self {
        self.max_input = Some(limit);
        self
    }

    /// Set the maximum output tokens.
    #[must_use]
    pub const fn max_output(mut self, limit: u64) -> Self {
        self.max_output = Some(limit);
        self
    }
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl PreTurnPolicy for BudgetPolicy {
    fn name(&self) -> &'static str {
        "budget"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        if let Some(max_cost) = self.max_cost
            && ctx.accumulated_cost.total >= max_cost
        {
            return PolicyVerdict::Stop(format!(
                "budget exceeded: cost {:.4} >= limit {:.4}",
                ctx.accumulated_cost.total, max_cost
            ));
        }

        if let Some(max_input) = self.max_input
            && ctx.accumulated_usage.input >= max_input
        {
            return PolicyVerdict::Stop(format!(
                "budget exceeded: input tokens {} >= limit {}",
                ctx.accumulated_usage.input, max_input
            ));
        }

        if let Some(max_output) = self.max_output
            && ctx.accumulated_usage.output >= max_output
        {
            return PolicyVerdict::Stop(format!(
                "budget exceeded: output tokens {} >= limit {}",
                ctx.accumulated_usage.output, max_output
            ));
        }

        PolicyVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{Cost, Usage};

    fn make_ctx<'a>(usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
        }
    }

    #[test]
    fn name_returns_budget() {
        assert_eq!(BudgetPolicy::new().name(), "budget");
    }

    #[test]
    fn no_limits_returns_continue() {
        let policy = BudgetPolicy::new();
        let usage = Usage { input: 1000, output: 500, ..Default::default() };
        let cost = Cost { total: 10.0, ..Default::default() };
        let ctx = make_ctx(&usage, &cost);
        assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Continue));
    }

    #[test]
    fn cost_exceeded_returns_stop() {
        let policy = BudgetPolicy::new().max_cost(1.0);
        let usage = Usage::default();
        let cost = Cost { total: 1.5, ..Default::default() };
        let ctx = make_ctx(&usage, &cost);
        assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
    }

    #[test]
    fn cost_not_exceeded_returns_continue() {
        let policy = BudgetPolicy::new().max_cost(5.0);
        let usage = Usage::default();
        let cost = Cost { total: 4.99, ..Default::default() };
        let ctx = make_ctx(&usage, &cost);
        assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Continue));
    }

    #[test]
    fn token_exceeded_returns_stop() {
        let policy = BudgetPolicy::new().max_input(100);
        let usage = Usage { input: 150, ..Default::default() };
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
    }

    #[test]
    fn boundary_value_at_limit() {
        let policy = BudgetPolicy::new().max_cost(1.0);
        let usage = Usage::default();
        let cost = Cost { total: 1.0, ..Default::default() };
        let ctx = make_ctx(&usage, &cost);
        // At exactly the limit, should stop (>= comparison)
        assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
    }
}
