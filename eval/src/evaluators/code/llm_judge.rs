//! Judge-backed code quality evaluator (T079).
//!
//! Dispatches through the shared [`dispatch_judge`](crate::dispatch_judge)
//! helper using the `code_llm_judge_v0` template registered on PR #818.

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::evaluators::{JudgeEvaluatorConfig, block_on, dispatch_judge, finish_metric_result};
use crate::prompt::{PromptContext, PromptTemplateRegistry};
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Judge-backed code quality evaluator (FR-017).
pub struct CodeLlmJudgeEvaluator {
    name: &'static str,
    config: JudgeEvaluatorConfig,
}

impl CodeLlmJudgeEvaluator {
    /// Create the evaluator bound to the given judge config.
    #[must_use]
    pub fn new(config: JudgeEvaluatorConfig) -> Self {
        Self {
            name: "code_llm_judge",
            config,
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    fn builtin_template() -> Arc<dyn crate::prompt::JudgePromptTemplate> {
        PromptTemplateRegistry::builtin()
            .get("code_llm_judge_v0")
            .expect("code_llm_judge_v0 registered on PR #818")
    }
}

impl Evaluator for CodeLlmJudgeEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // FR-020: no user message and no assistant response → criterion not set.
        if case.user_messages.is_empty() || invocation.final_response.is_none() {
            return None;
        }

        let builtin = Self::builtin_template();
        let ctx = PromptContext::new(Arc::new(case.clone()), Arc::new(invocation.clone()))
            .with_few_shot_examples(self.config.few_shot_examples.clone());

        let future = dispatch_judge(&self.config, builtin, &ctx);
        let outcome = match block_on(future) {
            Ok(outcome) => outcome,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("dispatch error: {err}")),
                });
            }
        };

        Some(finish_metric_result(self.name.to_string(), outcome))
    }
}
