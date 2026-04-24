//! Code-family evaluators (T077–T079 — code family).
//!
//! Public surface:
//! * [`CodeExtractor`] + [`CodeExtractorStrategy`] — strategy object that lifts
//!   code from an assistant response (markdown fence / regex / LLM). Shared by
//!   every code evaluator so extraction logic lives in exactly one place.
//! * [`CargoCheckEvaluator`] / [`ClippyEvaluator`] — deterministic evaluators
//!   that shell out to `cargo check` / `cargo clippy` in a tempdir.
//! * [`llm_judge::CodeLlmJudgeEvaluator`] — judge-backed evaluator using the
//!   `code_llm_judge_v0` template.
//!
//! `SandboxedExecutionEvaluator` (T080–T083, behind `evaluator-sandbox`)
//! lives in the [`sandbox`] submodule. The module compiles unconditionally
//! but its implementation forks per-platform: Unix uses POSIX `rlimit`s
//! (module-scoped `#![allow(unsafe_code)]` per FR-049) and Windows returns
//! [`crate::EvaluatorError::UnsupportedPlatform`] at evaluation time.

pub mod cargo_check;
pub mod clippy;
pub mod extractor;
pub(crate) mod harness;
pub mod llm_judge;
#[cfg(feature = "evaluator-sandbox")]
pub mod sandbox;

pub use cargo_check::CargoCheckEvaluator;
pub use clippy::ClippyEvaluator;
pub use extractor::{CodeExtractor, CodeExtractorStrategy};
#[cfg(feature = "evaluator-sandbox")]
pub use sandbox::{
    SandboxLimits, SandboxOutcome, SandboxRunner, SandboxedExecutionEvaluator, ShellRunner,
    run_sandboxed,
};
