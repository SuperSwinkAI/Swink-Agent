//! Evaluator trait and registry for composing multiple evaluation metrics.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;

use tokio::runtime::{Handle, RuntimeFlavor};
#[cfg(feature = "judge-core")]
use tokio_util::sync::CancellationToken;

use crate::aggregator::{Aggregator, Average};
use crate::error::EvalError;
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

    /// Async counterpart to [`Self::evaluate`] (spec 043 FR-048).
    ///
    /// Returns a boxed future rather than being declared `async fn` so the
    /// trait stays dyn-compatible — [`EvaluatorRegistry`] stores evaluators as
    /// `Arc<dyn Evaluator>`, and native `async fn`s in traits cannot be called
    /// through a trait object. The default implementation wraps the blocking
    /// [`Self::evaluate`] in an already-ready future: every built-in evaluator
    /// is either pure CPU work or a judge-backed evaluator that already drives
    /// its own async dispatch synchronously via `crate::evaluators::block_on`,
    /// so there is no blocking work left to offload with
    /// `tokio::task::spawn_blocking` here. Evaluators with genuinely
    /// long-running non-blocking async work should override this method
    /// directly instead of routing through `evaluate`.
    fn evaluate_async<'a>(
        &'a self,
        case: &'a EvalCase,
        invocation: &'a Invocation,
    ) -> Pin<Box<dyn Future<Output = Option<EvalMetricResult>> + Send + 'a>> {
        Box::pin(async move { self.evaluate(case, invocation) })
    }

    /// [`Aggregator`] used to reduce multiple same-evaluator samples (e.g.
    /// repeated [`EvalRunner::with_num_runs`](crate::EvalRunner::with_num_runs)
    /// iterations) into one composite score (FR-022/FR-023).
    ///
    /// Defaults to the arithmetic-mean [`Average`] aggregator. Judge-backed
    /// evaluators that carry a `JudgeEvaluatorConfig` override this to return
    /// `config.effective_aggregator()`, so a caller-supplied
    /// `with_aggregator(...)` override (or a family default such as the
    /// safety family's `AllPass`) is honored here too.
    fn aggregator(&self) -> Arc<dyn Aggregator> {
        Arc::new(Average)
    }
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

    /// Create a registry wired with a judge client plus the Phase 9+
    /// semantic evaluators that require a judge.
    ///
    /// Registers [`SemanticToolSelectionEvaluator`](crate::SemanticToolSelectionEvaluator)
    /// (Phase 9 / US5) and
    /// [`SemanticToolParameterEvaluator`](crate::SemanticToolParameterEvaluator)
    /// (Phase 10 / US6). Both evaluators are inert on cases without their
    /// respective criteria set (return `None`).
    #[must_use]
    pub fn with_judge(client: Arc<dyn JudgeClient>) -> Self {
        let mut registry = Self {
            evaluators: Vec::new(),
            judge: Some(Arc::clone(&client)),
        };
        registry.register(
            crate::semantic_tool_selection::SemanticToolSelectionEvaluator::new(Arc::clone(
                &client,
            )),
        );
        registry
            .register(crate::semantic_tool_parameter::SemanticToolParameterEvaluator::new(client));
        registry
    }

    /// Create a registry pre-loaded with the v1 defaults plus the v2 semantic
    /// evaluators that require a judge.
    ///
    /// Combines [`Self::with_defaults`] with the Phase 9+ semantic evaluators
    /// (see [`Self::with_judge`]).
    #[must_use]
    pub fn with_defaults_and_judge(client: Arc<dyn JudgeClient>) -> Self {
        let mut registry = Self::with_defaults();
        registry.judge = Some(Arc::clone(&client));
        registry.register(
            crate::semantic_tool_selection::SemanticToolSelectionEvaluator::new(Arc::clone(
                &client,
            )),
        );
        registry
            .register(crate::semantic_tool_parameter::SemanticToolParameterEvaluator::new(client));
        registry
    }

    /// Look up the [`Aggregator`] configured for a registered evaluator by
    /// name (FR-022/FR-023).
    ///
    /// Used by the runner to combine `num_runs` samples of the same
    /// evaluator into one composite score, honoring each evaluator's own
    /// [`Evaluator::aggregator`] (family default or caller override) instead
    /// of a single hard-coded reduction for every evaluator. Falls back to
    /// [`Average`] when `name` isn't registered — this should not happen in
    /// practice since callers only look up names already present in a prior
    /// [`Self::evaluate`] result.
    #[must_use]
    pub(crate) fn aggregator_for(&self, name: &str) -> Arc<dyn Aggregator> {
        self.evaluators
            .iter()
            .find(|evaluator| evaluator.name() == name)
            .map_or_else(
                || Arc::new(Average) as Arc<dyn Aggregator>,
                |evaluator| evaluator.aggregator(),
            )
    }

    /// Borrow the registered [`JudgeClient`], if any.
    ///
    /// Exposed so Phases 9–10 can pass the judge into their evaluator
    /// constructors at registration time.
    #[must_use]
    pub fn judge(&self) -> Option<&Arc<dyn JudgeClient>> {
        self.judge.as_ref()
    }

    /// Register a new evaluator, rejecting duplicate names.
    pub fn add(&mut self, evaluator: impl Evaluator + 'static) -> Result<(), EvalError> {
        let name = evaluator.name();
        if self
            .evaluators
            .iter()
            .any(|registered| registered.name() == name)
        {
            return Err(EvalError::duplicate_evaluator(name));
        }

        self.evaluators.push(Arc::new(evaluator));
        Ok(())
    }

    /// Register a new evaluator.
    ///
    /// Panics if an evaluator with the same [`Evaluator::name`] is already
    /// present in the registry. Use [`Self::add`] to surface the collision as
    /// [`EvalError::DuplicateEvaluator`].
    pub fn register(&mut self, evaluator: impl Evaluator + 'static) {
        self.add(evaluator)
            .expect("evaluator names must be unique within a registry");
    }

    /// Run all applicable evaluators for a case.
    ///
    /// If `case.evaluators` is non-empty, only evaluators whose names appear
    /// in that list are run. Otherwise, all registered evaluators are run.
    #[must_use]
    pub fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Vec<EvalMetricResult> {
        self.evaluate_instrumented_internal(
            case,
            invocation,
            #[cfg(feature = "judge-core")]
            None,
            |_name, run| run(),
        )
    }

    /// Run all applicable evaluators while exposing `cancellation` to any
    /// judge-backed evaluator dispatch performed under panic isolation.
    #[cfg(feature = "judge-core")]
    #[must_use]
    pub(crate) fn evaluate_with_judge_cancellation(
        &self,
        case: &EvalCase,
        invocation: &Invocation,
        cancellation: Option<&CancellationToken>,
    ) -> Vec<EvalMetricResult> {
        self.evaluate_instrumented_with_judge_cancellation(
            case,
            invocation,
            cancellation,
            |_name, run| run(),
        )
    }

    /// Variant of [`Self::evaluate`] that lets the caller wrap each
    /// evaluator invocation — e.g. in an OTel span (spec 043 US7 / FR-035).
    ///
    /// `wrap` is invoked once per applicable evaluator with its name and a
    /// closure that, when called, runs the evaluator under the existing
    /// panic-isolation guard. `wrap` may return `None` to drop the metric.
    ///
    /// The shape is deliberately synchronous to match the existing
    /// [`Self::evaluate`] surface and keep the observer bridge simple.
    pub fn evaluate_instrumented<F>(
        &self,
        case: &EvalCase,
        invocation: &Invocation,
        wrap: F,
    ) -> Vec<EvalMetricResult>
    where
        F: FnMut(&str, &mut dyn FnMut() -> Option<EvalMetricResult>) -> Option<EvalMetricResult>,
    {
        self.evaluate_instrumented_internal(
            case,
            invocation,
            #[cfg(feature = "judge-core")]
            None,
            wrap,
        )
    }

    /// Instrumented evaluator dispatch with runner cancellation available to
    /// judge-backed evaluators.
    #[cfg(feature = "judge-core")]
    pub(crate) fn evaluate_instrumented_with_judge_cancellation<F>(
        &self,
        case: &EvalCase,
        invocation: &Invocation,
        cancellation: Option<&CancellationToken>,
        wrap: F,
    ) -> Vec<EvalMetricResult>
    where
        F: FnMut(&str, &mut dyn FnMut() -> Option<EvalMetricResult>) -> Option<EvalMetricResult>,
    {
        self.evaluate_instrumented_internal(case, invocation, cancellation, wrap)
    }

    fn evaluate_instrumented_internal<F>(
        &self,
        case: &EvalCase,
        invocation: &Invocation,
        #[cfg(feature = "judge-core")] cancellation: Option<&CancellationToken>,
        mut wrap: F,
    ) -> Vec<EvalMetricResult>
    where
        F: FnMut(&str, &mut dyn FnMut() -> Option<EvalMetricResult>) -> Option<EvalMetricResult>,
    {
        #[cfg(feature = "judge-core")]
        let cancellation = cancellation.cloned();
        let filter = &case.evaluators;
        self.evaluators
            .iter()
            .filter(|e| filter.is_empty() || filter.iter().any(|name| name == e.name()))
            .filter_map(|e| {
                let name = e.name();
                let evaluator = Arc::clone(e);
                #[cfg(feature = "judge-core")]
                let cancellation = cancellation.clone();
                let mut runner = move || {
                    let evaluator = Arc::clone(&evaluator);
                    let case = case.clone();
                    let invocation = invocation.clone();
                    #[cfg(feature = "judge-core")]
                    let cancellation = cancellation.clone();
                    isolate_panic(evaluator.name(), move || {
                        #[cfg(feature = "judge-core")]
                        {
                            crate::judge::with_scoped_judge_cancellation(
                                cancellation.as_ref(),
                                || evaluator.evaluate(&case, &invocation),
                            )
                        }
                        #[cfg(not(feature = "judge-core"))]
                        evaluator.evaluate(&case, &invocation)
                    })
                };
                wrap(name, &mut runner)
            })
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

pub(crate) fn isolate_panic<F>(location: &str, action: F) -> Option<EvalMetricResult>
where
    F: FnOnce() -> Option<EvalMetricResult> + Send + 'static,
{
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| {
                handle.block_on(isolate_panic_async(location, async move { action() }))
            })
        }
        _ => isolate_panic_inline(location, action),
    }
}

