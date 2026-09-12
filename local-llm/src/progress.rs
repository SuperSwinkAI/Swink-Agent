//! Progress tracking for model download and loading.
//!
//! [`ProgressEvent`] represents lifecycle progress updates during model
//! download and loading, and [`ProgressCallbackFn`] allows callers to
//! observe transitions (e.g. for TUI progress bars).

use std::sync::Arc;

/// Progress event emitted during model download and loading.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Model weights are being downloaded.
    DownloadProgress {
        /// Bytes downloaded so far.
        bytes_downloaded: u64,
        /// Total bytes (`None` if server doesn't report Content-Length).
        total_bytes: Option<u64>,
    },

    /// Download finished successfully.
    DownloadComplete,

    /// Model is being loaded into memory.
    LoadingProgress {
        /// Status message from the loading pipeline.
        message: String,
    },

    /// Model loaded and ready for inference.
    LoadingComplete,
}

/// Callback invoked when model progress changes.
///
/// Stored behind `Arc` for cheap cloning and shared ownership.
pub type ProgressCallbackFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
