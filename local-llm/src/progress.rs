//! Progress tracking for model download and loading.
//!
//! [`ProgressEvent`] represents lifecycle progress updates during model
//! download and loading, and [`ProgressCallbackFn`] allows callers to
//! observe transitions (e.g. for TUI progress bars).

use std::sync::Arc;

/// Progress event emitted during model download and loading.
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
mod tests {
    use super::*;

    #[test]
    fn progress_variants_are_debug() {
        let variants = [
            ProgressEvent::DownloadProgress {
                bytes_downloaded: 100,
                total_bytes: Some(1000),
            },
            ProgressEvent::DownloadProgress {
                bytes_downloaded: 50,
                total_bytes: None,
            },
            ProgressEvent::DownloadComplete,
            ProgressEvent::LoadingProgress {
                message: "loading layers".into(),
            },
            ProgressEvent::LoadingComplete,
        ];
        for v in &variants {
            let debug = format!("{v:?}");
            assert!(!debug.is_empty());
        }
    }

    #[test]
    fn progress_callback_is_callable() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = Arc::clone(&called);
        let cb: ProgressCallbackFn = Arc::new(move |_progress| {
            called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        cb(ProgressEvent::DownloadComplete);
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn download_progress_with_none_total() {
        let event = ProgressEvent::DownloadProgress {
            bytes_downloaded: 42,
            total_bytes: None,
        };
        let debug = format!("{event:?}");
        assert!(debug.contains("42"));
    }

    #[test]
    fn loading_progress_carries_message() {
        let event = ProgressEvent::LoadingProgress {
            message: "initializing layers".into(),
        };
        let debug = format!("{event:?}");
        assert!(debug.contains("initializing layers"));
    }
}
