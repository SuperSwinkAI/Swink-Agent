use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;

use swink_agent::{ArtifactData, ArtifactStore};
use swink_agent_artifacts::FileArtifactStore;

fn text_data(content: &str) -> ArtifactData {
    ArtifactData {
        content: content.as_bytes().to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    }
}

fn assert_storage_error_kind(err: swink_agent::ArtifactError, expected_kind: ErrorKind) {
    let swink_agent::ArtifactError::Storage(source) = err else {
        panic!("expected storage error, got {err:?}");
    };
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("storage error should wrap std::io::Error");
    assert_eq!(io.kind(), expected_kind);
}

// T035: fs_save_and_load_round_trip
#[tokio::test]
async fn fs_save_and_load_round_trip() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let original = ArtifactData {
        content: b"hello world".to_vec(),
        content_type: "text/plain".to_string(),
        metadata: {
            let mut m = HashMap::new();
            m.insert("key".to_string(), "value".to_string());
            m
        },
    };

    let version = store
        .save("s1", "report.md", original.clone())
        .await
        .unwrap();
    assert_eq!(version.version, 1);
    assert_eq!(version.name, "report.md");
    assert_eq!(version.size, 11);
    assert_eq!(version.content_type, "text/plain");

    let (loaded, loaded_ver) = store.load("s1", "report.md").await.unwrap().unwrap();
    assert_eq!(loaded.content, b"hello world");
    assert_eq!(loaded.content_type, "text/plain");
    assert_eq!(loaded.metadata.get("key").unwrap(), "value");
    assert_eq!(loaded_ver.version, 1);
}

// T036: fs_persistence_across_instances
#[tokio::test]
async fn fs_persistence_across_instances() {
    let tmpdir = tempfile::TempDir::new().unwrap();

    {
        let store_a = FileArtifactStore::new(tmpdir.path());
        store_a
            .save("s1", "doc.txt", text_data("persisted"))
            .await
            .unwrap();
    }
    // store_a is dropped

    let store_b = FileArtifactStore::new(tmpdir.path());
    let (data, version) = store_b.load("s1", "doc.txt").await.unwrap().unwrap();
    assert_eq!(data.content, b"persisted");
    assert_eq!(version.version, 1);
}

// T037: fs_versioning_persists
#[tokio::test]
async fn fs_versioning_persists() {
    let tmpdir = tempfile::TempDir::new().unwrap();

    {
        let store = FileArtifactStore::new(tmpdir.path());
        store
            .save("s1", "notes.md", text_data("v1 content"))
            .await
            .unwrap();
        store
            .save("s1", "notes.md", text_data("v2 content"))
            .await
            .unwrap();
        store
            .save("s1", "notes.md", text_data("v3 content"))
            .await
            .unwrap();
    }

    let store = FileArtifactStore::new(tmpdir.path());

    // All 3 versions accessible
    for (ver_num, expected) in [(1, "v1 content"), (2, "v2 content"), (3, "v3 content")] {
        let (data, version) = store
            .load_version("s1", "notes.md", ver_num)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(data.content, expected.as_bytes());
        assert_eq!(version.version, ver_num);
    }

    // Latest is v3
    let (data, version) = store.load("s1", "notes.md").await.unwrap().unwrap();
    assert_eq!(data.content, b"v3 content");
    assert_eq!(version.version, 3);
}

// T038: fs_large_artifact_integrity
#[tokio::test]
async fn fs_large_artifact_integrity() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    // 1 MB deterministic pattern
    let large_content: Vec<u8> = (0..1_048_576u32).map(|i| (i % 256) as u8).collect();
    let data = ArtifactData {
        content: large_content.clone(),
        content_type: "application/octet-stream".to_string(),
        metadata: HashMap::new(),
    };

    let version = store.save("s1", "big.bin", data).await.unwrap();
    assert_eq!(version.size, 1_048_576);

    let (loaded, _) = store.load("s1", "big.bin").await.unwrap().unwrap();
    assert_eq!(loaded.content.len(), 1_048_576);
    assert_eq!(loaded.content, large_content);
}

// T039: fs_concurrent_saves_no_corruption
#[tokio::test]
async fn fs_concurrent_saves_no_corruption() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(FileArtifactStore::new(tmpdir.path()));

    let mut handles = Vec::new();
    for i in 0..10u32 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let content = format!("concurrent-{i}");
            let data = ArtifactData {
                content: content.into_bytes(),
                content_type: "text/plain".to_string(),
                metadata: HashMap::new(),
            };
            store.save("s1", "shared.txt", data).await.unwrap()
        }));
    }

    let mut versions = Vec::new();
    for handle in handles {
        versions.push(handle.await.unwrap());
    }

    // All 10 versions should exist with distinct version numbers
    let mut version_nums: Vec<u32> = versions.iter().map(|v| v.version).collect();
    version_nums.sort_unstable();
    assert_eq!(version_nums, (1..=10).collect::<Vec<u32>>());

    // All 10 versions should be loadable
    for ver_num in 1..=10u32 {
        let result = store
            .load_version("s1", "shared.txt", ver_num)
            .await
            .unwrap();
        assert!(result.is_some(), "version {ver_num} missing");
    }

    // Latest should be version 10
    let (_, latest) = store.load("s1", "shared.txt").await.unwrap().unwrap();
    assert_eq!(latest.version, 10);
}

// T040: fs_empty_session_returns_empty
#[tokio::test]
async fn fs_empty_session_returns_empty() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    // List on fresh store returns empty
    let metas = store.list("nonexistent-session").await.unwrap();
    assert!(metas.is_empty());

    // Load on fresh store returns None
    let result = store.load("s1", "missing.txt").await.unwrap();
    assert!(result.is_none());

    // Load version on fresh store returns None
    let result = store.load_version("s1", "missing.txt", 1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn fs_save_rolls_back_new_content_when_metadata_write_fails() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .expect("initial save should succeed");

    let artifact_dir = tmpdir.path().join("s1").join("report.md");
    let meta_path = artifact_dir.join("meta.json");
    tokio::fs::remove_file(&meta_path)
        .await
        .expect("meta.json should be removable");
    tokio::fs::create_dir(&meta_path)
        .await
        .expect("directory replacement should succeed");

    let err = store
        .save("s1", "report.md", text_data("v2"))
        .await
        .expect_err("save should fail when meta.json cannot be replaced");
    assert_storage_error_kind(err, ErrorKind::PermissionDenied);

    assert!(
        !artifact_dir.join("v2.bin").exists(),
        "new content file should be rolled back on metadata write failure"
    );
    assert!(
        artifact_dir.join("v1.bin").exists(),
        "previous committed content must remain intact"
    );
}
