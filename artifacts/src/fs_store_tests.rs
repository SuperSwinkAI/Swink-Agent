//! Tests for `fs_store`.
#![cfg(test)]

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::oneshot;
use tokio::task::yield_now;
use tokio::time::timeout;

use super::{FileArtifactStore, VersionRecord};
use swink_agent::{ArtifactData, ArtifactError, ArtifactStore};

fn text_data(content: &str) -> ArtifactData {
    ArtifactData::new(content.as_bytes().to_vec(), "text/plain".to_string())
}

fn assert_invalid_data_storage_error(err: ArtifactError, expected_snippet: &str) {
    let ArtifactError::Storage(source) = err else {
        panic!("expected storage error, got {err:?}");
    };
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("storage error should wrap std::io::Error");
    assert_eq!(io.kind(), ErrorKind::InvalidData);
    assert!(
        io.to_string().contains(expected_snippet),
        "expected error message to contain '{expected_snippet}', got '{io}'"
    );
}

async fn assert_delete_waits_for_lock<T>(delete_task: &tokio::task::JoinHandle<T>, reason: &str) {
    yield_now().await;
    assert!(!delete_task.is_finished(), "{reason}");
}

fn hold_process_lock(path: &Path) -> File {
    std::fs::create_dir_all(path.parent().expect("lock path should have a parent"))
        .expect("process lock parent should be creatable");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("process lock file should be openable");
    file.lock().expect("process lock should be acquirable");
    file
}

#[tokio::test]
async fn save_waits_for_interprocess_artifact_lock() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    let lock_path = store
        .process_lock_path("s1", "report.md")
        .expect("lock path should be valid");
    let process_lock = hold_process_lock(&lock_path);

    let mut save_task = tokio::spawn({
        let store = Arc::clone(&store);
        async move { store.save("s1", "report.md", text_data("v1")).await }
    });

    assert!(
        timeout(Duration::from_millis(100), &mut save_task)
            .await
            .is_err(),
        "save should wait while another process holds the artifact lock"
    );

    process_lock
        .unlock()
        .expect("process lock should be releasable");
    drop(process_lock);

    let version = save_task
        .await
        .expect("save task should join")
        .expect("save should succeed after process lock release");
    assert_eq!(version.version, 1);
}

#[tokio::test]
async fn delete_waits_for_in_flight_artifact_lock() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("initial save");

    let lock = store.artifact_lock("s1", "report.md").await;
    let guard = lock.lock().await;

    let (started_tx, started_rx) = oneshot::channel();
    let delete_store = Arc::clone(&store);
    let delete_task = tokio::spawn(async move {
        started_tx.send(()).expect("notify delete start");
        delete_store.delete("s1", "report.md").await
    });

    started_rx.await.expect("delete task started");
    assert_delete_waits_for_lock(
        &delete_task,
        "delete should wait for the per-artifact lock before removing files",
    )
    .await;

    drop(guard);

    delete_task
        .await
        .expect("delete task join")
        .expect("delete should succeed after lock release");

    assert!(
        store
            .load("s1", "report.md")
            .await
            .expect("load after delete")
            .is_none(),
        "artifact should be deleted once the lock is released"
    );
}

#[tokio::test]
async fn artifact_locks_are_shared_across_store_instances_for_same_root() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let first_store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    let second_store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    first_store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("initial save");

    let lock = first_store.artifact_lock("s1", "report.md").await;
    let guard = lock.lock().await;

    let (started_tx, started_rx) = oneshot::channel();
    let delete_task = tokio::spawn({
        let second_store = Arc::clone(&second_store);
        async move {
            started_tx.send(()).expect("notify delete start");
            second_store.delete("s1", "report.md").await
        }
    });

    started_rx.await.expect("delete task started");
    assert_delete_waits_for_lock(
        &delete_task,
        "a second store instance should wait on the root-wide artifact lock",
    )
    .await;

    drop(guard);

    delete_task
        .await
        .expect("delete task join")
        .expect("delete should succeed after lock release");

    assert!(
        first_store
            .load("s1", "report.md")
            .await
            .expect("load after delete")
            .is_none(),
        "artifact should be deleted once the cross-instance lock is released"
    );
}

