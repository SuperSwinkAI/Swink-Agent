//! Semantic tool-parameter evaluator (Spec 023 Phase 10 / US6).
//!
//! For each actual tool call in the invocation, this evaluator asks a
//! configured [`JudgeClient`] whether the call's arguments satisfy a declared
//! natural-language intent from `case.expected_tool_intent`. When the intent
//! carries a `tool_name` filter, only calls to that tool are judged; others
//! are skipped (not Pass, not Fail). The per-call verdicts are aggregated into
//! a single [`Score`].
//!
//! # Non-hang guarantee (FR-010, FR-014)
//!
//! Each judge call is wrapped in an outer `tokio::time::timeout`. The default
//! deadline is 5 minutes, overridable via [`SemanticToolParameterEvaluator::with_timeout`].
//! Both an inner [`JudgeError::Timeout`] and an outer timeout elapse map to
//! `Score::fail()` with timeout context surfaced in `EvalMetricResult.details`.
//!
//! # Opt-in
//!
//! The evaluator returns `None` when `case.expected_tool_intent.is_none()`
//! (FR-012). It also returns `None` when a `tool_name` filter is set but no
//! actual tool call matches that filter (AS-6.4 — targeted tool not present
//! means "no applicable calls to judge"). When no [`JudgeClient`] is
//! configured on the registry, the evaluator is simply not registered.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::evaluator::Evaluator;
use crate::judge::{JudgeClient, JudgeError, JudgeVerdict};
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation, RecordedToolCall, ToolIntent};

/// Default outer timeout applied to each judge call (5 minutes, FR-014).
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

/// Semantic tool-parameter evaluator backed by a [`JudgeClient`].
///
/// See the module-level docs for the full contract.
pub struct SemanticToolParameterEvaluator {
    judge: Arc<dyn JudgeClient>,
    timeout: Duration,
}

impl SemanticToolParameterEvaluator {
    /// Create a new evaluator with the default 5-minute per-call timeout.
    #[must_use]
    pub fn new(judge: Arc<dyn JudgeClient>) -> Self {
        Self {
            judge,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the per-judge-call outer `tokio::time::timeout` deadline.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Evaluator for SemanticToolParameterEvaluator {
    fn name(&self) -> &'static str {
        "semantic_tool_parameter"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // Opt-in per-case — FR-012 / AS-6.2.
        let intent = case.expected_tool_intent.as_ref()?;

        // Flatten all tool calls with their turn index, applying the optional
        // tool-name filter (AS-6.4 / T062).
        let filter = intent.tool_name.as_deref();
        let applicable: Vec<(usize, RecordedToolCall)> = invocation
            .turns
            .iter()
            .flat_map(|turn| {
                turn.tool_calls
                    .iter()
                    .cloned()
                    .map(move |tc| (turn.turn_index, tc))
            })
            .filter(|(_, call)| filter.is_none_or(|name| call.name == name))
            .collect();

        // AS-6.4: when a filter is set and no matching call exists in the
        // trajectory, the evaluator is not applicable (returns `None`, not
        // Pass/Fail). Without a filter, an empty trajectory also yields `None`
        // — spec Edge Cases list parallel to US5 empty-trajectory handling.
        if applicable.is_empty() {
            return None;
        }

        // Judge each applicable call. The Evaluator trait is sync; the
        // async judge is driven via whichever runtime context the caller
        // provides (see `drive_judge_calls` for the selection logic).
        let judge = Arc::clone(&self.judge);
        let timeout_limit = self.timeout;
        let intent = intent.clone();
        let outcomes: Vec<CallOutcome> = drive_judge_calls(move || async move {
            let mut results = Vec::with_capacity(applicable.len());
            for (turn_index, call) in &applicable {
                let prompt = build_prompt(&intent, *turn_index, call);
                let outcome = match timeout(timeout_limit, judge.judge(&prompt)).await {
                    Ok(Ok(verdict)) => CallOutcome::Verdict {
                        tool: call.name.clone(),
                        verdict,
                    },
                    Ok(Err(err)) => CallOutcome::JudgeError {
                        tool: call.name.clone(),
                        error: err,
                    },
                    Err(_elapsed) => CallOutcome::OuterTimeout {
                        tool: call.name.clone(),
                        limit: timeout_limit,
                    },
                };
                results.push(outcome);
            }
            results
        });

        Some(aggregate(&outcomes))
    }
}

/// Drive an async workload to completion from the sync `Evaluator::evaluate`
/// entry point, regardless of the caller's Tokio runtime state.
///
/// Mirrors the helper in [`crate::semantic_tool_selection`] — kept module-
/// local to avoid a new cross-module public surface. See that module's
/// helper for the full runtime-selection rationale.
fn drive_judge_calls<F, Fut, T>(make_future: F) -> T
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    use tokio::runtime::{Handle, RuntimeFlavor};

    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(make_future()));
    }

    if Handle::try_current().is_ok() {
        return std::thread::Builder::new()
            .name("swink-eval-judge".to_string())
            .spawn(|| block_on_owned_runtime(make_future))
            .expect("spawn current-thread judge helper")
            .join()
            .expect("current-thread judge helper panicked");
    }

    block_on_owned_runtime(make_future)
}

