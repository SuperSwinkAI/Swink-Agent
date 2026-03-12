//! Error types for the TUI crate.

use std::io;

use swink_agent::AgentError;

/// Top-level error type for the TUI binary.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Terminal I/O failure.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// Agent-level error.
    #[error(transparent)]
    Agent(#[from] AgentError),

    /// Catch-all for other errors (e.g. from third-party crates).
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}
