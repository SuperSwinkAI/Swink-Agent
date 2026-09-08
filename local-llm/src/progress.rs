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
#[path = "progress_tests.rs"]
mod tests;
