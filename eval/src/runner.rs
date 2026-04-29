//! Evaluation runner that orchestrates the full eval pipeline.
//!
//! Spec 043 US2 adds bounded parallelism, `num_runs` variance sampling, a
//! pluggable invocation cache, cooperative cancellation, and
//! `initial_session_file` loading. See FR-036..FR-040 and research §R-009,
//! R-013, R-020, R-023.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use swink_agent::{
    Agent, AssistantMessage, ContentBlock, Cost, ModelSpec, SessionState, StopReason, Usage,
    UserMessage,
};

use crate::cache::{CacheKey, EvaluationDataStore, FingerprintContext};
use crate::error::EvalError;
use crate::evaluator::EvaluatorRegistry;
use crate::score::{Score, Verdict};
#[cfg(feature = "telemetry")]
use crate::telemetry::{CaseSpan, EvalsTelemetry, RunSetSpan, RunSetSpanRef};
use crate::trajectory::TrajectoryCollector;
use crate::types::{
    EvalCase, EvalCaseResult, EvalMetricResult, EvalSet, EvalSetResult, EvalSummary, Invocation,
    TurnRecord,
};

struct FactoryCancellationGuard(CancellationToken);

impl Drop for FactoryCancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Factory that creates a configured [`Agent`] for each eval case.
pub trait AgentFactory: Send + Sync {
    /// Create an agent and cancellation token for the given eval case.
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError>;

    /// Optional hook invoked with a baseline [`SessionState`] loaded from the
    /// runner's `initial_session_file` (FR-039). Default is a no-op.
    fn with_initial_session(&self, _state: &SessionState) {}

    /// Optional hook: SHA-256 of the agent's tool names + JSON schemas,
    /// contributing to the cache fingerprint (FR-038).
    fn tool_set_hash(&self, _case: &EvalCase) -> Option<String> {
        None
    }

    /// Optional hook: model identifier, contributing to the cache fingerprint.
    fn agent_model(&self, _case: &EvalCase) -> Option<String> {
        None
    }
}

/// Aggregated per-(case, evaluator) sample surfaced by
/// [`EvalRunner::with_num_runs`]. `std_dev` over the samples quantifies judge
/// non-determinism (research §R-013).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerMetricSample {
    /// Name of the evaluator.
    pub evaluator_name: String,
    /// Per-run raw scores in run-order.
    pub scores: Vec<f64>,
    /// Mean of `scores`.
    pub mean: f64,
    /// Population standard deviation of `scores`. `0.0` for a single sample.
    pub std_dev: f64,
}

impl RunnerMetricSample {
    fn from_samples(evaluator_name: String, scores: Vec<f64>) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let n = scores.len() as f64;
        let mean = if scores.is_empty() {
            0.0
        } else {
            scores.iter().sum::<f64>() / n
        };
        let std_dev = if scores.len() <= 1 {
            0.0
        } else {
            (scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n).sqrt()
        };
        Self {
            evaluator_name,
            scores,
            mean,
            std_dev,
        }
    }
}

/// Orchestrates evaluation: runs agents, captures trajectories, and scores
/// results. Default: sequential, `num_runs=1`, no cache, no cancellation.
pub struct EvalRunner {
    registry: EvaluatorRegistry,
    parallelism: usize,
    num_runs: u32,
    cache: Option<Arc<dyn EvaluationDataStore>>,
    cancel: Option<CancellationToken>,
    initial_session_file: Option<PathBuf>,
    agent_invocations: Arc<AtomicUsize>,
    #[cfg(feature = "telemetry")]
    telemetry: Option<Arc<EvalsTelemetry>>,
}

impl EvalRunner {
    /// Create a runner with a custom evaluator registry.
    #[must_use]
    pub fn new(registry: EvaluatorRegistry) -> Self {
        Self {
            registry,
            parallelism: 1,
            num_runs: 1,
            cache: None,
            cancel: None,
            initial_session_file: None,
            agent_invocations: Arc::new(AtomicUsize::new(0)),
            #[cfg(feature = "telemetry")]
            telemetry: None,
        }
    }

