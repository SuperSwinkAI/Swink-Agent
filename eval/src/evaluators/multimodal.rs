//! Multimodal-family evaluators (T085).
//!
//! Only `ImageSafetyEvaluator` ships in this spec — audio multimodal is
//! deferred per FR-019. The evaluator consumes `case.attachments` via the
//! shared [`materialize_case_attachments`](crate::materialize_case_attachments)
//! helper (T086) and dispatches through `dispatch_judge` with the
//! `image_safety_v0` template registered on PR #818.

use std::path::PathBuf;
use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::evaluators::{
    JudgeEvaluatorConfig, build_prompt_context, dispatch_judge, finish_metric_result,
    materialize_case_attachments,
};
use crate::prompt::PromptTemplateRegistry;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};
use crate::url_filter::{DefaultUrlFilter, UrlFilter};

/// Judge-backed image-safety evaluator (FR-019).
pub struct ImageSafetyEvaluator {
    name: &'static str,
    config: JudgeEvaluatorConfig,
    eval_set_root: PathBuf,
    url_filter: Arc<dyn UrlFilter>,
}

impl ImageSafetyEvaluator {
    /// Create the evaluator bound to the given judge config.
    ///
    /// `eval_set_root` is the directory used to resolve `Attachment::Path`
    /// entries; callers typically pass the on-disk root of the eval set.
    #[must_use]
    pub fn new(config: JudgeEvaluatorConfig, eval_set_root: impl Into<PathBuf>) -> Self {
        Self {
            name: "image_safety",
            config,
            eval_set_root: eval_set_root.into(),
            url_filter: Arc::new(DefaultUrlFilter),
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the URL filter used when materializing `Attachment::Url`.
    #[must_use]
    pub fn with_url_filter(mut self, filter: Arc<dyn UrlFilter>) -> Self {
        self.url_filter = filter;
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
}

impl Evaluator for ImageSafetyEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // FR-020: without attachments, the criterion isn't set.
        if case.attachments.is_empty() {
            return None;
        }

        let builtin = PromptTemplateRegistry::builtin()
            .get("image_safety_v0")
            .expect("image_safety_v0 registered on PR #818");

        let materialize =
            materialize_case_attachments(case, &self.eval_set_root, &*self.url_filter);
        let materialized = match crate::evaluators::block_on(materialize) {
            Ok(materialized) => materialized,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("attachment materialization failed: {err}")),
                });
            }
        };

        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "materialized_attachments".to_string(),
            serde_json::Value::Number(serde_json::Number::from(materialized.len())),
        );

        let ctx = build_prompt_context(&self.config, case, invocation).with_custom(custom);

        let outcome = match crate::evaluators::block_on(dispatch_judge(&self.config, builtin, &ctx))
        {
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
