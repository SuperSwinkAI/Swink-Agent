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
mod tests {
    use super::*;
    use crate::types::{BudgetConstraints, RecordedToolCall, TurnRecord};
    use std::time::Duration;
    use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

    fn make_invocation(turns: &[&[(&str, serde_json::Value)]]) -> Invocation {
        let turn_records: Vec<TurnRecord> = turns
            .iter()
            .enumerate()
            .map(|(i, calls)| {
                let tool_calls = calls
                    .iter()
                    .enumerate()
                    .map(|(j, (name, args))| RecordedToolCall {
                        id: format!("call_{i}_{j}"),
                        name: (*name).to_string(),
                        arguments: args.clone(),
                    })
                    .collect();
                TurnRecord {
                    turn_index: i,
                    assistant_message: AssistantMessage {
                        content: vec![],
                        provider: "test".to_string(),
                        model_id: "test-model".to_string(),
                        usage: Usage::default(),
                        cost: Cost::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp: 0,
                    },
                    tool_calls,
                    tool_results: vec![],
                    duration: Duration::from_millis(50),
                }
            })
            .collect();

        Invocation {
            turns: turn_records,
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: Duration::from_millis(100),
            final_response: None,
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("test", "test-model"),
        }
    }

    fn minimal_case() -> EvalCase {
        EvalCase {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: None,
            system_prompt: "test".to_string(),
            user_messages: vec!["test".to_string()],
            expected_trajectory: None,
            expected_response: None,
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn no_tool_calls_returns_none() {
        let eval = EfficiencyEvaluator::new();
        let invocation = make_invocation(&[&[]]);
        assert!(eval.evaluate(&minimal_case(), &invocation).is_none());
    }

    #[test]
    fn all_unique_perfect_score() {
        let eval = EfficiencyEvaluator::new();
        let invocation = make_invocation(&[&[
            ("read", serde_json::json!({"file": "a.rs"})),
            ("write", serde_json::json!({"file": "b.rs"})),
        ]]);
        // 1 turn, 2 unique calls out of 2 → dup_ratio=1.0
        // ideal = max(1, 2) = 2, actual = 1 → step_ratio = min(2,1)/1 = 1.0
        // composite = 0.6*1.0 + 0.4*1.0 = 1.0
        let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
        assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duplicate_calls_penalized() {
        let eval = EfficiencyEvaluator::new();
        let invocation = make_invocation(&[&[
            ("read", serde_json::json!({"file": "a.rs"})),
            ("read", serde_json::json!({"file": "a.rs"})),
            ("read", serde_json::json!({"file": "a.rs"})),
            ("write", serde_json::json!({"file": "b.rs"})),
        ]]);
        // 4 total, 2 unique → dup_ratio = 0.5
        // ideal = max(1, 2) = 2, actual = 1 → step_ratio = 1.0
        // composite = 0.6*0.5 + 0.4*1.0 = 0.7
        let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
        assert!((result.score.value - 0.7).abs() < 0.01);
    }

    #[test]
    fn step_ratio_uses_budget() {
        let eval = EfficiencyEvaluator::new();
        let invocation = make_invocation(&[
            &[("read", serde_json::json!({}))],
            &[("write", serde_json::json!({}))],
            &[("read", serde_json::json!({"file": "c.rs"}))],
            &[("write", serde_json::json!({"file": "d.rs"}))],
        ]);
        let mut case = minimal_case();
        case.budget = Some(BudgetConstraints {
            max_cost: None,
            max_tokens: None,
            max_turns: Some(2),
            max_duration: None,
        });
        // 4 unique / 4 total → dup_ratio = 1.0
        // ideal = budget.max_turns = 2, actual = 4 → step_ratio = 2/4 = 0.5
        // composite = 0.6*1.0 + 0.4*0.5 = 0.8
        let result = eval.evaluate(&case, &invocation).unwrap();
        assert!((result.score.value - 0.8).abs() < 0.01);
    }

    #[test]
    fn composite_weighted() {
        let eval = EfficiencyEvaluator::new();
        // 2 calls same args = 1 unique / 2 total → dup = 0.5
        // 2 turns, ideal = max(1,1) = 1, step = 1/2 = 0.5
        // composite = 0.6*0.5 + 0.4*0.5 = 0.5
        let invocation = make_invocation(&[
            &[("read", serde_json::json!({"file": "a.rs"}))],
            &[("read", serde_json::json!({"file": "a.rs"}))],
        ]);
        let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
        assert!((result.score.value - 0.5).abs() < 0.01);
    }
}
