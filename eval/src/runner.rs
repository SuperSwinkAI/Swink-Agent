//! Evaluation runner that orchestrates the full eval pipeline.
//!
//! The runner creates agents via an [`AgentFactory`], captures their execution
//! trajectories, scores them with an [`EvaluatorRegistry`], and aggregates results.

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use swink_agent::{Agent, AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage, UserMessage};

use crate::error::EvalError;
use crate::evaluator::EvaluatorRegistry;
use crate::score::{Score, Verdict};
use crate::trajectory::{BudgetGuard, TrajectoryCollector};
use crate::types::{
    EvalCase, EvalCaseResult, EvalMetricResult, EvalSet, EvalSetResult, EvalSummary, Invocation,
    TurnRecord,
};

/// Factory that creates a configured [`Agent`] for each eval case.
///
/// Decouples the runner from agent construction so the caller controls
/// model selection, tools, and system prompt.
pub trait AgentFactory: Send + Sync {
    /// Create an agent and cancellation token for the given eval case.
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError>;
}

/// Orchestrates evaluation: runs agents, captures trajectories, and scores results.
pub struct EvalRunner {
    registry: EvaluatorRegistry,
}

impl EvalRunner {
    /// Create a runner with a custom evaluator registry.
    #[must_use]
    pub const fn new(registry: EvaluatorRegistry) -> Self {
        Self { registry }
    }

    /// Create a runner pre-loaded with built-in evaluators.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(EvaluatorRegistry::with_defaults())
    }

    /// Run a single eval case and return the scored result.
    pub async fn run_case(
        &self,
        case: &EvalCase,
        factory: &dyn AgentFactory,
    ) -> Result<EvalCaseResult, EvalError> {
        info!(case_id = %case.id, case_name = %case.name, "running eval case");

        let (mut agent, cancel) = factory.create_agent(case)?;

        // Build user messages from the case.
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

        // Run the agent and collect the trajectory with budget guarding.
        let stream = agent.prompt_stream(messages)?;
        let guard = BudgetGuard::from_case(case, cancel);
        let invocation = TrajectoryCollector::collect_with_guard(stream, guard).await;

        // Score.
        let metric_results = self.registry.evaluate(case, &invocation);
        let verdict = if metric_results.iter().all(|r| r.score.verdict().is_pass()) {
            Verdict::Pass
        } else {
            Verdict::Fail
        };

        info!(
            case_id = %case.id,
            verdict = ?verdict,
            metrics = metric_results.len(),
            "eval case complete"
        );

        Ok(EvalCaseResult {
            case_id: case.id.clone(),
            invocation,
            metric_results,
            verdict,
        })
    }

    /// Run an entire eval set and return aggregated results.
    pub async fn run_set(
        &self,
        eval_set: &EvalSet,
        factory: &dyn AgentFactory,
    ) -> Result<EvalSetResult, EvalError> {
        info!(
            set_id = %eval_set.id,
            cases = eval_set.cases.len(),
            "running eval set"
        );

        let mut case_results = Vec::with_capacity(eval_set.cases.len());
        let mut total_cost = Cost::default();
        let mut total_usage = Usage::default();
        let mut total_duration = std::time::Duration::ZERO;
        let mut passed = 0usize;
        let mut failed = 0usize;

        for case in &eval_set.cases {
            match self.run_case(case, factory).await {
                Ok(result) => {
                    total_cost += result.invocation.total_cost.clone();
                    total_usage += result.invocation.total_usage.clone();
                    total_duration += result.invocation.total_duration;
                    if result.verdict.is_pass() {
                        passed += 1;
                    } else {
                        failed += 1;
                    }
                    case_results.push(result);
                }
                Err(e) => {
                    warn!(case_id = %case.id, error = %e, "eval case failed with error — recording failure and continuing");
                    failed += 1;
                    case_results.push(EvalCaseResult {
                        case_id: case.id.clone(),
                        invocation: Invocation {
                            turns: vec![TurnRecord {
                                turn_index: 0,
                                assistant_message: AssistantMessage {
                                    content: vec![],
                                    provider: String::new(),
                                    model_id: String::new(),
                                    usage: Usage::default(),
                                    cost: Cost::default(),
                                    stop_reason: StopReason::Error,
                                    error_message: Some(e.to_string()),
                                    timestamp: swink_agent::now_timestamp(),
                                    cache_hint: None,
                                },
                                tool_calls: vec![],
                                tool_results: vec![],
                                duration: std::time::Duration::ZERO,
                            }],
                            total_usage: Usage::default(),
                            total_cost: Cost::default(),
                            total_duration: std::time::Duration::ZERO,
                            final_response: None,
                            stop_reason: StopReason::Error,
                            model: ModelSpec::new("unknown", "unknown"),
                        },
                        metric_results: vec![EvalMetricResult {
                            evaluator_name: "error".to_string(),
                            score: Score::fail(),
                            details: Some(e.to_string()),
                        }],
                        verdict: Verdict::Fail,
                    });
                }
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
            set_id = %eval_set.id,
            passed = summary.passed,
            failed = summary.failed,
            total = summary.total_cases,
            "eval set complete"
        );

        Ok(EvalSetResult {
            eval_set_id: eval_set.id.clone(),
            case_results,
            summary,
            timestamp: swink_agent::now_timestamp(),
        })
    }
}
