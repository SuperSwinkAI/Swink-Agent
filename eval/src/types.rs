//! Data types for evaluation cases, invocations, and results.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, ToolResultMessage, Usage};

use crate::score::{Score, Verdict};

// ─── Recorded Data ──────────────────────────────────────────────────────────

/// A tool call as captured from the agent event stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedToolCall {
    /// Provider-assigned tool call ID.
    pub id: String,
    /// Name of the tool that was invoked.
    pub name: String,
    /// Parsed JSON arguments passed to the tool.
    pub arguments: serde_json::Value,
}

/// A single recorded turn from an agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    /// Zero-based index of this turn within the run.
    pub turn_index: usize,
    /// The assistant message produced during this turn.
    pub assistant_message: AssistantMessage,
    /// Tool calls made during this turn (in execution order).
    pub tool_calls: Vec<RecordedToolCall>,
    /// Tool results returned during this turn.
    pub tool_results: Vec<ToolResultMessage>,
    /// Wall-clock duration of this turn.
    pub duration: Duration,
}

/// Complete trace of an agent run, built by [`TrajectoryCollector`](crate::TrajectoryCollector).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    /// All turns in execution order.
    pub turns: Vec<TurnRecord>,
    /// Aggregated token usage across all turns.
    pub total_usage: Usage,
    /// Aggregated cost across all turns.
    pub total_cost: Cost,
    /// Wall-clock duration of the entire run.
    pub total_duration: Duration,
    /// Extracted text from the final assistant message, if any.
    pub final_response: Option<String>,
    /// Stop reason from the final turn.
    pub stop_reason: StopReason,
    /// Model used for this run.
    pub model: ModelSpec,
}

// ─── Expected Data ──────────────────────────────────────────────────────────

/// A single expected tool invocation in a golden path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedToolCall {
    /// The tool name that should be called.
    pub tool_name: String,
    /// If present, the arguments must match exactly (JSON equality).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Criteria for matching the final response text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ResponseCriteria {
    /// Response must match exactly.
    Exact { expected: String },
    /// Response must contain the given substring.
    Contains { substring: String },
    /// Response must match the given regex pattern.
    Regex { pattern: String },
    /// Custom scoring function (not serializable — set programmatically).
    #[serde(skip)]
    Custom(#[serde(skip)] Arc<dyn Fn(&str) -> Score + Send + Sync>),
}

impl std::fmt::Debug for ResponseCriteria {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact { expected } => {
                f.debug_struct("Exact").field("expected", expected).finish()
            }
            Self::Contains { substring } => f
                .debug_struct("Contains")
                .field("substring", substring)
                .finish(),
            Self::Regex { pattern } => f.debug_struct("Regex").field("pattern", pattern).finish(),
            Self::Custom(_) => f.debug_tuple("Custom").field(&"<fn>").finish(),
        }
    }
}

/// Budget constraints for cost and latency governance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConstraints {
    /// Maximum allowed cost in dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
    /// Maximum allowed total tokens (input + output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Maximum allowed number of turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// Maximum allowed wall-clock duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Duration>,
}

// ─── Eval Case & Set ────────────────────────────────────────────────────────

/// A single evaluation scenario.
///
/// Defines the agent prompt, expected outcomes, and which evaluators to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Unique identifier for this case.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description of what this case tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// System prompt for the agent.
    pub system_prompt: String,
    /// Initial user messages (the prompt).
    pub user_messages: Vec<String>,
    /// Expected tool call trajectory (golden path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_trajectory: Option<Vec<ExpectedToolCall>>,
    /// Expected final response criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_response: Option<ResponseCriteria>,
    /// Cost/budget governance constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetConstraints>,
    /// Names of evaluators to run. Empty means all registered evaluators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluators: Vec<String>,
    /// Arbitrary metadata for user-defined extensions and filtering.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

/// A named collection of evaluation cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSet {
    /// Unique identifier for this set.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The cases in this set.
    pub cases: Vec<EvalCase>,
}

// ─── Results ────────────────────────────────────────────────────────────────

/// Per-evaluator result for a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMetricResult {
    /// Name of the evaluator that produced this result.
    pub evaluator_name: String,
    /// The numeric score.
    pub score: Score,
    /// Optional human-readable details about the scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Result of evaluating a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCaseResult {
    /// The case ID that was evaluated.
    pub case_id: String,
    /// The captured invocation trace.
    pub invocation: Invocation,
    /// Per-evaluator metric results.
    pub metric_results: Vec<EvalMetricResult>,
    /// Overall verdict (all metrics must pass).
    pub verdict: Verdict,
}

/// Result of evaluating an entire eval set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSetResult {
    /// The eval set ID.
    pub eval_set_id: String,
    /// Per-case results.
    pub case_results: Vec<EvalCaseResult>,
    /// Aggregated summary statistics.
    pub summary: EvalSummary,
    /// Unix timestamp when this result was produced.
    pub timestamp: u64,
}

/// Aggregated statistics for an eval set run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    /// Total number of cases evaluated.
    pub total_cases: usize,
    /// Number of cases that passed all metrics.
    pub passed: usize,
    /// Number of cases that failed at least one metric.
    pub failed: usize,
    /// Aggregated cost across all cases.
    pub total_cost: Cost,
    /// Aggregated token usage across all cases.
    pub total_usage: Usage,
    /// Total wall-clock duration across all cases.
    pub total_duration: Duration,
}
