//! Progress tracking for model download and loading.
//!
//! [`ProgressEvent`] represents lifecycle progress updates during model
//! download and loading, and [`ProgressCallbackFn`] allows callers to
//! observe transitions (e.g. for TUI progress bars).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use hf_hub::progress::{DownloadEvent, ProgressEvent as HfProgressEvent, ProgressHandler};

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

/// `hf-hub` reports `0` for totals the server never provided; map that to
/// `None` so consumers can distinguish an unknown size from an empty file.
fn known_total(total_bytes: u64) -> Option<u64> {
    (total_bytes > 0).then_some(total_bytes)
}

/// Bridges `hf-hub`'s [`ProgressHandler`] events into this crate's
/// [`ProgressEvent::DownloadProgress`] callbacks.
///
/// hf-hub 1.0 shares a single handler across a download (no per-chunk
/// cloning), but [`ProgressHandler::on_progress`] takes `&self`, so byte
/// aggregation lives behind interior mutability. [`DownloadEvent::Progress`]
/// carries per-file *deltas* — only files whose state changed, each with a
/// cumulative `bytes_completed` — so the handler tracks the latest count per
/// filename and sums them. In practice this crate downloads one file at a
/// time, but the accumulation is defensive against multi-file events.
pub(crate) struct HfHubDownloadProgress {
    callback: ProgressCallbackFn,
    total_bytes: AtomicU64,
    per_file_bytes: Mutex<HashMap<String, u64>>,
}

impl HfHubDownloadProgress {
    pub(crate) fn new(callback: ProgressCallbackFn) -> Self {
        Self {
            callback,
            total_bytes: AtomicU64::new(0),
            per_file_bytes: Mutex::new(HashMap::new()),
        }
    }
}

impl ProgressHandler for HfHubDownloadProgress {
    fn on_progress(&self, event: &HfProgressEvent) {
        // This crate never uploads; ignore `Upload(_)` events.
        let HfProgressEvent::Download(event) = event else {
            return;
        };
        match event {
            DownloadEvent::Start { total_bytes, .. } => {
                self.total_bytes.store(*total_bytes, Ordering::SeqCst);
                self.per_file_bytes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clear();
                (self.callback)(ProgressEvent::DownloadProgress {
                    bytes_downloaded: 0,
                    total_bytes: known_total(*total_bytes),
                });
            }
            DownloadEvent::Progress { files } => {
                let bytes_downloaded = {
                    let mut per_file = self
                        .per_file_bytes
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    for file in files {
                        per_file.insert(file.filename.clone(), file.bytes_completed);
                    }
                    per_file.values().sum()
                };
                (self.callback)(ProgressEvent::DownloadProgress {
                    bytes_downloaded,
                    total_bytes: known_total(self.total_bytes.load(Ordering::SeqCst)),
                });
            }
            DownloadEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            } => {
                (self.callback)(ProgressEvent::DownloadProgress {
                    bytes_downloaded: *bytes_completed,
                    total_bytes: known_total(*total_bytes),
                });
            }
            // [`ProgressEvent::DownloadComplete`] is emitted by the loader
            // once `resolve_model_path` returns — don't double-emit it here.
            DownloadEvent::Complete => {}
        }
    }
}

/// Resolves a model file to a local path, downloading it if needed.
///
/// hf-hub 1.0's `download_file` is cache-aware: when online it revalidates
/// the cached copy via `If-None-Match` (a `304 Not Modified` returns the
/// cached path without re-downloading), and when the network is unreachable
/// it falls back to resolving from the local cache alone. Compared to the
/// 0.5-era "cache first, never revalidate" behavior this trades one
/// conditional request per load for staleness detection, while offline use
/// from a warm cache keeps working.
pub(crate) async fn resolve_model_path(
    repo_id: &str,
    filename: &str,
    progress_cb: Option<ProgressCallbackFn>,
) -> Result<PathBuf, hf_hub::HFError> {
    resolve_model_path_with_client(hf_hub::HFClient::new()?, repo_id, filename, progress_cb).await
}

/// Test seam for [`resolve_model_path`]: takes a pre-built [`hf_hub::HFClient`]
/// so tests can point at a hermetic cache directory and an unroutable endpoint.
async fn resolve_model_path_with_client(
    client: hf_hub::HFClient,
    repo_id: &str,
    filename: &str,
    progress_cb: Option<ProgressCallbackFn>,
) -> Result<PathBuf, hf_hub::HFError> {
    // hf-hub 1.0 takes owner and name as separate arguments; model repos are
    // always `owner/name`, so an id without a `/` is a configuration error.
    let Some((owner, name)) = repo_id.split_once('/') else {
        return Err(hf_hub::HFError::InvalidParameter(format!(
            "model repo id {repo_id:?} must be in \"owner/name\" form"
        )));
    };
    let progress =
        progress_cb.map(|cb| hf_hub::progress::Progress::new(HfHubDownloadProgress::new(cb)));
    client
        .model(owner, name)
        .download_file()
        .filename(filename)
        .maybe_progress(progress)
        .send()
        .await
}

#[cfg(test)]
mod tests {
    use hf_hub::progress::{FileProgress, FileStatus, UploadEvent};

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