fn block_on_owned_runtime<F, Fut, T>(make_future: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime for judge calls");
    rt.block_on(make_future())
}

/// Per-call outcome combining judge success, judge error, or outer timeout.
enum CallOutcome {
    Verdict { tool: String, verdict: JudgeVerdict },
    JudgeError { tool: String, error: JudgeError },
    OuterTimeout { tool: String, limit: Duration },
}

/// Aggregate per-call outcomes into the final [`EvalMetricResult`].
///
/// * Any judge error or outer timeout → overall `Score::fail()` with all
///   diagnostic reasons concatenated.
/// * Otherwise → mean verdict score across successful judge calls (FR-014).
fn aggregate(outcomes: &[CallOutcome]) -> EvalMetricResult {
    let mut verdict_scores: Vec<f64> = Vec::new();
    let mut verdict_reasons: Vec<String> = Vec::new();
    let mut had_failure = false;
    let mut failure_details: Vec<String> = Vec::new();

    for outcome in outcomes {
        match outcome {
            CallOutcome::Verdict { tool, verdict } => {
                verdict_scores.push(verdict.score.clamp(0.0, 1.0));
                let reason = verdict
                    .reason
                    .clone()
                    .unwrap_or_else(|| "no reason".to_string());
                verdict_reasons.push(format!(
                    "{tool}: {status} ({reason})",
                    status = if verdict.pass { "pass" } else { "fail" }
                ));
                if !verdict.pass {
                    had_failure = true;
                }
            }
            CallOutcome::JudgeError { tool, error } => {
                had_failure = true;
                failure_details.push(format!(
                    "{tool}: judge error — {variant}: {error}",
                    variant = judge_error_variant(error),
                ));
            }
            CallOutcome::OuterTimeout { tool, limit } => {
                had_failure = true;
                failure_details.push(format!("{tool}: judge call exceeded {limit:?}"));
            }
        }
    }

    let score = if had_failure {
        Score::fail()
    } else {
        let mean = if verdict_scores.is_empty() {
            0.0
        } else {
            let total: f64 = verdict_scores.iter().sum();
            #[allow(clippy::cast_precision_loss)]
            let len_f = verdict_scores.len() as f64;
            total / len_f
        };
        Score::new(mean, 0.5)
    };

    let mut details: Vec<String> = Vec::new();
    if !verdict_reasons.is_empty() {
        details.push(verdict_reasons.join("; "));
    }
    if !failure_details.is_empty() {
        details.push(failure_details.join("; "));
    }

    EvalMetricResult {
        evaluator_name: "semantic_tool_parameter".to_string(),
        score,
        details: if details.is_empty() {
            None
        } else {
            Some(details.join(" | "))
        },
    }
}

const fn judge_error_variant(err: &JudgeError) -> &'static str {
    match err {
        JudgeError::Transport(_) => "Transport",
        JudgeError::Timeout => "Timeout",
        JudgeError::MalformedResponse(_) => "MalformedResponse",
        JudgeError::Other(_) => "Other",
    }
}

fn build_prompt(intent: &ToolIntent, turn_index: usize, call: &RecordedToolCall) -> String {
    let args = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "<unserializable>".into());
    let tool_name = intent.tool_name.as_deref().unwrap_or(call.name.as_str());
    let intent_text = &intent.intent;
    let actual_name = &call.name;
    format!(
        "You are judging whether an agent's tool-call arguments semantically \
satisfy a declared intent.\n\n\
Declared intent:\n{intent_text}\n\n\
Targeted tool name: {tool_name}\n\
Actual tool call under review (turn {turn_index}):\n  {actual_name}({args})\n\n\
Decide whether the arguments fulfil the declared intent. Respond with a \
Pass/Fail verdict and a short reason.",
    )
}

#[cfg(test)]
#[path = "semantic_tool_parameter_tests.rs"]
mod tests;
