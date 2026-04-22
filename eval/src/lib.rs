#![forbid(unsafe_code)]
//! Evaluation framework for swink-agent.
//!
//! Provides trajectory tracing, golden path verification, response matching,
//! and cost/latency governance for LLM-powered agent loops.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use swink_agent_eval::{EvalRunner, EvalSet, EvalCase};
//!
//! let set = EvalSet { id: "demo".into(), name: "Demo".into(), description: None, cases: vec![] };
//! let runner = EvalRunner::with_defaults();
//! let result = runner.run_set(&set, &my_factory).await?;
//! println!("Passed: {}/{}", result.summary.passed, result.summary.total_cases);
//! ```

mod audit;
mod budget;
mod environment_state;
mod efficiency;
mod error;
mod evaluator;
mod gate;
mod judge;
mod match_;
mod response;
mod runner;
mod score;
mod semantic_tool_parameter;
mod semantic_tool_selection;
mod store;
pub mod testing;
mod trajectory;
mod types;
#[cfg(feature = "yaml")]
mod yaml;

// ─── Public API ─────────────────────────────────────────────────────────────

pub use audit::AuditedInvocation;
pub use budget::BudgetEvaluator;
pub use environment_state::EnvironmentStateEvaluator;
pub use efficiency::EfficiencyEvaluator;
pub use error::EvalError;
pub use evaluator::{Evaluator, EvaluatorRegistry};
pub use gate::{GateConfig, GateResult, check_gate};
pub use judge::{JudgeClient, JudgeError, JudgeVerdict};
pub use match_::{MatchMode, TrajectoryMatcher};
pub use response::ResponseMatcher;
pub use runner::{AgentFactory, EvalRunner};
pub use score::{Score, Verdict};
pub use semantic_tool_parameter::SemanticToolParameterEvaluator;
pub use semantic_tool_selection::SemanticToolSelectionEvaluator;
pub use store::{EvalStore, FsEvalStore};
pub use testing::{MockJudge, SlowMockJudge};
pub use trajectory::TrajectoryCollector;
pub use types::{
    BudgetConstraints, EnvironmentState, EvalCase, EvalCaseResult, EvalMetricResult, EvalSet,
    EvalSetResult, EvalSummary, ExpectedToolCall, Invocation, RecordedToolCall, ResponseCriteria,
    StateCapture, ToolIntent, TurnRecord, validate_eval_case, validate_eval_set,
};
#[cfg(feature = "yaml")]
pub use yaml::load_eval_set_yaml;
