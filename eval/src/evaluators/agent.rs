//! Agent/trajectory-family evaluators (T069, T070).
//!
//! Nine judge-backed evaluators score agent behavior across trajectory
//! accuracy, task completion, user satisfaction, tone, knowledge retention,
//! language detection, perceived error, and multi-agent interactions.
//!
//! * [`TrajectoryAccuracyEvaluator`] — trajectory quality without a reference
//!   (prompt: `trajectory_accuracy_v0`).
//! * [`TrajectoryAccuracyWithRefEvaluator`] — trajectory accuracy against an
//!   expected trajectory (prompt: `trajectory_accuracy_with_ref_v0`).
//! * [`TaskCompletionEvaluator`] — whether the declared assertion was met
//!   (prompt: `task_completion_v0`).
//! * [`UserSatisfactionEvaluator`] — projected user satisfaction
//!   (prompt: `user_satisfaction_v0`).
//! * [`AgentToneEvaluator`] — response tone quality (prompt: `agent_tone_v0`).
//! * [`KnowledgeRetentionEvaluator`] — context retention across turns
//!   (prompt: `knowledge_retention_v0`).
//! * [`LanguageDetectionEvaluator`] — language-match between prompt and response
//!   (prompt: `language_detection_v0`).
//! * [`PerceivedErrorEvaluator`] — user-visible error signals in the response
//!   (prompt: `perceived_error_v0`).
//! * [`InteractionsEvaluator`] — multi-agent interaction topology accuracy
//!   (prompt: `interactions_v0`).

#![forbid(unsafe_code)]
#![cfg(feature = "evaluator-agent")]

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

use super::{JudgeEvaluatorConfig, build_prompt_context, evaluate_with_builtin};

fn has_final_response(invocation: &Invocation) -> bool {
    invocation
        .final_response
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn has_user_prompt(case: &EvalCase) -> bool {
    !case.user_messages.is_empty()
}

/// Macro for single-rubric agent evaluators. Each evaluator's FR-020 criterion
/// is supplied as a closure; bodies dispatch via [`evaluate_with_builtin`].
macro_rules! agent_evaluator {
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

        impl $crate::evaluators::JudgeEvaluatorBuilder for $name {
            fn judge_config_mut(&mut self) -> &mut JudgeEvaluatorConfig {
                &mut self.config
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

agent_evaluator! {
    /// Trajectory accuracy without a reference trajectory
    /// (prompt: `trajectory_accuracy_v0`).
    ///
    /// Criterion: the case must have a user prompt and the invocation must
    /// have a non-empty final response.
    TrajectoryAccuracyEvaluator,
    "trajectory_accuracy",
    "trajectory_accuracy_v0",
    |case, invocation| has_user_prompt(case) && has_final_response(invocation)
}

agent_evaluator! {
    /// Trajectory accuracy against an expected reference trajectory
    /// (prompt: `trajectory_accuracy_with_ref_v0`).
    ///
    /// Criterion: the case must declare an `expected_trajectory`, have a user
    /// prompt, and the invocation must have a non-empty final response.
    TrajectoryAccuracyWithRefEvaluator,
    "trajectory_accuracy_with_ref",
    "trajectory_accuracy_with_ref_v0",
    |case, invocation| case.expected_trajectory.is_some()
        && has_user_prompt(case)
        && has_final_response(invocation)
}

agent_evaluator! {
    /// Task completion against a declared assertion
    /// (prompt: `task_completion_v0`).
    ///
    /// Criterion: the case must declare an `expected_assertion` and the
    /// invocation must have a non-empty final response.
    TaskCompletionEvaluator,
    "task_completion",
    "task_completion_v0",
    |case, invocation| case.expected_assertion.is_some() && has_final_response(invocation)
}

agent_evaluator! {
    /// Projected user satisfaction with the response
    /// (prompt: `user_satisfaction_v0`).
    ///
    /// Criterion: the case must have a user prompt and the invocation must
    /// have a non-empty final response.
    UserSatisfactionEvaluator,
    "user_satisfaction",
    "user_satisfaction_v0",
    |case, invocation| has_user_prompt(case) && has_final_response(invocation)
}

agent_evaluator! {
    /// Response tone quality — professional, helpful register
    /// (prompt: `agent_tone_v0`).
    ///
    /// Criterion: the invocation must have a non-empty final response.
    /// A user prompt is not required because tone is scored on the response
    /// alone.
    AgentToneEvaluator,
    "agent_tone",
    "agent_tone_v0",
    |_case, invocation| has_final_response(invocation)
}

agent_evaluator! {
    /// Knowledge retention across conversation turns
    /// (prompt: `knowledge_retention_v0`).
    ///
    /// Criterion: the case must have a user prompt and the invocation must
    /// have a non-empty final response.
    KnowledgeRetentionEvaluator,
    "knowledge_retention",
    "knowledge_retention_v0",
    |case, invocation| has_user_prompt(case) && has_final_response(invocation)
}

agent_evaluator! {
    /// Language-match between prompt and response
    /// (prompt: `language_detection_v0`).
    ///
    /// Criterion: the case must have a user prompt and the invocation must
    /// have a non-empty final response.
    LanguageDetectionEvaluator,
    "language_detection",
    "language_detection_v0",
    |case, invocation| has_user_prompt(case) && has_final_response(invocation)
}

agent_evaluator! {
    /// User-visible error signals in the response
    /// (prompt: `perceived_error_v0`).
    ///
    /// Criterion: the invocation must have a non-empty final response.
    /// A user prompt is not required because error signals are scored on the
    /// response alone.
    PerceivedErrorEvaluator,
    "perceived_error",
    "perceived_error_v0",
    |_case, invocation| has_final_response(invocation)
}

agent_evaluator! {
    /// Multi-agent interaction topology accuracy against declared expected
    /// interactions (prompt: `interactions_v0`).
    ///
    /// Criterion: the case must declare `expected_interactions`, have a user
    /// prompt, and the invocation must have a non-empty final response.
    InteractionsEvaluator,
    "interactions",
    "interactions_v0",
    |case, invocation| case.expected_interactions.is_some()
        && has_user_prompt(case)
        && has_final_response(invocation)
}
