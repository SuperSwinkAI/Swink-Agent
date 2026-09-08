//! Tests for `progress`.
#![cfg(test)]

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
