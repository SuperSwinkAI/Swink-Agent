//! Shared cargo-harness utilities for the code family (T077 support module).
//!
//! Scaffolds a minimal Rust crate on disk so deterministic evaluators like
//! [`super::cargo_check::CargoCheckEvaluator`] and [`super::clippy::ClippyEvaluator`]
//! can shell out to `cargo` without assuming any ambient filesystem shape.

use std::fs;
use std::io;
use std::path::Path;

use crate::score::Score;
use crate::types::EvalMetricResult;

/// How a harness writes an extracted snippet to disk before invoking cargo.
#[derive(Debug, Clone)]
pub struct CargoHarness {
    /// Absolute or PATH-resolvable cargo binary name.
    pub cargo_bin: String,
    /// Crate name used when scaffolding `Cargo.toml`.
    pub crate_name: String,
}

impl Default for CargoHarness {
    fn default() -> Self {
        Self {
            cargo_bin: "cargo".to_string(),
            crate_name: "swink_agent_eval_snippet".to_string(),
        }
    }
}

impl CargoHarness {
    /// Scaffold a minimal `[lib]` crate at `root` containing `code` as the lib body.
    pub fn scaffold(&self, root: &Path, code: &str) -> io::Result<()> {
        let src = root.join("src");
        fs::create_dir_all(&src)?;
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
                name = self.crate_name,
            ),
        )?;
        fs::write(src.join("lib.rs"), code)?;
        Ok(())
    }
}

/// Outcome returned by `cargo` shell-outs.
pub struct CargoOutcome {
    pub success: bool,
    pub message: String,
}

impl CargoOutcome {
    pub fn into_metric_result(self, evaluator_name: &'static str) -> EvalMetricResult {
        EvalMetricResult {
            evaluator_name: evaluator_name.to_string(),
            score: if self.success {
                Score::pass()
            } else {
                Score::fail()
            },
            details: Some(self.message),
        }
    }
}
