//! Deterministic `cargo check` evaluator (T077 — cargo-check portion).
//!
//! Extracts code from the assistant response using a configured
//! [`CodeExtractor`], writes it to a tempdir, and shells out to
//! `cargo check --message-format=short`. The exit status drives the score;
//! captured stderr is surfaced in `details` on failure.

use std::process::Command;
use std::sync::Arc;

use tempfile::TempDir;

use crate::evaluator::Evaluator;
use crate::evaluators::code::extractor::CodeExtractor;
use crate::evaluators::code::harness::{CargoHarness, CargoOutcome};
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Deterministic `cargo check` evaluator (FR-017).
pub struct CargoCheckEvaluator {
    name: &'static str,
    extractor: Arc<CodeExtractor>,
    harness: CargoHarness,
}

impl CargoCheckEvaluator {
    /// Create a new evaluator with the given extractor strategy.
    #[must_use]
    pub fn new(extractor: Arc<CodeExtractor>) -> Self {
        Self {
            name: "cargo_check",
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

impl Evaluator for CargoCheckEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let response = invocation.final_response.as_ref()?;
        let code = crate::evaluators::block_on(self.extractor.extract(response))?;

        let outcome = run_cargo(&self.harness, &code, &["check", "--message-format=short"]);
        Some(outcome.into_metric_result(self.name))
    }
}

/// Shell out to `cargo` with the configured arguments.
pub(super) fn run_cargo(harness: &CargoHarness, code: &str, args: &[&str]) -> CargoOutcome {
    let tempdir = match TempDir::new() {
        Ok(dir) => dir,
        Err(err) => {
            return CargoOutcome {
                success: false,
                message: format!("tempdir creation failed: {err}"),
            };
        }
    };

    if let Err(err) = harness.scaffold(tempdir.path(), code) {
        return CargoOutcome {
            success: false,
            message: format!("scaffold failed: {err}"),
        };
    }

    let mut command = Command::new(&harness.cargo_bin);
    command.arg("--offline").args(args);
    command.current_dir(tempdir.path());
    command.env("CARGO_TARGET_DIR", tempdir.path().join("target"));

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            return CargoOutcome {
                success: false,
                message: format!("cargo spawn failed: {err}"),
            };
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    CargoOutcome {
        success: output.status.success(),
        message: if output.status.success() {
            String::from("ok")
        } else {
            stderr
                .lines()
                .filter(|line| !line.trim().is_empty())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        },
    }
}
