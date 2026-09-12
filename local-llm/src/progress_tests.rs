//! Tests for `progress`.
#![cfg(test)]

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
