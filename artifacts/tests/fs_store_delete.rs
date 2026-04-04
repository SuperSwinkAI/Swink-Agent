use std::collections::HashMap;

use swink_agent::{ArtifactData, ArtifactStore};
use swink_agent_artifacts::FileArtifactStore;

fn text_data(content: &str) -> ArtifactData {
    ArtifactData {
        content: content.as_bytes().to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    }
}

// T071: fs_delete_removes_files
#[tokio::test]
async fn fs_delete_removes_files() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    // Save an artifact with multiple versions
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v2"))
        .await
        .unwrap();

    // Verify artifact directory exists on disk
    let artifact_dir = tmpdir.path().join("s1").join("report.md");
    assert!(artifact_dir.exists(), "artifact dir should exist before delete");

    // Delete
    store.delete("s1", "report.md").await.unwrap();

    // load returns None
    let result = store.load("s1", "report.md").await.unwrap();
    assert!(result.is_none());

    // list returns empty
    let metas = store.list("s1").await.unwrap();
    assert!(metas.is_empty());

    // Artifact directory removed from disk
    assert!(
        !artifact_dir.exists(),
        "artifact dir should be removed after delete"
    );
}

// T072: fs_delete_nonexistent_succeeds
#[tokio::test]
async fn fs_delete_nonexistent_succeeds() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    // Delete on empty/fresh store succeeds silently
    let result = store.delete("no-session", "missing.txt").await;
    assert!(result.is_ok());

    // Delete nonexistent name within a session that has other artifacts
    store
        .save("s1", "keep.txt", text_data("hello"))
        .await
        .unwrap();
    let result = store.delete("s1", "nonexistent.txt").await;
    assert!(result.is_ok());

    // Existing artifact is unaffected
    let (data, _) = store.load("s1", "keep.txt").await.unwrap().unwrap();
    assert_eq!(data.content, b"hello");
}
