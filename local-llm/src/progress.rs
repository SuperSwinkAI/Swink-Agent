//! Progress tracking for model download and loading.
//!
//! [`ProgressEvent`] represents lifecycle progress updates during model
//! download and loading, and [`ProgressCallbackFn`] allows callers to
//! observe transitions (e.g. for TUI progress bars).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// `hf-hub` clones progress handlers per download chunk, so byte aggregation
/// must live in shared state rather than per-clone fields.
#[derive(Clone)]
pub(crate) struct HfHubDownloadProgress {
    callback: ProgressCallbackFn,
    total_bytes: Arc<AtomicU64>,
    downloaded_bytes: Arc<AtomicU64>,
}

impl HfHubDownloadProgress {
    pub(crate) fn new(callback: ProgressCallbackFn) -> Self {
        Self {
            callback,
            total_bytes: Arc::new(AtomicU64::new(0)),
            downloaded_bytes: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl hf_hub::api::tokio::Progress for HfHubDownloadProgress {
    async fn init(&mut self, size: usize, _filename: &str) {
        let size = size as u64;
        self.total_bytes.store(size, Ordering::SeqCst);
        self.downloaded_bytes.store(0, Ordering::SeqCst);
        (self.callback)(ProgressEvent::DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: Some(size),
        });
    }

    async fn update(&mut self, size: usize) {
        let downloaded = self
            .downloaded_bytes
            .fetch_add(size as u64, Ordering::SeqCst)
            + size as u64;
        let total_bytes = self.total_bytes.load(Ordering::SeqCst);
        (self.callback)(ProgressEvent::DownloadProgress {
            bytes_downloaded: downloaded,
            total_bytes: Some(total_bytes),
        });
    }

    async fn finish(&mut self) {}
}

pub(crate) async fn resolve_model_path(
    repo_id: &str,
    filename: &str,
    progress_cb: Option<ProgressCallbackFn>,
) -> Result<PathBuf, hf_hub::api::tokio::ApiError> {
    resolve_model_path_from_cache(hf_hub::Cache::from_env(), repo_id, filename, progress_cb).await
}

async fn resolve_model_path_from_cache(
    cache: hf_hub::Cache,
    repo_id: &str,
    filename: &str,
    progress_cb: Option<ProgressCallbackFn>,
) -> Result<PathBuf, hf_hub::api::tokio::ApiError> {
    if let Some(path) = cache.model(repo_id.to_string()).get(filename) {
        return Ok(path);
    }

    let api = hf_hub::api::tokio::Api::new()?;
    let repo = api.model(repo_id.to_string());
    match progress_cb {
        Some(cb) => {
            repo.download_with_progress(filename, HfHubDownloadProgress::new(cb))
                .await
        }
        None => repo.download(filename).await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

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

    #[tokio::test]
    async fn hf_hub_download_progress_accumulates_across_clones() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let callback: ProgressCallbackFn = Arc::new(move |event| {
            events_clone.lock().unwrap().push(event);
        });

        let mut progress = HfHubDownloadProgress::new(callback);
        let mut clone = progress.clone();

        hf_hub::api::tokio::Progress::init(&mut progress, 100, "model.gguf").await;
        hf_hub::api::tokio::Progress::update(&mut progress, 30).await;
        hf_hub::api::tokio::Progress::update(&mut clone, 20).await;
        hf_hub::api::tokio::Progress::update(&mut progress, 50).await;

        let events = events.lock().unwrap();
        let download_events: Vec<(u64, Option<u64>)> = events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::DownloadProgress {
                    bytes_downloaded,
                    total_bytes,
                } => Some((*bytes_downloaded, *total_bytes)),
                _ => None,
            })
            .collect();
        assert_eq!(
            download_events,
            vec![
                (0, Some(100)),
                (30, Some(100)),
                (50, Some(100)),
                (100, Some(100))
            ]
        );
    }

    #[tokio::test]
    async fn resolve_model_path_uses_cached_file_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let repo_id = "unsloth/synthetic-model".to_string();
        let filename = "model.gguf";
        let cache = hf_hub::Cache::new(temp.path().join("hub"));
        let repo = cache.model(repo_id.clone());
        repo.create_ref("commit-123").unwrap();

        let mut pointer_path = repo.pointer_path("commit-123");
        std::fs::create_dir_all(&pointer_path).unwrap();
        pointer_path.push(filename);
        std::fs::write(&pointer_path, b"cached").unwrap();

        let resolved = resolve_model_path_from_cache(cache, &repo_id, filename, None)
            .await
            .unwrap();
        assert_eq!(resolved, pointer_path);
    }
}
