//! Progress tracking for model download and loading.
//!
//! [`ModelProgress`] represents the lifecycle stages of a local model, and
//! [`ProgressCallbackFn`] allows callers to observe transitions (e.g. for
//! TUI progress bars).

use std::sync::Arc;

/// Lifecycle stage of a local model.
#[derive(Debug, Clone)]
pub enum ModelProgress {
    /// Model weights are being downloaded.
    Downloading {
        /// Bytes downloaded so far.
        downloaded: u64,
        /// Total bytes (0 if unknown).
        total: u64,
    },

    /// Model is being loaded into memory.
    Loading,

    /// Model is ready for inference.
    Ready,

    /// Model failed to load or download.
    Failed { message: String },
}

/// Callback invoked when model progress changes.
///
/// Stored behind `Arc` for cheap cloning and shared ownership.
pub type ProgressCallbackFn = Arc<dyn Fn(ModelProgress) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_variants_are_debug() {
        let variants = [
            ModelProgress::Downloading {
                downloaded: 100,
                total: 1000,
            },
            ModelProgress::Loading,
            ModelProgress::Ready,
            ModelProgress::Failed {
                message: "oops".into(),
            },
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
        cb(ModelProgress::Ready);
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
