//! Semantic tool-selection evaluator (Spec 023 Phase 9 / US5).
//!
//! For each actual tool call in the invocation, this evaluator asks a
//! configured [`JudgeClient`] whether the chosen tool was semantically
//! appropriate given the user goal, available session history, and chosen
//! tool. The per-call verdicts are aggregated into a single [`Score`].
//!
//! # Non-hang guarantee (FR-010, FR-014)
//!
//! Each judge call is wrapped in an outer `tokio::time::timeout`. The default
//! deadline is 5 minutes, overridable via [`SemanticToolSelectionEvaluator::with_timeout`].
//! Both an inner [`JudgeError::Timeout`] and an outer timeout elapse map to
//! `Score::fail()` with timeout context surfaced in `EvalMetricResult.details`.
//!
//! # Opt-in
//!
//! The evaluator returns `None` when `EvalCase.semantic_tool_selection` is
//! `false`, or when the invocation contains no tool calls (FR-011). When no
//! [`JudgeClient`] is configured on the registry, the evaluator is simply not
//! registered — so consumers of [`EvaluatorRegistry::with_defaults`] see no
//! semantic tool-selection metric in results.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::evaluator::Evaluator;
use crate::judge::{JudgeClient, JudgeError, JudgeVerdict};
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation, RecordedToolCall};

/// Default outer timeout applied to each judge call (5 minutes, FR-014).
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

/// Semantic tool-selection evaluator backed by a [`JudgeClient`].
///
/// See the module-level docs for the full contract.
pub struct SemanticToolSelectionEvaluator {
    judge: Arc<dyn JudgeClient>,
    timeout: Duration,
}

impl SemanticToolSelectionEvaluator {
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

impl Evaluator for SemanticToolSelectionEvaluator {
    fn name(&self) -> &'static str {
        "semantic_tool_selection"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // Opt-in per-case — FR-011 / AS-5.3.
        if !case.semantic_tool_selection {
            return None;
        }

        // Empty trajectory → not applicable (spec Edge Cases list).
        let calls: Vec<(usize, &RecordedToolCall)> = invocation
            .turns
            .iter()
            .flat_map(|turn| turn.tool_calls.iter().map(move |tc| (turn.turn_index, tc)))
            .collect();
        if calls.is_empty() {
            return None;
        }

        let goal = goal_from_case(case);
        let tool_menu = available_tool_menu(invocation);

