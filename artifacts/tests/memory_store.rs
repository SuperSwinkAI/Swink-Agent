use std::collections::HashMap;

use swink_agent::{ArtifactData, ArtifactError, ArtifactStore};
use swink_agent_artifacts::InMemoryArtifactStore;

fn text_data(content: &str) -> ArtifactData {
    ArtifactData {
        content: content.as_bytes().to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    }
}

// T016: save_creates_version_one
#[tokio::test]
async fn save_creates_version_one() {
    let store = InMemoryArtifactStore::new();
    let version = store
        .save("s1", "report.md", text_data("hello"))
        .await
        .unwrap();

    assert_eq!(version.version, 1);
    assert_eq!(version.name, "report.md");
    assert_eq!(version.content_type, "text/plain");
    assert_eq!(version.size, 5);
}

// T017: save_same_name_increments_version
#[tokio::test]
async fn save_same_name_increments_version() {
    let store = InMemoryArtifactStore::new();
    let v1 = store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();
    let v2 = store
        .save("s1", "report.md", text_data("v2"))
        .await
        .unwrap();

    assert_eq!(v1.version, 1);
    assert_eq!(v2.version, 2);
}

// T018: load_returns_latest_version
#[tokio::test]
async fn load_returns_latest_version() {
    let store = InMemoryArtifactStore::new();
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v2"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v3"))
        .await
        .unwrap();

    let (data, version) = store.load("s1", "report.md").await.unwrap().unwrap();
    assert_eq!(version.version, 3);
    assert_eq!(data.content, b"v3");
}

// T019: load_version_returns_specific
#[tokio::test]
async fn load_version_returns_specific() {
    let store = InMemoryArtifactStore::new();
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v2"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v3"))
        .await
        .unwrap();

    let (data, version) = store
        .load_version("s1", "report.md", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version.version, 1);
    assert_eq!(data.content, b"v1");
}

// T020: load_nonexistent_returns_none
#[tokio::test]
async fn load_nonexistent_returns_none() {
    let store = InMemoryArtifactStore::new();
    let result = store.load("s1", "missing.txt").await.unwrap();
    assert!(result.is_none());
}

// T021: load_version_nonexistent_returns_none
#[tokio::test]
async fn load_version_nonexistent_returns_none() {
    let store = InMemoryArtifactStore::new();
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();

    let result = store.load_version("s1", "report.md", 99).await.unwrap();
    assert!(result.is_none());
}

// T022: save_validates_name
#[tokio::test]
async fn save_validates_name() {
    let store = InMemoryArtifactStore::new();

    let result = store.save("s1", "", text_data("x")).await;
    assert!(matches!(result, Err(ArtifactError::InvalidName { .. })));

    let result = store.save("s1", "../etc/passwd", text_data("x")).await;
    assert!(matches!(result, Err(ArtifactError::InvalidName { .. })));

    let result = store.save("s1", "/leading", text_data("x")).await;
    assert!(matches!(result, Err(ArtifactError::InvalidName { .. })));
}

// T023: save_empty_content_succeeds
#[tokio::test]
async fn save_empty_content_succeeds() {
    let store = InMemoryArtifactStore::new();
    let data = ArtifactData {
        content: vec![],
        content_type: "application/octet-stream".to_string(),
        metadata: HashMap::new(),
    };
    let version = store.save("s1", "empty.bin", data).await.unwrap();

    assert_eq!(version.version, 1);
    assert_eq!(version.size, 0);

    let (loaded, _) = store.load("s1", "empty.bin").await.unwrap().unwrap();
    assert!(loaded.content.is_empty());
}
