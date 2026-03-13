//! Evaluator trait and registry for composing multiple evaluation metrics.

use std::sync::Arc;

use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Pluggable evaluator that scores an invocation against an eval case.
///
/// Implementations return `None` when the evaluator does not apply to the
/// given case (e.g., no expected trajectory is defined). This avoids forcing
/// every evaluator to produce a score for every case.
pub trait Evaluator: Send + Sync {
    /// Unique name for this evaluator, used in [`EvalCase::evaluators`] filters.
    fn name(&self) -> &'static str;

    /// Score the actual invocation against the expected case.
    ///
    /// Returns `None` if this evaluator is not applicable to the given case.
    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult>;
}

/// Blanket implementation for named closure pairs.
///
/// Allows quick one-off evaluators:
/// ```rust,ignore
/// let eval: Box<dyn Evaluator> = Box::new(("my_metric", |case, inv| { ... }));
/// ```
impl<F> Evaluator for (&'static str, F)
where
    F: Fn(&EvalCase, &Invocation) -> Option<EvalMetricResult> + Send + Sync,
{
    fn name(&self) -> &'static str {
        self.0
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        (self.1)(case, invocation)
    }
}

/// Registry of named evaluators, stored as `Arc<dyn Evaluator>`.
///
/// The registry runs all applicable evaluators for a case, optionally
/// filtered by the case's [`EvalCase::evaluators`] list.
pub struct EvaluatorRegistry {
    evaluators: Vec<Arc<dyn Evaluator>>,
}

impl EvaluatorRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            evaluators: Vec::new(),
        }
    }

    /// Create a registry pre-loaded with the built-in evaluators:
    /// [`TrajectoryMatcher`](crate::TrajectoryMatcher) (in-order),
    /// [`BudgetEvaluator`](crate::BudgetEvaluator),
    /// [`ResponseMatcher`](crate::ResponseMatcher), and
    /// [`EfficiencyEvaluator`](crate::EfficiencyEvaluator).
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(crate::match_::TrajectoryMatcher::in_order());
        registry.register(crate::budget::BudgetEvaluator);
        registry.register(crate::response::ResponseMatcher);
        registry.register(crate::efficiency::EfficiencyEvaluator::new());
        registry
    }

    /// Register a new evaluator.
    pub fn register(&mut self, evaluator: impl Evaluator + 'static) {
        self.evaluators.push(Arc::new(evaluator));
    }

    /// Run all applicable evaluators for a case.
    ///
    /// If `case.evaluators` is non-empty, only evaluators whose names appear
    /// in that list are run. Otherwise, all registered evaluators are run.
    #[must_use]
    pub fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Vec<EvalMetricResult> {
        let filter = &case.evaluators;
        self.evaluators
            .iter()
            .filter(|e| filter.is_empty() || filter.iter().any(|name| name == e.name()))
            .filter_map(|e| e.evaluate(case, invocation))
            .collect()
    }
}

impl Default for EvaluatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