        // Judge each call. The Evaluator trait is sync; the async judge is
        // driven through whichever runtime context the caller provides:
        //
        // - Inside a multi-thread Tokio runtime (e.g. `#[tokio::main]`
        //   or `flavor = "multi_thread"`): use `block_in_place` + the
        //   ambient `Handle::block_on` so the active runtime keeps
        //   scheduling other tasks while this evaluator waits.
        // - Outside a Tokio runtime (or inside a current-thread flavor
        //   where `block_in_place` is unsupported): build an ephemeral
        //   current-thread runtime and `block_on` it.
        //
        // This keeps the evaluator usable from plain `#[test]` functions
        // and other sync callers without panicking on
        // `Handle::current()`.
        let outcomes: Vec<CallOutcome> = drive_judge_calls(|| async {
            let mut results = Vec::with_capacity(calls.len());
            let mut history = String::new();
            for (turn_index, call) in &calls {
                let prompt = build_prompt(&goal, &tool_menu, &history, *turn_index, call);
                let outcome = match timeout(self.timeout, self.judge.judge(&prompt)).await {
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
                        limit: self.timeout,
                    },
                };
                // Append this call to the running history view so later
                // judgements have full context.
                append_history(&mut history, *turn_index, call);
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
/// Tokio's `Handle::current()` panics when no runtime is active — the
/// pattern an earlier revision of this evaluator used. This helper picks
/// the right strategy at call time:
///
/// * Multi-thread runtime active → `block_in_place` + the ambient
///   `Handle::block_on` so the host runtime keeps scheduling other tasks
///   while we wait on the judge.
/// * No runtime → build an ephemeral current-thread runtime and
///   `block_on` it. Keeps the evaluator usable from plain `#[test]`
///   functions and other sync callers.
///
/// ## Known limitation
///
/// Running the evaluator from *inside* a single-threaded Tokio runtime
/// (e.g. `#[tokio::test(flavor = "current_thread")]` or a manually built
/// `new_current_thread` runtime) will panic with "Cannot start a runtime
/// from within a runtime". This is an inherent Tokio constraint, not a
/// bug in this helper — see
/// <https://docs.rs/tokio/latest/tokio/task/fn.block_in_place.html>. Use
/// a multi-thread runtime or call from plain sync context.
fn drive_judge_calls<F, Fut, T>(make_future: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    use tokio::runtime::{Handle, RuntimeFlavor};

    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(make_future()));
    }

    // No runtime active — build a short-lived one. `.expect` here is
    // unreachable in practice: runtime construction fails only on OS-level
    // resource exhaustion, at which point the evaluator cannot function.
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
/// * Otherwise → mean verdict score across successful judge calls (FR-014
///   / `contracts/public-api.md` §`SemanticToolSelectionEvaluator`).
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
        evaluator_name: "semantic_tool_selection".to_string(),
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

fn goal_from_case(case: &EvalCase) -> String {
    if case.user_messages.is_empty() {
        "(no user goal provided)".to_string()
    } else {
        case.user_messages.join("\n")
    }
}

/// Summarise the tools the agent actually used as a best-effort "menu".
///
/// Spec 023 does not expose the agent's registered tool set to evaluators —
/// the [`Invocation`] only contains the tools that were *called*. We surface
/// that set (unique by name) so the judge at least sees the tool universe the
/// agent drew from.
fn available_tool_menu(invocation: &Invocation) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for turn in &invocation.turns {
        for call in &turn.tool_calls {
            if !seen.contains(&call.name.as_str()) {
                seen.push(call.name.as_str());
            }
        }
    }
    if seen.is_empty() {
        "(none)".to_string()
    } else {
        seen.join(", ")
    }
}

fn append_history(buf: &mut String, turn_index: usize, call: &RecordedToolCall) {
    let args = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "<unserializable>".into());
    let name = &call.name;
    // Writing to a `String` is infallible.
    let _ = writeln!(buf, "- turn {turn_index}: {name}({args})");
}

fn build_prompt(
    goal: &str,
    tool_menu: &str,
    history: &str,
    turn_index: usize,
    call: &RecordedToolCall,
) -> String {
    let args = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "<unserializable>".into());
    let history_section = if history.is_empty() {
        "(no prior tool calls)".to_string()
    } else {
        history.to_string()
    };
    let name = &call.name;
    format!(
        "You are judging whether an agent's tool-selection decision was \
semantically appropriate.\n\n\
User goal:\n{goal}\n\n\
Tools the agent has been observed using on this run:\n{tool_menu}\n\n\
Session history so far:\n{history_section}\n\
Current tool call under review (turn {turn_index}):\n  {name}({args})\n\n\
Decide whether the chosen tool is an appropriate selection for advancing the \
user goal given the history. Respond with a Pass/Fail verdict and a short \
reason.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration as StdDuration;

    use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};

    use crate::testing::MockJudge;
    use crate::types::{EvalCase, Invocation, TurnRecord};

    fn simple_case() -> EvalCase {
        EvalCase {
            id: "c1".into(),
            name: "C1".into(),
            description: None,
            system_prompt: "be helpful".into(),
            user_messages: vec!["read the config".into()],
            expected_trajectory: None,
            expected_response: None,
            expected_assertion: None,
            expected_interactions: None,
            few_shot_examples: vec![],
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            attachments: vec![],
            session_id: None,
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: true,
            state_capture: None,
        }
    }

    fn invocation_with_calls(names: &[&str]) -> Invocation {
        let tool_calls: Vec<RecordedToolCall> = names
            .iter()
            .enumerate()
            .map(|(i, n)| RecordedToolCall {
                id: format!("id{i}"),
                name: (*n).to_string(),
                arguments: serde_json::json!({"k": i}),
            })
            .collect();
        Invocation {
            turns: vec![TurnRecord {
                turn_index: 0,
                assistant_message: AssistantMessage {
                    content: vec![ContentBlock::Text { text: "ok".into() }],
                    provider: "p".into(),
                    model_id: "m".into(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    error_kind: None,
                    timestamp: 0,
                    cache_hint: None,
                },
                tool_calls,
                tool_results: vec![],
                duration: StdDuration::from_millis(1),
            }],
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: StdDuration::from_millis(1),
            final_response: Some("done".into()),
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("p", "m"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_none_when_flag_disabled() {
        let mut case = simple_case();
        case.semantic_tool_selection = false;
        let invocation = invocation_with_calls(&["read_file"]);
        let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
        let evaluator = SemanticToolSelectionEvaluator::new(judge);
        assert!(evaluator.evaluate(&case, &invocation).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_none_when_trajectory_empty() {
        let case = simple_case();
        let mut invocation = invocation_with_calls(&[]);
        invocation.turns[0].tool_calls.clear();
        let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
        let evaluator = SemanticToolSelectionEvaluator::new(judge);
        assert!(evaluator.evaluate(&case, &invocation).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn default_timeout_is_five_minutes() {
        let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
        let evaluator = SemanticToolSelectionEvaluator::new(judge);
        assert_eq!(evaluator.timeout, Duration::from_mins(5));
    }
}
