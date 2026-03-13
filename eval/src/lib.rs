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

mod budget;
mod error;
mod evaluator;
mod match_;
mod response;
mod runner;
mod score;
mod store;
mod trajectory;
mod types;

// ─── Public API ─────────────────────────────────────────────────────────────

pub use budget::BudgetEvaluator;
pub use error::EvalError;
pub use evaluator::{Evaluator, EvaluatorRegistry};
pub use match_::{MatchMode, TrajectoryMatcher};
pub use response::ResponseMatcher;
pub use runner::{AgentFactory, EvalRunner};
pub use score::{Score, Verdict};
pub use store::{EvalStore, FsEvalStore};
pub use trajectory::TrajectoryCollector;
pub use types::{
    BudgetConstraints, EvalCase, EvalCaseResult, EvalMetricResult, EvalSet, EvalSetResult,
    EvalSummary, ExpectedToolCall, Invocation, RecordedToolCall, ResponseCriteria, TurnRecord,
};
