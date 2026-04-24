//! Quality-family judge-backed evaluators (T058, T059).
//!
//! Each evaluator in this module:
//!
//! * Returns `None` from `evaluate` when its criterion is absent (FR-020).
//! * Dispatches through [`super::dispatch_judge`] using its built-in
//!   `_v0` prompt template (FR-011).
//! * Records `prompt_version` in `EvalMetricResult::details`.
//! * Defaults to the [`crate::Average`] aggregator at the family level (per
//!   data-model §3 quality family).
//!
//! The [`HallucinationEvaluator`] and [`FaithfulnessEvaluator`] intentionally
//! ship distinct rubrics (spec 043 clarification Q1):
//!
//! * `HallucinationEvaluator` scores against general world knowledge and the
//!   user prompt — retrieved context is NOT consulted.
//! * `FaithfulnessEvaluator` scores against the retrieved context supplied to
//!   the agent (few-shot examples or metadata); general knowledge is NOT
//!   consulted.

#![forbid(unsafe_code)]
#![cfg(feature = "evaluator-quality")]

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::types::{AssertionKind, EvalCase, EvalMetricResult, Invocation};

use super::{JudgeEvaluatorConfig, build_prompt_context, evaluate_with_builtin};

/// Macro to reduce boilerplate for single-rubric quality evaluators.
macro_rules! simple_quality_evaluator {
    (
        $(#[$meta:meta])*
        $name:ident, $eval_name:literal, $template:literal, $criterion:expr
    ) => {
        $(#[$meta])*
        pub struct $name {
            config: JudgeEvaluatorConfig,
        }

        impl $name {
            /// Construct with the supplied judge config.
            #[must_use]
            pub const fn new(config: JudgeEvaluatorConfig) -> Self {
                Self { config }
            }

            /// Override the prompt template used by this evaluator.
            #[must_use]
            pub fn with_prompt(mut self, template: Arc<dyn crate::prompt::JudgePromptTemplate>) -> Self {
                self.config = self.config.with_prompt(template);
                self
            }

            /// Attach evaluator-level few-shot examples that render before any
            /// case-level examples.
            #[must_use]
            pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
                self.config = self.config.with_few_shot(examples);
                self
            }

            /// Override the system prompt visible to the template render.
            #[must_use]
            pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
                self.config = self.config.with_system_prompt(prompt);
                self
            }

            /// Attach an output schema for custom prompt templates.
            #[must_use]
            pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
                self.config = self.config.with_output_schema(schema);
                self
            }

            /// Toggle judge reasoning capture.
            #[must_use]
            pub fn with_use_reasoning(mut self, flag: bool) -> Self {
                self.config = self.config.with_use_reasoning(flag);
                self
            }

            /// Override the feedback key used by downstream exporters.
            #[must_use]
            pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
                self.config = self.config.with_feedback_key(key);
                self
            }

            /// Borrow the underlying config (e.g., to inspect the judge
            /// registry or feedback key).
            #[must_use]
            pub const fn config(&self) -> &JudgeEvaluatorConfig {
                &self.config
            }
        }

        impl Evaluator for $name {
            fn name(&self) -> &'static str {
                $eval_name
            }

            fn evaluate(
                &self,
                case: &EvalCase,
                invocation: &Invocation,
            ) -> Option<EvalMetricResult> {
                // FR-020: return None when the criterion is absent.
                let criterion: fn(&EvalCase, &Invocation) -> bool = $criterion;
                if !criterion(case, invocation) {
                    return None;
                }

                Some(evaluate_with_builtin(
                    $eval_name,
                    $template,
                    &self.config,
                    &build_prompt_context(&self.config, case, invocation),
                ))
            }
        }
    };
}

