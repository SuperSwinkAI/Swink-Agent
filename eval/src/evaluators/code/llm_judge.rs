//! Judge-backed code quality evaluator (T079).
//!
//! Dispatches through the shared [`dispatch_judge`](crate::dispatch_judge)
//! helper using the `code_llm_judge_v0` template registered on PR #818.

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::evaluators::{
    JudgeEvaluatorConfig, block_on, build_prompt_context, dispatch_judge, finish_metric_result,
};
use crate::prompt::PromptTemplateRegistry;
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
        let ctx = build_prompt_context(&self.config, case, invocation);

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
