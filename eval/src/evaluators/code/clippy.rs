//! Deterministic `cargo clippy` evaluator (T077 — clippy portion).
//!
//! Mirrors [`super::cargo_check::CargoCheckEvaluator`] but invokes
//! `cargo clippy -- -D warnings` so any warning fails the metric.

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::evaluators::code::cargo_check::run_cargo;
use crate::evaluators::code::extractor::CodeExtractor;
use crate::evaluators::code::harness::CargoHarness;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Deterministic `cargo clippy` evaluator (FR-017).
pub struct ClippyEvaluator {
    name: &'static str,
    extractor: Arc<CodeExtractor>,
    harness: CargoHarness,
}

impl ClippyEvaluator {
    /// Create a new evaluator with the given extractor strategy.
    #[must_use]
    pub fn new(extractor: Arc<CodeExtractor>) -> Self {
        Self {
            name: "clippy",
            extractor,
            harness: CargoHarness::default(),
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the harness used to scaffold the tempdir project.
    #[must_use]
    pub fn with_harness(mut self, harness: CargoHarness) -> Self {
        self.harness = harness;
        self
    }
}

impl Evaluator for ClippyEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let response = invocation.final_response.as_ref()?;
        let code = crate::evaluators::block_on(self.extractor.extract(response))?;

        let outcome = run_cargo(&self.harness, &code, &["clippy", "--", "-D", "warnings"]);
        Some(outcome.into_metric_result(self.name))
    }
}
