//! Budget governance evaluator.
//!
//! Checks that an agent run stays within defined cost, token, turn,
//! and duration constraints.

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Evaluator that checks cost, token, turn, and duration budgets.
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

        if let Some(max_tokens) = budget.max_tokens
            && invocation.total_usage.total > max_tokens
        {
            violations.push(format!(
                "tokens {} exceeded max {}",
                invocation.total_usage.total, max_tokens
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

        if let Some(max_duration) = budget.max_duration
            && invocation.total_duration > max_duration
        {
            violations.push(format!(
                "duration {:?} exceeded max {:?}",
                invocation.total_duration, max_duration
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