#[tokio::test]
async fn load_returns_invalid_data_when_latest_content_file_is_missing() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = FileArtifactStore::new(tmpdir.path());
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("save should succeed");

    let content_path = store.version_path("s1", "report.md", 1);
    tokio::fs::remove_file(&content_path)
        .await
        .expect("content file should be removable");

    let err = store
        .load("s1", "report.md")
        .await
        .expect_err("missing content should be surfaced as corruption");
    assert_invalid_data_storage_error(err, "metadata references missing content");
}

#[tokio::test]
async fn load_version_returns_invalid_data_when_content_file_is_missing() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = FileArtifactStore::new(tmpdir.path());
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("save should succeed");

    let content_path = store.version_path("s1", "report.md", 1);
    tokio::fs::remove_file(&content_path)
        .await
        .expect("content file should be removable");

    let err = store
        .load_version("s1", "report.md", 1)
        .await
        .expect_err("missing content should be surfaced as corruption");
    assert_invalid_data_storage_error(err, "metadata references missing content");
}

#[tokio::test]
async fn load_version_waits_for_in_flight_metadata_commit() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("initial save");

    let lock = store.artifact_lock("s1", "report.md").await;
    let guard = lock.lock().await;

    let content_path = store.version_path("s1", "report.md", 2);
    tokio::fs::write(&content_path, b"v2")
        .await
        .expect("stage new content before metadata commit");

    let mut meta = store
        .read_meta("s1", "report.md")
        .await
        .expect("read current metadata");
    let now = Utc::now();
    meta.versions.push(VersionRecord {
        name: "report.md".to_string(),
        version: 2,
        created_at: now,
        size: 2,
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    });

    let (started_tx, started_rx) = oneshot::channel();
    let load_store = Arc::clone(&store);
    let load_task = tokio::spawn(async move {
        started_tx.send(()).expect("notify load start");
        load_store.load_version("s1", "report.md", 2).await
    });

    started_rx.await.expect("load task started");
    yield_now().await;
    assert!(
        !load_task.is_finished(),
        "load_version should wait for the artifact lock while metadata is being committed"
    );

    store
        .write_meta("s1", "report.md", &meta)
        .await
        .expect("commit metadata");
    drop(guard);

    let (data, version) = load_task
        .await
        .expect("load task join")
        .expect("load should succeed after metadata commit")
        .expect("version should exist after metadata commit");
    assert_eq!(data.content, b"v2");
    assert_eq!(version.version, 2);
}

#[tokio::test]
async fn list_waits_for_in_flight_metadata_commit() {
    let tmpdir = tempfile::TempDir::new().expect("tempdir");
    let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("initial save");

    let lock = store.artifact_lock("s1", "report.md").await;
    let guard = lock.lock().await;

    let content_path = store.version_path("s1", "report.md", 2);
    tokio::fs::write(&content_path, b"v2")
        .await
        .expect("stage new content before metadata commit");

    let mut meta = store
        .read_meta("s1", "report.md")
        .await
        .expect("read current metadata");
    let now = Utc::now();
    meta.versions.push(VersionRecord {
        name: "report.md".to_string(),
        version: 2,
        created_at: now,
        size: 2,
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    });

    let (started_tx, started_rx) = oneshot::channel();
    let list_store = Arc::clone(&store);
    let list_task = tokio::spawn(async move {
        started_tx.send(()).expect("notify list start");
        list_store.list("s1").await
    });

    started_rx.await.expect("list task started");
    yield_now().await;
    assert!(
        !list_task.is_finished(),
        "list should wait for the artifact lock while metadata is being committed"
    );

    store
        .write_meta("s1", "report.md", &meta)
        .await
        .expect("commit metadata");
    drop(guard);

    let artifacts = list_task
        .await
        .expect("list task join")
        .expect("list should succeed after metadata commit");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name, "report.md");
    assert_eq!(artifacts[0].latest_version, 2);
}
