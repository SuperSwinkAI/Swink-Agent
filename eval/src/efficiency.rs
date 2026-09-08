//! Trajectory efficiency evaluator.
//!
//! Scores how efficiently an agent used its tools by measuring duplicate
//! tool calls and turn count relative to an ideal.

use std::collections::HashSet;

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Evaluator that scores trajectory efficiency based on duplicate tool calls
/// and step count relative to an ideal.
///
/// **Scoring algorithm:**
/// - Duplicate ratio (weight 0.6): `unique_calls / total_calls`
/// - Step ratio (weight 0.4): `min(ideal, actual) / actual`
/// - Composite: `0.6 * duplicate_ratio + 0.4 * step_ratio`
///
/// Returns `None` when total tool calls across all turns is zero.
pub struct EfficiencyEvaluator {
    threshold: f64,
}

impl EfficiencyEvaluator {
    /// Create a new evaluator with default threshold of 0.5.
    #[must_use]
    pub const fn new() -> Self {
        Self { threshold: 0.5 }
    }

    /// Set a custom pass/fail threshold.
    #[must_use]
    pub const fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }
}

impl Default for EfficiencyEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl Evaluator for EfficiencyEvaluator {
    fn name(&self) -> &'static str {
        "efficiency"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // Flatten all tool calls across turns.
        let all_calls: Vec<_> = invocation
            .turns
            .iter()
            .flat_map(|t| &t.tool_calls)
            .collect();

        let total = all_calls.len();
        if total == 0 {
            return None;
        }

        // Duplicate ratio: unique / total.
        let unique_keys: HashSet<_> = all_calls
            .iter()
            .map(|tc| {
                let args_str = serde_json::to_string(&tc.arguments).unwrap_or_default();
                (tc.name.clone(), args_str)
            })
            .collect();
        let unique = unique_keys.len();
        #[allow(clippy::cast_precision_loss)]
        let duplicate_ratio = unique as f64 / total as f64;

        // Step ratio: ideal / actual turns.
        let actual_turns = invocation.turns.len();
        let ideal = case
            .budget
            .as_ref()
            .and_then(|b| b.max_turns)
            .unwrap_or_else(|| unique_keys.len().max(1));
        #[allow(clippy::cast_precision_loss)]
        let step_ratio = (ideal.min(actual_turns) as f64 / actual_turns as f64).clamp(0.0, 1.0);

        // Composite.
        let composite = 0.6f64.mul_add(duplicate_ratio, 0.4 * step_ratio);

        let details = format!(
            "duplicate ratio: {duplicate_ratio:.2} ({unique}/{total} unique), \
             step ratio: {step_ratio:.2} ({}/{actual_turns} turns efficient), \
             composite: {composite:.2}",
            ideal.min(actual_turns),
        );

        Some(EvalMetricResult {
            evaluator_name: "efficiency".to_string(),
            score: Score::new(composite, self.threshold),
            details: Some(details),
        })
    }
}

#[cfg(test)]
#[path = "efficiency_tests.rs"]
mod tests;