    /// Create a runner pre-loaded with built-in evaluators.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(EvaluatorRegistry::with_defaults())
    }

    /// Maximum number of concurrent case executions (FR-036).
    ///
    /// # Panics
    ///
    /// Panics if `n == 0`.
    #[must_use]
    pub fn with_parallelism(mut self, n: usize) -> Self {
        assert!(n > 0, "EvalRunner::with_parallelism: n must be > 0");
        self.parallelism = n;
        self
    }

    /// Repeat judge-side scoring `n` times per case (FR-037 / Q2).
    ///
    /// # Panics
    ///
    /// Panics if `n == 0`.
    #[must_use]
    pub fn with_num_runs(mut self, n: u32) -> Self {
        assert!(n > 0, "EvalRunner::with_num_runs: n must be > 0");
        self.num_runs = n;
        self
    }

    /// Attach a pluggable [`EvaluationDataStore`] for cached invocations
    /// (FR-038).
    #[must_use]
    pub fn with_cache(mut self, store: Arc<dyn EvaluationDataStore>) -> Self {
        self.cache = Some(store);
        self
    }

    /// Attach a [`CancellationToken`] honored at every await point (FR-040).
    #[must_use]
    pub fn with_cancellation(mut self, token: CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Load the given JSON file as an initial [`SessionState`] before each
    /// case (FR-039 / R-023). Missing / malformed files surface as
    /// [`EvalError::InvalidCase`] — never a panic.
    #[must_use]
    pub fn with_initial_session_file(mut self, path: PathBuf) -> Self {
        self.initial_session_file = Some(path);
        self
    }

    /// Attach an [`EvalsTelemetry`] (spec 043 US7 / FR-035). When present,
    /// [`Self::run_set`] emits the three-level span tree
    /// `swink.eval.run_set` → `swink.eval.case` → `swink.eval.evaluator`.
    #[cfg(feature = "telemetry")]
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: Arc<EvalsTelemetry>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Number of times an agent was actually invoked (cache miss count).
    #[must_use]
    pub fn agent_invocation_count(&self) -> usize {
        self.agent_invocations.load(Ordering::SeqCst)
    }

    /// Reset the agent-invocation counter to zero.
    pub fn reset_agent_invocation_count(&self) {
        self.agent_invocations.store(0, Ordering::SeqCst);
    }

    /// Run a single eval case and return the scored result.
    pub async fn run_case(
        &self,
        case: &EvalCase,
        factory: &dyn AgentFactory,
    ) -> Result<EvalCaseResult, EvalError> {
        info!(case_id = %case.id, case_name = %case.name, "running eval case");
        let initial_session = self.load_initial_session()?;
        if let Some(state) = &initial_session {
            factory.with_initial_session(state);
        }
        let invocation = invoke_agent_impl(
            case,
            factory,
            self.cancel.as_ref(),
            initial_session.as_ref(),
            &self.agent_invocations,
        )
        .await?;
        let metric_results = self.registry.evaluate(case, &invocation);
        Ok(scored_case_result(case, invocation, metric_results))
    }

    /// Run an entire eval set and return aggregated results.
    #[allow(clippy::too_many_lines)]
    pub async fn run_set(
        &self,
        eval_set: &EvalSet,
        factory: &dyn AgentFactory,
    ) -> Result<EvalSetResult, EvalError> {
        info!(
            set_id = %eval_set.id, cases = eval_set.cases.len(),
            parallelism = self.parallelism, num_runs = self.num_runs,
            cache = self.cache.is_some(), "running eval set"
        );

        // ─── FR-035: root span for the whole run_set ──────────────────
        #[cfg(feature = "telemetry")]
        let run_set_span: Option<RunSetSpan> = self
            .telemetry
            .as_ref()
            .map(|t| t.start_run_set_span(eval_set));

        let initial_session = self.load_initial_session()?;
        let initial_session_json = initial_session
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(EvalError::from)?;
        if let Some(state) = &initial_session {
            factory.with_initial_session(state);
        }

        let semaphore = Arc::new(Semaphore::new(self.parallelism));
        let eval_set_id = eval_set.id.clone();

        // buffer_unordered-style loop over local futures — keeps
        // `factory: &dyn AgentFactory` borrowable (no 'static bound required).
        let mut futures_vec = Vec::with_capacity(eval_set.cases.len());
        for (index, case) in eval_set.cases.iter().enumerate() {
            let sem = Arc::clone(&semaphore);
            let cache = self.cache.clone();
            let registry = &self.registry;
            let num_runs = self.num_runs;
            let cancel = self.cancel.clone();
            let initial_session_state = initial_session.clone();
            let initial_session_value = initial_session_json.clone();
            let agent_invocations = Arc::clone(&self.agent_invocations);
            let eval_set_id = eval_set_id.clone();
            #[cfg(feature = "telemetry")]
            let telemetry = self.telemetry.clone();
            #[cfg(feature = "telemetry")]
            let run_set_context = run_set_span.as_ref().map(|s| RunSetSpanRef {
                context: s.context().clone(),
                set_id: eval_set_id.clone(),
            });

            futures_vec.push(async move {
                if let Some(tok) = &cancel
                    && tok.is_cancelled()
                {
                    return (index, cancelled_case_result(case));
                }
                let permit = match sem.acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => return (index, cancelled_case_result(case)),
                };
                if let Some(tok) = &cancel
                    && tok.is_cancelled()
                {
                    drop(permit);
                    return (index, cancelled_case_result(case));
                }

                #[cfg(feature = "telemetry")]
                let case_span: Option<CaseSpan> = match (&telemetry, &run_set_context) {
                    (Some(t), Some(parent)) => Some(t.start_case_span_raw(parent, case)),
                    _ => None,
                };
                #[cfg(feature = "telemetry")]
                let case_start = std::time::Instant::now();

                let result = execute_case(
                    case,
                    factory,
                    &eval_set_id,
                    cache.as_deref(),
                    registry,
                    num_runs,
                    cancel.as_ref(),
                    initial_session_state.as_ref(),
                    initial_session_value.as_ref(),
                    &agent_invocations,
                    #[cfg(feature = "telemetry")]
                    telemetry.as_deref(),
                    #[cfg(feature = "telemetry")]
                    case_span.as_ref(),
                )
                .await
                .unwrap_or_else(|e| error_case_result(case, &e));

                #[cfg(feature = "telemetry")]
                if let Some(span) = case_span {
                    span.end(&result, case_start.elapsed());
                }

                drop(permit);
                (index, result)
            });
        }

        let results: Vec<(usize, EvalCaseResult)> = futures::future::join_all(futures_vec).await;
        let mut ordered: Vec<Option<EvalCaseResult>> =
            (0..eval_set.cases.len()).map(|_| None).collect();
        for (index, result) in results {
            ordered[index] = Some(result);
        }
        let case_results: Vec<EvalCaseResult> = ordered
            .into_iter()
            .map(|slot| slot.expect("every case produces a result"))
            .collect();

        let mut total_cost = Cost::default();
        let mut total_usage = Usage::default();
        let mut total_duration = std::time::Duration::ZERO;
        let mut passed = 0usize;
        let mut failed = 0usize;
        for result in &case_results {
            total_cost += result.invocation.total_cost.clone();
            total_usage += result.invocation.total_usage.clone();
            total_duration += result.invocation.total_duration;
            if result.verdict.is_pass() {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        let summary = EvalSummary {
            total_cases: eval_set.cases.len(),
            passed,
            failed,
            total_cost,
            total_usage,
            total_duration,
        };
        info!(
            set_id = %eval_set.id, passed = summary.passed,
            failed = summary.failed, total = summary.total_cases,
            "eval set complete"
        );

        #[cfg(feature = "telemetry")]
        if let Some(span) = run_set_span {
            span.end(summary.passed, summary.failed);
        }

        Ok(EvalSetResult {
            eval_set_id: eval_set.id.clone(),
            case_results,
            summary,
            timestamp: swink_agent::now_timestamp(),
        })
    }

    fn load_initial_session(&self) -> Result<Option<SessionState>, EvalError> {
        let Some(path) = &self.initial_session_file else {
            return Ok(None);
        };
        let bytes = std::fs::read(path).map_err(|err| {
            EvalError::invalid_case(format!(
                "initial_session_file `{}` unreadable: {err}",
                path.display()
            ))
        })?;
        let state: SessionState = serde_json::from_slice(&bytes).map_err(|err| {
            EvalError::invalid_case(format!(
                "initial_session_file `{}` is not valid SessionState JSON: {err}",
                path.display()
            ))
        })?;
        Ok(Some(state))
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_case(
    case: &EvalCase,
    factory: &dyn AgentFactory,
    eval_set_id: &str,
    cache: Option<&(dyn EvaluationDataStore + 'static)>,
    registry: &EvaluatorRegistry,
    num_runs: u32,
    cancel: Option<&CancellationToken>,
    initial_session: Option<&SessionState>,
    initial_session_json: Option<&serde_json::Value>,
    agent_invocations: &AtomicUsize,
    #[cfg(feature = "telemetry")] telemetry: Option<&EvalsTelemetry>,
    #[cfg(feature = "telemetry")] case_span: Option<&CaseSpan>,
) -> Result<EvalCaseResult, EvalError> {
    info!(case_id = %case.id, case_name = %case.name, "running eval case");

    let fingerprint = case.content_fingerprint();
    let fp_ctx = FingerprintContext {
        initial_session: initial_session_json.cloned(),
        tool_set_hash: factory.tool_set_hash(case),
        agent_model: factory.agent_model(case),
    };
    let cache_key = CacheKey::from_fingerprint(&fingerprint, &fp_ctx);

    let cached = match cache {
        Some(store) => match store.get(eval_set_id, &case.id, &cache_key) {
            Ok(v) => v,
            Err(err) => {
                warn!(case_id = %case.id, error = %err, "cache read failed");
                None
            }
        },
        None => None,
    };

    let invocation = if let Some(inv) = cached {
        debug!(case_id = %case.id, "cache hit");
        inv
    } else {
        let inv =
            invoke_agent_impl(case, factory, cancel, initial_session, agent_invocations).await?;
        if let Some(store) = cache
            && let Err(err) = store.put(eval_set_id, &case.id, &cache_key, &inv)
        {
            warn!(case_id = %case.id, error = %err, "cache write failed");
        }
        inv
    };

    let metric_results = dispatch_evaluators(
        registry,
        case,
        &invocation,
        num_runs,
        cancel,
        #[cfg(feature = "telemetry")]
        telemetry,
        #[cfg(feature = "telemetry")]
        case_span,
    );
    Ok(scored_case_result(case, invocation, metric_results))
}

fn scored_case_result(
    case: &EvalCase,
    invocation: Invocation,
    mut metric_results: Vec<EvalMetricResult>,
) -> EvalCaseResult {
    if metric_results.is_empty() {
        metric_results.push(no_applicable_evaluators_metric());
    }
    let verdict = if metric_results.iter().all(|r| r.score.verdict().is_pass()) {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    EvalCaseResult {
        case_id: case.id.clone(),
        invocation,
        metric_results,
        verdict,
    }
}

fn no_applicable_evaluators_metric() -> EvalMetricResult {
    EvalMetricResult {
        evaluator_name: "no_applicable_evaluators".to_string(),
        score: Score::fail(),
        details: Some(
            "no evaluator produced a metric; configure an applicable evaluator or expected criteria"
                .to_string(),
        ),
    }
}

async fn invoke_agent_impl(
    case: &EvalCase,
    factory: &dyn AgentFactory,
    cancel: Option<&CancellationToken>,
    initial_session: Option<&SessionState>,
    agent_invocations: &AtomicUsize,
) -> Result<Invocation, EvalError> {
    agent_invocations.fetch_add(1, Ordering::SeqCst);
    let (mut agent, factory_cancel) = factory.create_agent(case)?;
    if let Some(state) = initial_session {
        *agent
            .session_state()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state.clone();
    }
    let _factory_cancel = FactoryCancellationGuard(factory_cancel);
    let messages: Vec<_> = case
        .user_messages
        .iter()
        .map(|text| {
            swink_agent::AgentMessage::Llm(swink_agent::LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text { text: text.clone() }],
                timestamp: swink_agent::now_timestamp(),
                cache_hint: None,
            }))
        })
        .collect();
    let stream = agent.prompt_stream(messages)?;
    let invocation = if let Some(tok) = cancel {
        tokio::select! {
            biased;
            () = tok.cancelled() => {
                // select! drops the losing branch's future, which drops the
                // stream and cancels any in-flight request to the agent.
                return Ok(cancellation_placeholder_invocation());
            }
            inv = TrajectoryCollector::collect_from_stream(stream) => inv,
        }
    } else {
        TrajectoryCollector::collect_from_stream(stream).await
    };
    Ok(invocation)
}

#[allow(clippy::too_many_arguments)]
fn dispatch_evaluators(
    registry: &EvaluatorRegistry,
    case: &EvalCase,
    invocation: &Invocation,
    num_runs: u32,
    cancel: Option<&CancellationToken>,
    #[cfg(feature = "telemetry")] telemetry: Option<&EvalsTelemetry>,
    #[cfg(feature = "telemetry")] case_span: Option<&CaseSpan>,
) -> Vec<EvalMetricResult> {
    debug_assert!(num_runs > 0);
    if num_runs == 1 {
        return run_registry_once(
            registry,
            case,
            invocation,
            #[cfg(feature = "telemetry")]
            telemetry,
            #[cfg(feature = "telemetry")]
            case_span,
        );
    }

    let mut per_evaluator: std::collections::BTreeMap<String, Vec<EvalMetricResult>> =
        std::collections::BTreeMap::new();
    let mut cancelled = false;
    for run_idx in 0..num_runs {
        if let Some(tok) = cancel
            && tok.is_cancelled()
        {
            cancelled = true;
            break;
        }
        // Only the first num_runs iteration gets its evaluator spans emitted
        // so the span tree stays proportional to evaluator count; per-run
        // scores are still aggregated on the synthesised mean span below.
        #[cfg(feature = "telemetry")]
        let iteration_telemetry = if run_idx == 0 { telemetry } else { None };
        #[cfg(feature = "telemetry")]
        let iteration_case_span = if run_idx == 0 { case_span } else { None };
        let iteration = run_registry_once(
            registry,
            case,
            invocation,
            #[cfg(feature = "telemetry")]
            iteration_telemetry,
            #[cfg(feature = "telemetry")]
            iteration_case_span,
        );
        for metric in iteration {
            per_evaluator
                .entry(metric.evaluator_name.clone())
                .or_default()
                .push(metric);
        }
        debug!(case_id = %case.id, run = run_idx + 1, "num_runs sample recorded");
    }

    let mut aggregated: Vec<EvalMetricResult> = per_evaluator
        .into_iter()
        .map(|(name, samples)| {
            let scores: Vec<f64> = samples.iter().map(|m| m.score.value).collect();
            let threshold = samples.first().map_or(0.5, |m| m.score.threshold);
            let sample = RunnerMetricSample::from_samples(name.clone(), scores);
            let mut detail_lines = vec![format!(
                "num_runs={} mean={:.4} std_dev={:.4}",
                sample.scores.len(),
                sample.mean,
                sample.std_dev
            )];
            let prior: Vec<_> = samples.iter().filter_map(|m| m.details.clone()).collect();
            if !prior.is_empty() {
                detail_lines.push(prior.join(" | "));
            }
            EvalMetricResult {
                evaluator_name: name,
                score: Score::new(sample.mean, threshold),
                details: Some(detail_lines.join(" :: ")),
            }
        })
        .collect();

    if cancelled {
        aggregated.push(cancelled_metric_result(
            "runner cancellation observed during multi-run evaluator dispatch",
        ));
    }

    aggregated
}

/// Invoke every applicable evaluator once. When `telemetry` + `case_span`
/// are supplied, each evaluator call is wrapped in a `swink.eval.evaluator`
/// span (FR-035) that inherits the case span as parent.
fn run_registry_once(
    registry: &EvaluatorRegistry,
    case: &EvalCase,
    invocation: &Invocation,
    #[cfg(feature = "telemetry")] telemetry: Option<&EvalsTelemetry>,
    #[cfg(feature = "telemetry")] case_span: Option<&CaseSpan>,
) -> Vec<EvalMetricResult> {
    #[cfg(feature = "telemetry")]
    if let (Some(t), Some(parent)) = (telemetry, case_span) {
        return registry.evaluate_instrumented(case, invocation, |name, run| {
            let span = t.start_evaluator_span(parent, name);
            let outcome = run();
            match outcome.as_ref() {
                Some(metric) => span.end(metric),
                None => span.end_inapplicable(name),
            }
            outcome
        });
    }
    registry.evaluate(case, invocation)
}

fn cancelled_case_result(case: &EvalCase) -> EvalCaseResult {
    EvalCaseResult {
        case_id: case.id.clone(),
        invocation: error_invocation(None),
        metric_results: vec![cancelled_metric_result(
            "runner cancellation observed before case completion",
        )],
        verdict: Verdict::Fail,
    }
}

fn cancelled_metric_result(details: &str) -> EvalMetricResult {
    EvalMetricResult {
        evaluator_name: "cancelled".to_string(),
        score: Score::fail(),
        details: Some(details.to_string()),
    }
}

fn error_case_result(case: &EvalCase, err: &EvalError) -> EvalCaseResult {
    warn!(case_id = %case.id, error = %err, "eval case failed with error");
    EvalCaseResult {
        case_id: case.id.clone(),
        invocation: error_invocation(Some(err.to_string())),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "error".to_string(),
            score: Score::fail(),
            details: Some(err.to_string()),
        }],
        verdict: Verdict::Fail,
    }
}

fn cancellation_placeholder_invocation() -> Invocation {
    error_invocation(None)
}

fn error_invocation(error_message: Option<String>) -> Invocation {
    let turns = error_message
        .map(|msg| {
            vec![TurnRecord {
                turn_index: 0,
                assistant_message: AssistantMessage {
                    content: vec![],
                    provider: String::new(),
                    model_id: String::new(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Error,
                    error_message: Some(msg),
                    error_kind: None,
                    timestamp: swink_agent::now_timestamp(),
                    cache_hint: None,
                },
                tool_calls: vec![],
                tool_results: vec![],
                duration: std::time::Duration::ZERO,
            }]
        })
        .unwrap_or_default();
    Invocation {
        turns,
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: std::time::Duration::ZERO,
        final_response: None,
        stop_reason: StopReason::Error,
        model: ModelSpec::new("unknown", "unknown"),
    }
}
