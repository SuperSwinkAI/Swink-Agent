//! Reporters and export surfaces for eval results.
//!
//! Spec 043 §FR-041 requires always-on, plain-text reporters that render an
//! [`EvalSetResult`] to stdout strings or artifact bytes. The `Reporter` trait
//! is the common surface; concrete implementations live in sibling modules:
//!
//! * [`ConsoleReporter`] — plain-text, line-oriented (no ANSI, no cursor
//!   control, no interactivity per Q8 clarification).
//! * [`JsonReporter`] — self-contained JSON matching
//!   `specs/043-evals-adv-features/contracts/eval-result.schema.json`.
//! * [`MarkdownReporter`] — PR-comment-ready Markdown table.
//!
//! HTML and LangSmith exporters (behind their respective feature flags) follow
//! in later tasks.
//!
//! All reporters are deterministic: given the same `EvalSetResult` they
//! produce byte-identical output.
//!
//! [`EvalSetResult`]: crate::EvalSetResult

use std::path::PathBuf;

use thiserror::Error;

use crate::EvalSetResult;

pub mod console;
pub mod json;
pub mod markdown;

pub use console::ConsoleReporter;
pub use json::{JsonReporter, SCHEMA_VERSION};
pub use markdown::MarkdownReporter;

/// Stable JSON schema path shipped alongside spec 043.
///
/// Consumers that want to validate a reporter's JSON output against the
/// published schema can `include_str!` this file; the `JsonReporter`
/// regression tests do exactly that.
pub const JSON_SCHEMA_PATH: &str = "specs/043-evals-adv-features/contracts/eval-result.schema.json";

/// Renders an [`EvalSetResult`] into a concrete output surface.
///
/// Per spec 043 §FR-041 the three always-on reporters
/// ([`ConsoleReporter`], [`JsonReporter`], [`MarkdownReporter`]) are plain,
/// deterministic, and side-effect-free. Reporters that target a remote
/// backend (e.g. LangSmith) use [`ReporterOutput::Remote`] and may perform
/// network I/O; consult each reporter's documentation.
///
/// [`EvalSetResult`]: crate::EvalSetResult
pub trait Reporter: Send + Sync {
    /// Render the given result.
    ///
    /// # Errors
    ///
    /// Returns [`ReporterError`] when formatting fails, when an artifact
    /// cannot be written, or when a remote backend rejects the payload.
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError>;
}

/// Output produced by a [`Reporter::render`] call.
///
/// The three variants cover the common delivery channels:
/// * [`Stdout`](Self::Stdout) — text intended for terminal display.
/// * [`Artifact`](Self::Artifact) — bytes + filesystem path the caller may
///   persist (e.g. `--out report.json`).
/// * [`Remote`](Self::Remote) — the payload was pushed to an external
///   backend; `identifier` is backend-specific (LangSmith run id, etc.).
#[derive(Debug, Clone)]
pub enum ReporterOutput {
    /// Plain text suitable for stdout or a log line.
    Stdout(String),
    /// A byte artifact to write at the given path.
    Artifact {
        /// Suggested destination path. The reporter does not write to it;
        /// the caller decides whether to persist.
        path: PathBuf,
        /// Raw bytes of the artifact.
        bytes: Vec<u8>,
    },
    /// A remote push result, identified by backend + opaque id.
    Remote {
        /// Human-readable backend name (e.g. `"langsmith"`).
        backend: String,
        /// Backend-specific identifier (e.g. LangSmith run id).
        identifier: String,
    },
}

/// Error surface for [`Reporter`] implementations.
#[derive(Debug, Error)]
pub enum ReporterError {
    /// Filesystem or stream I/O failed.
    #[error("reporter I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Rendering/serialization failed (e.g. JSON encoding).
    #[error("reporter formatting error: {0}")]
    Format(String),
    /// A remote backend push failed.
    #[error("reporter network error: {0}")]
    Network(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_error_from_io() {
        let io_err = std::io::Error::other("boom");
        let err: ReporterError = io_err.into();
        assert!(err.to_string().contains("boom"));
        assert!(matches!(err, ReporterError::Io(_)));
    }

    #[test]
    fn reporter_output_variants_are_constructible() {
        let _stdout = ReporterOutput::Stdout("hello".into());
        let _artifact = ReporterOutput::Artifact {
            path: PathBuf::from("/tmp/out.json"),
            bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let _remote = ReporterOutput::Remote {
            backend: "langsmith".into(),
            identifier: "run-1234".into(),
        };
    }

    #[test]
    fn schema_path_constant_points_at_repo_contract() {
        // Sanity: the published schema path stays stable; reporters and
        // their regression tests must reference the same string.
        assert!(JSON_SCHEMA_PATH.ends_with("eval-result.schema.json"));
    }
}
