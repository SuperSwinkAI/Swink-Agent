//! Budget governance evaluator.
//!
//! Checks that an agent run stays within defined cost, token, and turn
//! constraints.

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Evaluator that checks cost, token, and turn budgets.
///
/// Returns `None` when the case has no `budget` constraints defined.
pub struct BudgetEvaluator;

impl Evaluator for BudgetEvaluator {
    fn name(&self) -> &'static str {
        "budget"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let budget = case.budget.as_ref()?;
        let mut violations = Vec::new();

        if let Some(max_cost) = budget.max_cost
            && invocation.total_cost.total > max_cost
        {
            violations.push(format!(
                "cost ${:.4} exceeded max ${:.4}",
                invocation.total_cost.total, max_cost
            ));
        }

        if let Some(max_input) = budget.max_input
            && invocation.total_usage.input > max_input
        {
            violations.push(format!(
                "input tokens {} exceeded max {}",
                invocation.total_usage.input, max_input
            ));
        }

        if let Some(max_output) = budget.max_output
            && invocation.total_usage.output > max_output
        {
            violations.push(format!(
                "output tokens {} exceeded max {}",
                invocation.total_usage.output, max_output
            ));
        }

        if let Some(max_turns) = budget.max_turns
            && invocation.turns.len() > max_turns
        {
            violations.push(format!(
                "turns {} exceeded max {}",
                invocation.turns.len(),
                max_turns
            ));
        }

        let score = if violations.is_empty() {
            Score::pass()
        } else {
            Score::fail()
        };

        let details = if violations.is_empty() {
            "all budget constraints satisfied".to_string()
        } else {
            violations.join("; ")
        };

        Some(EvalMetricResult {
            evaluator_name: "budget".to_string(),
            score,
            details: Some(details),
        })
    }
}