    /// Collects every emitted [`ProgressEvent`] plus a handler wired to it.
    fn recording_handler() -> (Arc<Mutex<Vec<ProgressEvent>>>, HfHubDownloadProgress) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let callback: ProgressCallbackFn = Arc::new(move |event| {
            events_clone.lock().unwrap().push(event);
        });
        (events, HfHubDownloadProgress::new(callback))
    }

    fn file_progress(filename: &str, bytes_completed: u64, status: FileStatus) -> FileProgress {
        FileProgress {
            filename: filename.to_string(),
            bytes_completed,
            total_bytes: 0,
            status,
        }
    }

    fn download_events(events: &[ProgressEvent]) -> Vec<(u64, Option<u64>)> {
        events
            .iter()
            .filter_map(|event| match event {
                ProgressEvent::DownloadProgress {
                    bytes_downloaded,
                    total_bytes,
                } => Some((*bytes_downloaded, *total_bytes)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hf_hub_download_progress_accumulates_per_file_deltas() {
        let (events, handler) = recording_handler();

        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Start {
            total_files: 1,
            total_bytes: 100,
        }));
        // `Progress` events carry cumulative per-file counts, not chunk sizes.
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("model.gguf", 30, FileStatus::InProgress)],
        }));
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("model.gguf", 80, FileStatus::InProgress)],
        }));
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("model.gguf", 100, FileStatus::Complete)],
        }));
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Complete));

        let events = events.lock().unwrap();
        assert_eq!(
            download_events(&events),
            vec![
                (0, Some(100)),
                (30, Some(100)),
                (80, Some(100)),
                (100, Some(100)),
            ]
        );
        // `DownloadEvent::Complete` must not emit `DownloadComplete` — the
        // loader emits that after `resolve_model_path` returns.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProgressEvent::DownloadComplete)),
            "handler must not double-emit DownloadComplete"
        );
    }

    #[test]
    fn hf_hub_download_progress_sums_across_files() {
        let (events, handler) = recording_handler();

        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Start {
            total_files: 2,
            total_bytes: 300,
        }));
        // Deltas only mention changed files; unchanged files keep their last
        // recorded cumulative count.
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("a.gguf", 100, FileStatus::Complete)],
        }));
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("b.gguf", 50, FileStatus::InProgress)],
        }));
        handler.on_progress(&HfProgressEvent::Download(DownloadEvent::Progress {
            files: vec![file_progress("b.gguf", 200, FileStatus::Complete)],
        }));

        let events = events.lock().unwrap();
        assert_eq!(
            download_events(&events),
            vec![
                (0, Some(300)),
                (100, Some(300)),
                (150, Some(300)),
                (300, Some(300)),
            ]
        );
    }

    #[test]
    fn hf_hub_download_progress_passes_aggregate_through() {
        let (events, handler) = recording_handler();

        handler.on_progress(&HfProgressEvent::Download(
            DownloadEvent::AggregateProgress {
                bytes_completed: 512,
                total_bytes: 1024,
                bytes_per_sec: Some(100.0),
            },
        ));
        // A zero total means the size is unknown → `None`.
        handler.on_progress(&HfProgressEvent::Download(
            DownloadEvent::AggregateProgress {
                bytes_completed: 640,
                total_bytes: 0,
                bytes_per_sec: None,
            },
        ));

        let events = events.lock().unwrap();
        assert_eq!(
            download_events(&events),
            vec![(512, Some(1024)), (640, None)]
        );
    }

    #[test]
    fn hf_hub_download_progress_ignores_upload_events() {
        let (events, handler) = recording_handler();

        handler.on_progress(&HfProgressEvent::Upload(UploadEvent::Start {
            total_files: 1,
            total_bytes: 100,
        }));
        handler.on_progress(&HfProgressEvent::Upload(UploadEvent::Committing));
        handler.on_progress(&HfProgressEvent::Upload(UploadEvent::Complete));

        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resolve_model_path_rejects_repo_id_without_owner() {
        let temp = tempfile::tempdir().unwrap();
        let client = hf_hub::HFClient::builder()
            .endpoint("http://127.0.0.1:1")
            .cache_dir(temp.path().join("hub"))
            .retry_max_attempts(0)
            .build()
            .unwrap();

        let err = resolve_model_path_with_client(client, "no-owner", "model.gguf", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, hf_hub::HFError::InvalidParameter(_)),
            "expected InvalidParameter, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_model_path_falls_back_to_cache_when_offline() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("hub");
        let filename = "model.gguf";

        // hf-hub 1.0 cache layout: `models--{owner}--{name}/refs/{revision}`
        // holds the commit hash and `snapshots/{commit}/{filename}` holds the
        // file. Build it by hand so the test is hermetic.
        let repo_dir = cache_dir.join("models--unsloth--synthetic-model");
        let commit = "0123456789abcdef0123456789abcdef01234567";
        std::fs::create_dir_all(repo_dir.join("refs")).unwrap();
        std::fs::write(repo_dir.join("refs").join("main"), commit).unwrap();
        let snapshot_dir = repo_dir.join("snapshots").join(commit);
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        let cached_path = snapshot_dir.join(filename);
        std::fs::write(&cached_path, b"cached").unwrap();

        // Unroutable endpoint: the connection error counts as transient, which
        // triggers hf-hub's offline fallback to the local cache.
        let client = hf_hub::HFClient::builder()
            .endpoint("http://127.0.0.1:1")
            .cache_dir(&cache_dir)
            .retry_max_attempts(0)
            .build()
            .unwrap();

        let resolved =
            resolve_model_path_with_client(client, "unsloth/synthetic-model", filename, None)
                .await
                .unwrap();
        assert_eq!(resolved, cached_path);
    }
}
