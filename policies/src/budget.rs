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
///     .with_pre_turn_policy(BudgetPolicy::new().with_max_cost(5.0));
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

    /// Set the maximum total cost, in USD.
    ///
    /// Cost is compared against the loop's accumulated cost. Most adapters do
    /// not price their own responses; the agent loop fills those in from the
    /// model catalog, so this ceiling is enforced for any model with catalog
    /// pricing. Models with no catalog pricing (unknown or local models) accrue
    /// zero cost and are therefore not constrained by this limit — use
    /// [`with_max_input`](Self::with_max_input) /
    /// [`with_max_output`](Self::with_max_output) to cap those.
    #[must_use]
    pub const fn with_max_cost(mut self, limit: f64) -> Self {
        self.max_cost = Some(limit);
        self
    }

    /// Set the maximum input tokens.
    #[must_use]
    pub const fn with_max_input(mut self, limit: u64) -> Self {
        self.max_input = Some(limit);
        self
    }

    /// Set the maximum output tokens.
    #[must_use]
    pub const fn with_max_output(mut self, limit: u64) -> Self {
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
#[path = "budget_tests.rs"]
mod tests;