fn has_final_response(_case: &EvalCase, invocation: &Invocation) -> bool {
    invocation
        .final_response
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn has_user_prompt_and_response(case: &EvalCase, invocation: &Invocation) -> bool {
    !case.user_messages.is_empty() && has_final_response(case, invocation)
}

fn has_system_prompt_and_response(case: &EvalCase, invocation: &Invocation) -> bool {
    !case.system_prompt.trim().is_empty() && has_final_response(case, invocation)
}

fn has_retrieved_context(case: &EvalCase, invocation: &Invocation) -> bool {
    has_final_response(case, invocation) && !case.few_shot_examples.is_empty()
}

simple_quality_evaluator! {
    /// Helpfulness on a 1-7 scale (prompt: `helpfulness_v0`).
    HelpfulnessEvaluator,
    "helpfulness",
    "helpfulness_v0",
    has_user_prompt_and_response
}

simple_quality_evaluator! {
    /// Factual correctness of the final response (prompt: `correctness_v0`).
    CorrectnessEvaluator,
    "correctness",
    "correctness_v0",
    has_user_prompt_and_response
}

simple_quality_evaluator! {
    /// Conciseness on a 3-level scale (prompt: `conciseness_v0`).
    ConcisenessEvaluator,
    "conciseness",
    "conciseness_v0",
    has_final_response
}

simple_quality_evaluator! {
    /// Coherence on a 5-level scale (prompt: `coherence_v0`).
    CoherenceEvaluator,
    "coherence",
    "coherence_v0",
    has_final_response
}

simple_quality_evaluator! {
    /// Response relevance to the user prompt (prompt: `response_relevance_v0`).
    ResponseRelevanceEvaluator,
    "response_relevance",
    "response_relevance_v0",
    has_user_prompt_and_response
}

simple_quality_evaluator! {
    /// Hallucination check against general knowledge (prompt:
    /// `hallucination_v0`). Distinct rubric from
    /// [`FaithfulnessEvaluator`]; retrieved context is NOT consulted here.
    HallucinationEvaluator,
    "hallucination",
    "hallucination_v0",
    has_user_prompt_and_response
}

simple_quality_evaluator! {
    /// Faithfulness to RETRIEVED context (prompt: `faithfulness_v0`).
    ///
    /// Distinct rubric from [`HallucinationEvaluator`]; requires `few_shot_examples`
    /// on the case (the spec's canonical retrieved-context surface).
    FaithfulnessEvaluator,
    "faithfulness",
    "faithfulness_v0",
    has_retrieved_context
}

simple_quality_evaluator! {
    /// Plan adherence relative to the system prompt (prompt:
    /// `plan_adherence_v0`).
    PlanAdherenceEvaluator,
    "plan_adherence",
    "plan_adherence_v0",
    has_system_prompt_and_response
}

simple_quality_evaluator! {
    /// Detect assistant laziness (prompt: `laziness_v0`).
    LazinessEvaluator,
    "laziness",
    "laziness_v0",
    has_user_prompt_and_response
}

/// Goal success against the declared `expected_assertion` (prompt:
/// `goal_success_rate_v0`).
///
/// Returns `None` when the case has no `expected_assertion` — `expected_assertion`
/// is this evaluator's criterion per spec 043 data-model §3 quality family.
pub struct GoalSuccessRateEvaluator {
    config: JudgeEvaluatorConfig,
}

impl GoalSuccessRateEvaluator {
    /// Construct with the supplied judge config.
    #[must_use]
    pub const fn new(config: JudgeEvaluatorConfig) -> Self {
        Self { config }
    }

    /// Override the prompt template used by this evaluator.
    #[must_use]
    pub fn with_prompt(mut self, template: Arc<dyn crate::prompt::JudgePromptTemplate>) -> Self {
        self.config = self.config.with_prompt(template);
        self
    }

    /// Attach evaluator-level few-shot examples that render before any
    /// case-level examples.
    #[must_use]
    pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
        self.config = self.config.with_few_shot(examples);
        self
    }

    /// Override the system prompt visible to the template render.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config = self.config.with_system_prompt(prompt);
        self
    }

    /// Attach an output schema for custom prompt templates.
    #[must_use]
    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.config = self.config.with_output_schema(schema);
        self
    }

    /// Toggle judge reasoning capture.
    #[must_use]
    pub fn with_use_reasoning(mut self, flag: bool) -> Self {
        self.config = self.config.with_use_reasoning(flag);
        self
    }

    /// Override the feedback key used by downstream exporters.
    #[must_use]
    pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
        self.config = self.config.with_feedback_key(key);
        self
    }

    /// Borrow the underlying config.
    #[must_use]
    pub const fn config(&self) -> &JudgeEvaluatorConfig {
        &self.config
    }
}

impl Evaluator for GoalSuccessRateEvaluator {
    fn name(&self) -> &'static str {
        "goal_success_rate"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // Criterion: expected_assertion must be present AND of a shape this
        // evaluator is intended for (goal-completion rubrics). Any assertion
        // kind qualifies; tests that want tighter coupling can filter with
        // `evaluators`.
        let _assertion = case.expected_assertion.as_ref()?;
        if !has_final_response(case, invocation) {
            return None;
        }

        Some(evaluate_with_builtin(
            "goal_success_rate",
            "goal_success_rate_v0",
            &self.config,
            &build_prompt_context(&self.config, case, invocation),
        ))
    }
}

/// Helper: true when the case declares a goal-completion flavoured assertion.
///
/// Exposed for downstream consumers that want to route cases between
/// `GoalSuccessRateEvaluator` and `TaskCompletionEvaluator`.
#[must_use]
pub fn assertion_implies_goal_completion(case: &EvalCase) -> bool {
    matches!(
        case.expected_assertion.as_ref().map(|a| &a.kind),
        Some(AssertionKind::GoalCompleted | AssertionKind::Custom { .. })
    )
}
