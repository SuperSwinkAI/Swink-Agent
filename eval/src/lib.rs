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

pub mod aggregator;
mod audit;
mod budget;
pub mod cache;
mod efficiency;
mod environment_state;
mod error;
mod evaluator;
#[cfg(feature = "judge-core")]
pub mod evaluators;
mod gate;
#[cfg(feature = "generation")]
pub mod generation;
pub mod judge;
mod match_;
#[cfg(feature = "judge-core")]
pub mod prompt;
pub mod report;
mod response;
mod runner;
mod score;
mod semantic_tool_parameter;
mod semantic_tool_selection;
#[cfg(feature = "simulation")]
pub mod simulation;
mod store;
#[cfg(feature = "telemetry")]
pub mod telemetry;
pub mod testing;
#[cfg(feature = "trace-ingest")]
pub mod trace;
mod trajectory;
mod types;
mod url_filter;
#[cfg(feature = "yaml")]
mod yaml;

// ─── Public API ─────────────────────────────────────────────────────────────

pub use aggregator::{Aggregator, AllPass, AnyPass, Average, Weighted};
pub use audit::AuditedInvocation;
pub use budget::BudgetEvaluator;
pub use cache::{
    CacheKey as TaskResultCacheKey, EvaluationDataStore, FingerprintContext,
    LocalFileTaskResultStore, StoreError, canonicalize_fingerprint, tool_set_hash,
};
pub use efficiency::EfficiencyEvaluator;
pub use environment_state::EnvironmentStateEvaluator;
pub use error::EvalError;
pub use evaluator::{Evaluator, EvaluatorRegistry};
#[cfg(feature = "evaluator-code")]
pub use evaluators::code::llm_judge::CodeLlmJudgeEvaluator;
#[cfg(feature = "evaluator-code")]
pub use evaluators::code::{
    CargoCheckEvaluator, ClippyEvaluator, CodeExtractor, CodeExtractorStrategy,
};
#[cfg(feature = "multimodal")]
pub use evaluators::multimodal::ImageSafetyEvaluator;
#[cfg(feature = "evaluator-simple")]
pub use evaluators::simple::{ExactMatchEvaluator, LevenshteinDistanceEvaluator};
#[cfg(feature = "evaluator-structured")]
pub use evaluators::structured::{JsonMatchEvaluator, JsonSchemaEvaluator, KeyStrategy};
#[cfg(feature = "judge-core")]
pub use evaluators::{
    Detail, DetailBuffer, DispatchError, DispatchOutcome, EvaluatorError, JudgeEvaluatorConfig,
    dispatch_judge, drive_judge_call, evaluate_with_builtin, finish_metric_result,
    materialize_case_attachments,
};

#[cfg(feature = "evaluator-quality")]
pub use evaluators::quality::{
    CoherenceEvaluator, ConcisenessEvaluator, CorrectnessEvaluator, FaithfulnessEvaluator,
    GoalSuccessRateEvaluator, HallucinationEvaluator, HelpfulnessEvaluator, LazinessEvaluator,
    PlanAdherenceEvaluator, ResponseRelevanceEvaluator, assertion_implies_goal_completion,
};
#[cfg(feature = "evaluator-safety")]
pub use evaluators::safety::{
    CodeInjectionEvaluator, FairnessEvaluator, HarmfulnessEvaluator, PIIClass, PIILeakageEvaluator,
    PromptInjectionEvaluator, ToxicityEvaluator,
};
pub use gate::{GateConfig, GateResult, check_gate};
pub use judge::{
    CacheKey, DEFAULT_JUDGE_CACHE_CAPACITY, JudgeCache, JudgeClient, JudgeError, JudgeRegistry,
    JudgeRegistryBuilder, JudgeRegistryError, JudgeVerdict, RetryPolicy,
};
pub use match_::{MatchMode, TrajectoryMatcher};
#[cfg(feature = "judge-core")]
pub use prompt::{
    BUILTIN_TEMPLATE_VERSIONS, JudgePromptTemplate, MinijinjaTemplate, PromptContext, PromptError,
    PromptFamily, PromptTemplateRegistry,
};
pub use report::{
    ConsoleReporter, JsonReporter, MarkdownReporter, Reporter, ReporterError, ReporterOutput,
};
pub use response::ResponseMatcher;
pub use runner::{AgentFactory, EvalRunner, RunnerMetricSample};
pub use score::{Score, Verdict};
pub use semantic_tool_parameter::SemanticToolParameterEvaluator;
pub use semantic_tool_selection::SemanticToolSelectionEvaluator;
pub use store::{EvalStore, FsEvalStore};
#[cfg(feature = "telemetry")]
pub use telemetry::{EvalsTelemetry, EvalsTelemetryBuilder};
pub use testing::{MockJudge, PanickingMockJudge, SlowMockJudge};
pub use trajectory::TrajectoryCollector;
pub use types::{
    Assertion, AssertionKind, Attachment, AttachmentError, BudgetConstraints, CASE_NAMESPACE,
    CaseFingerprint, EnvironmentState, EvalCase, EvalCaseResult, EvalMetricResult, EvalSet,
    EvalSetResult, EvalSummary, ExpectedToolCall, FewShotExample, InteractionExpectation,
    Invocation, MaterializedAttachment, RecordedToolCall, ResponseCriteria, StateCapture,
    ToolIntent, TurnRecord, validate_eval_case, validate_eval_set,
};
pub use url_filter::{DefaultUrlFilter, UrlFilter};
#[cfg(feature = "yaml")]
pub use yaml::load_eval_set_yaml;
