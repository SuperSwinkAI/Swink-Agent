//! Data types for evaluation cases, invocations, and results.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, ToolResultMessage, Usage};
use swink_agent_policies::{BudgetPolicy, MaxTurnsPolicy};

use crate::error::EvalError;
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

/// Named snapshot of an environment state produced by a [`StateCapture`].
///
/// Used with `EvalCase::expected_environment_state` to assert that after the
/// agent completes, the captured environment matches the expected values via
/// full JSON equality (FR-013, FR-015).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentState {
    /// Identifier for this state entry. Duplicate names within a single
    /// `expected_environment_state` are rejected at case-load time
    /// (FR-015, SC-009).
    pub name: String,
    /// Expected (or captured) JSON value; compared for full JSON equality.
    pub state: serde_json::Value,
}

/// Expected semantic tool intent used by the tool-parameter semantic evaluator.
///
/// When `tool_name` is `Some`, only tool calls whose name matches are judged;
/// other calls are skipped (not Pass, not Fail).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIntent {
    /// Natural-language description of what the tool call should accomplish.
    pub intent: String,
    /// When `Some`, restrict judging to tool calls with this exact name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Callback that captures the environment state after an agent run completes.
///
/// Registered programmatically on an [`EvalCase`] (or supplied by the
/// `AgentFactory`). The callback is invoked once after the agent finishes; its
/// output populates the "actual" side for the `EnvironmentStateEvaluator`.
///
/// Panics are caught by the evaluator and surfaced as `Score::fail()` with the
/// panic message (FR-014).
pub type StateCapture = Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>;

/// Budget constraints for cost and latency governance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConstraints {
    /// Maximum allowed cost in dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
    /// Maximum allowed input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input: Option<u64>,
    /// Maximum allowed output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// Maximum allowed number of turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

impl BudgetConstraints {
    /// Convert budget constraints into loop policies for agent construction.
    #[must_use]
    pub fn to_policies(&self) -> (Option<BudgetPolicy>, Option<MaxTurnsPolicy>) {
        let budget_policy =
            if self.max_cost.is_none() && self.max_input.is_none() && self.max_output.is_none() {
                None
            } else {
                let mut policy = BudgetPolicy::new();
                if let Some(max_cost) = self.max_cost {
                    policy = policy.max_cost(max_cost);
                }
                if let Some(max_input) = self.max_input {
                    policy = policy.max_input(max_input);
                }
                if let Some(max_output) = self.max_output {
                    policy = policy.max_output(max_output);
                }
                Some(policy)
            };

        let max_turns_policy = self.max_turns.map(MaxTurnsPolicy::new);

        (budget_policy, max_turns_policy)
    }
}

// ─── Eval Case & Set ────────────────────────────────────────────────────────

/// A single evaluation scenario.
///
/// Defines the agent prompt, expected outcomes, and which evaluators to run.
#[derive(Clone, Serialize, Deserialize)]
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
    /// Expected environment-state snapshots keyed by name (FR-013).
    ///
    /// Compared against the output of `state_capture` via full JSON equality.
    /// Duplicate names are rejected at case-load time (FR-015, SC-009).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_environment_state: Option<Vec<EnvironmentState>>,
    /// Expected semantic tool intent for the tool-parameter evaluator (FR-012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_tool_intent: Option<ToolIntent>,
    /// Enable semantic tool-selection scoring for this case (FR-011).
    #[serde(default, skip_serializing_if = "is_false")]
    pub semantic_tool_selection: bool,
    /// Callback that produces the actual environment state after the agent
    /// completes. Programmatic only — mirrors `ResponseCriteria::Custom`.
    #[serde(skip)]
    pub state_capture: Option<StateCapture>,
}

