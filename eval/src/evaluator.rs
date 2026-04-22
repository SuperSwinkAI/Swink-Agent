//! Evaluator trait and registry for composing multiple evaluation metrics.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::judge::JudgeClient;
use crate::score::Score;
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
    /// Judge client available to semantic evaluators.
    ///
    /// Stored here so Phase 9 (`SemanticToolSelectionEvaluator`) and Phase 10
    /// (`SemanticToolParameterEvaluator`) can wire themselves into the
    /// `with_judge` / `with_defaults_and_judge` constructors when they land.
    /// Today the field is read only by downstream phases; semantic evaluators
    /// that consume it are NOT yet implemented.
    judge: Option<Arc<dyn JudgeClient>>,
}

impl EvaluatorRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            evaluators: Vec::new(),
            judge: None,
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
        registry.register(crate::environment_state::EnvironmentStateEvaluator);
        registry
    }

    /// Create a registry wired with a judge client but no other defaults.
    ///
    /// Equivalent to [`Self::new`] today, with the judge stored for
    /// later-registered semantic evaluators (Phases 9–10). Phase 9
    /// (`SemanticToolSelectionEvaluator`) and Phase 10
    /// (`SemanticToolParameterEvaluator`) will wire themselves into this
    /// constructor when they land. Until then the registry is empty but the
    /// judge is retained — callers can still `register()` their own semantic
    /// evaluators that consume the judge.
    // TODO(#740,#741): register SemanticToolSelectionEvaluator /
    // SemanticToolParameterEvaluator once Phases 9–10 land.
    #[must_use]
    pub fn with_judge(client: Arc<dyn JudgeClient>) -> Self {
        Self {
            evaluators: Vec::new(),
            judge: Some(client),
        }
    }

    /// Create a registry pre-loaded with the v1 defaults plus the v2 semantic
    /// evaluators that require a judge.
    ///
    /// Today this is equivalent to [`Self::with_defaults`] with the judge
    /// attached; the semantic evaluator registrations are deferred to
    /// Phases 9–10 per the spec-023 rollout plan.
    // TODO(#740,#741): register SemanticToolSelectionEvaluator /
    // SemanticToolParameterEvaluator once Phases 9–10 land.
    #[must_use]
    pub fn with_defaults_and_judge(client: Arc<dyn JudgeClient>) -> Self {
        let mut registry = Self::with_defaults();
        registry.judge = Some(client);
        registry
    }

    /// Borrow the registered [`JudgeClient`], if any.
    ///
    /// Exposed so Phases 9–10 can pass the judge into their evaluator
    /// constructors at registration time.
    #[must_use]
    pub fn judge(&self) -> Option<&Arc<dyn JudgeClient>> {
        self.judge.as_ref()
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
            .filter_map(
                |e| match catch_unwind(AssertUnwindSafe(|| e.evaluate(case, invocation))) {
                    Ok(result) => result,
                    Err(payload) => Some(EvalMetricResult {
                        evaluator_name: e.name().to_string(),
                        score: Score::fail(),
                        details: Some(format!(
                            "evaluator panicked: {}",
                            panic_payload_message(payload.as_ref())
                        )),
                    }),
                },
            )
            .collect()
    }
}

impl Default for EvaluatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::MockJudge;

    #[test]
    fn with_defaults_has_no_judge() {
        let registry = EvaluatorRegistry::with_defaults();
        assert!(registry.judge().is_none());
    }

    #[test]
    fn with_judge_stores_client() {
        let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
        let registry = EvaluatorRegistry::with_judge(judge);
        assert!(registry.judge().is_some());
    }

    #[test]
    fn with_defaults_and_judge_has_defaults_plus_judge() {
        let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
        let registry = EvaluatorRegistry::with_defaults_and_judge(judge);
        assert!(registry.judge().is_some());
        assert_eq!(registry.evaluators.len(), 5);
    }
}