async fn isolate_panic_async<Fut>(location: &str, action: Fut) -> Option<EvalMetricResult>
where
    Fut: Future<Output = Option<EvalMetricResult>> + Send + 'static,
{
    match tokio::spawn(action).await {
        Ok(result) => result,
        Err(join_error) => Some(panic_metric(location, &join_error_message(join_error))),
    }
}

fn isolate_panic_inline<F>(location: &str, action: F) -> Option<EvalMetricResult>
where
    F: FnOnce() -> Option<EvalMetricResult>,
{
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(result) => result,
        Err(payload) => Some(panic_metric(
            location,
            &panic_payload_message(payload.as_ref()),
        )),
    }
}

fn join_error_message(join_error: tokio::task::JoinError) -> String {
    if join_error.is_panic() {
        let payload = join_error.into_panic();
        panic_payload_message(payload.as_ref())
    } else if join_error.is_cancelled() {
        "panic isolation task cancelled".to_string()
    } else {
        "unknown join error".to_string()
    }
}

fn panic_metric(location: &str, message: &str) -> EvalMetricResult {
    EvalMetricResult {
        evaluator_name: location.to_string(),
        score: Score::fail(),
        details: Some(format!("evaluator panicked in {location}: {message}")),
    }
}

#[cfg(test)]
#[path = "evaluator_tests.rs"]
mod tests;
