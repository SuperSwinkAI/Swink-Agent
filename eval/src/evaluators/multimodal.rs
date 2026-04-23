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
    JudgeEvaluatorConfig, dispatch_judge, finish_metric_result, materialize_case_attachments,
};
use crate::prompt::{PromptContext, PromptTemplateRegistry};
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

        let ctx = PromptContext::new(Arc::new(case.clone()), Arc::new(invocation.clone()))
            .with_few_shot_examples(self.config.few_shot_examples.clone())
            .with_custom(custom);

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