impl std::fmt::Debug for EvalCase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvalCase")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("system_prompt", &self.system_prompt)
            .field("user_messages", &self.user_messages)
            .field("expected_trajectory", &self.expected_trajectory)
            .field("expected_response", &self.expected_response)
            .field("budget", &self.budget)
            .field("evaluators", &self.evaluators)
            .field("metadata", &self.metadata)
            .field(
                "expected_environment_state",
                &self.expected_environment_state,
            )
            .field("expected_tool_intent", &self.expected_tool_intent)
            .field("semantic_tool_selection", &self.semantic_tool_selection)
            .field(
                "state_capture",
                &self.state_capture.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
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

// ─── Case-load Validation (FR-015, SC-009) ──────────────────────────────────

/// Validate a single [`EvalCase`] against the case-load rules.
///
/// Currently enforces:
///
/// * `expected_environment_state` — names MUST be unique. Duplicates are
///   rejected with [`EvalError::InvalidCase`] pointing at the offending name
///   (FR-015, SC-009).
///
/// This check is shared by [`validate_eval_set`] and the YAML loader so
/// programmatic constructors get the same guarantees as on-disk configs.
pub fn validate_eval_case(case: &EvalCase) -> Result<(), EvalError> {
    if let Some(states) = &case.expected_environment_state {
        let mut seen: HashSet<&str> = HashSet::with_capacity(states.len());
        for state in states {
            if !seen.insert(state.name.as_str()) {
                return Err(EvalError::invalid_case(format!(
                    "case `{case_id}`: duplicate expected_environment_state name `{name}`",
                    case_id = case.id,
                    name = state.name,
                )));
            }
        }
    }
    Ok(())
}

/// Validate an entire [`EvalSet`], short-circuiting on the first invalid case.
pub fn validate_eval_set(set: &EvalSet) -> Result<(), EvalError> {
    for case in &set.cases {
        validate_eval_case(case)?;
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn base_case(id: &str) -> EvalCase {
        EvalCase {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            system_prompt: String::new(),
            user_messages: vec!["hi".to_string()],
            expected_trajectory: None,
            expected_response: None,
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: false,
            state_capture: None,
        }
    }

    #[test]
    fn validate_accepts_unique_environment_state_names() {
        let mut case = base_case("c1");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 1}),
            },
            EnvironmentState {
                name: "beta".into(),
                state: serde_json::json!({"v": 2}),
            },
        ]);
        assert!(validate_eval_case(&case).is_ok());
    }

    #[test]
    fn validate_rejects_duplicate_environment_state_names() {
        let mut case = base_case("dup");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 1}),
            },
            EnvironmentState {
                name: "alpha".into(),
                state: serde_json::json!({"v": 2}),
            },
        ]);
        let err = validate_eval_case(&case).expect_err("duplicate should be rejected");
        match err {
            EvalError::InvalidCase { reason } => {
                assert!(reason.contains("alpha"), "reason: {reason}");
                assert!(reason.contains("dup"), "reason mentions case id: {reason}");
            }
            other => panic!("expected InvalidCase, got {other:?}"),
        }
    }

    #[test]
    fn validate_none_environment_state_is_ok() {
        let case = base_case("none");
        assert!(validate_eval_case(&case).is_ok());
    }

    #[test]
    fn validate_eval_set_propagates_case_errors() {
        let mut case = base_case("bad");
        case.expected_environment_state = Some(vec![
            EnvironmentState {
                name: "x".into(),
                state: serde_json::Value::Null,
            },
            EnvironmentState {
                name: "x".into(),
                state: serde_json::Value::Null,
            },
        ]);
        let set = EvalSet {
            id: "set".into(),
            name: "Set".into(),
            description: None,
            cases: vec![case],
        };
        assert!(validate_eval_set(&set).is_err());
    }

    #[test]
    fn environment_state_serde_round_trip() {
        let state = EnvironmentState {
            name: "db".into(),
            state: serde_json::json!({"rows": 3, "schema": "public"}),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: EnvironmentState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, state.name);
        assert_eq!(back.state, state.state);
    }

    #[test]
    fn eval_case_serde_round_trip_with_v2_fields() {
        let mut case = base_case("v2");
        case.expected_environment_state = Some(vec![EnvironmentState {
            name: "alpha".into(),
            state: serde_json::json!({"n": 1}),
        }]);
        case.expected_tool_intent = Some(ToolIntent {
            intent: "read config".into(),
            tool_name: Some("read_file".into()),
        });
        case.semantic_tool_selection = true;
        let yaml_like = serde_json::to_string(&case).unwrap();
        let back: EvalCase = serde_json::from_str(&yaml_like).unwrap();
        assert_eq!(back.expected_environment_state.as_ref().unwrap().len(), 1);
        assert_eq!(
            back.expected_tool_intent.as_ref().unwrap().intent,
            "read config"
        );
        assert!(back.semantic_tool_selection);
        assert!(back.state_capture.is_none());
    }
}

#[cfg(test)]
mod budget_policy_tests {
    use super::*;
    use swink_agent::{Cost, PolicyContext, PolicyVerdict, PreTurnPolicy, SessionState, Usage};

    fn make_ctx<'a>(turn_index: usize, usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
        let state = Box::leak(Box::new(SessionState::new()));
        PolicyContext {
            turn_index,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    #[test]
    fn budget_constraints_to_policies_none_when_unset() {
        let constraints = BudgetConstraints {
            max_cost: None,
            max_input: None,
            max_output: None,
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();

        assert!(budget_policy.is_none());
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_budget_only_for_cost() {
        let constraints = BudgetConstraints {
            max_cost: Some(1.0),
            max_input: None,
            max_output: None,
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage::default();
        let cost = Cost {
            total: 1.0,
            ..Default::default()
        };
        let ctx = make_ctx(0, &usage, &cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_budget_only_for_input_output() {
        let constraints = BudgetConstraints {
            max_cost: None,
            max_input: Some(10),
            max_output: Some(20),
            max_turns: None,
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage {
            input: 10,
            output: 20,
            total: 30,
            ..Default::default()
        };
        let cost = Cost::default();
        let ctx = make_ctx(0, &usage, &cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(max_turns_policy.is_none());
    }

    #[test]
    fn budget_constraints_to_policies_builds_both_policies_when_needed() {
        let constraints = BudgetConstraints {
            max_cost: Some(2.0),
            max_input: None,
            max_output: None,
            max_turns: Some(3),
        };

        let (budget_policy, max_turns_policy) = constraints.to_policies();
        let usage = Usage::default();
        let cost = Cost {
            total: 2.0,
            ..Default::default()
        };
        let budget_ctx = make_ctx(0, &usage, &cost);
        let turn_cost = Cost::default();
        let turn_ctx = make_ctx(3, &usage, &turn_cost);

        assert!(matches!(
            PreTurnPolicy::evaluate(&budget_policy.unwrap(), &budget_ctx),
            PolicyVerdict::Stop(_)
        ));
        assert!(matches!(
            PreTurnPolicy::evaluate(&max_turns_policy.unwrap(), &turn_ctx),
            PolicyVerdict::Stop(_)
        ));
    }
}
