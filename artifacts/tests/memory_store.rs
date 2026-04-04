use std::collections::HashMap;

use swink_agent::{ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore};
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

// T029: list_returns_all_artifacts
#[tokio::test]
async fn list_returns_all_artifacts() {
    let store = InMemoryArtifactStore::new();

    let csv_data = ArtifactData {
        content: b"a,b,c".to_vec(),
        content_type: "text/csv".to_string(),
        metadata: HashMap::new(),
    };
    let json_data = ArtifactData {
        content: b"{}".to_vec(),
        content_type: "application/json".to_string(),
        metadata: HashMap::new(),
    };

    store
        .save("s1", "report.md", text_data("hello"))
        .await
        .unwrap();
    store.save("s1", "data.csv", csv_data).await.unwrap();
    store.save("s1", "config.json", json_data).await.unwrap();

    let mut metas: Vec<ArtifactMeta> = store.list("s1").await.unwrap();
    metas.sort_by(|a, b| a.name.cmp(&b.name));

    assert_eq!(metas.len(), 3);
    assert_eq!(metas[0].name, "config.json");
    assert_eq!(metas[0].content_type, "application/json");
    assert_eq!(metas[0].latest_version, 1);
    assert_eq!(metas[1].name, "data.csv");
    assert_eq!(metas[1].content_type, "text/csv");
    assert_eq!(metas[1].latest_version, 1);
    assert_eq!(metas[2].name, "report.md");
    assert_eq!(metas[2].content_type, "text/plain");
    assert_eq!(metas[2].latest_version, 1);
}

// T030: list_empty_session_returns_empty
#[tokio::test]
async fn list_empty_session_returns_empty() {
    let store = InMemoryArtifactStore::new();
    let metas = store.list("nonexistent-session").await.unwrap();
    assert!(metas.is_empty());
}

// T031: list_reflects_latest_version
#[tokio::test]
async fn list_reflects_latest_version() {
    let store = InMemoryArtifactStore::new();
    store
        .save("s1", "report.md", text_data("v1"))
        .await
        .unwrap();
    store
        .save("s1", "report.md", text_data("v2"))
        .await
        .unwrap();

    let metas = store.list("s1").await.unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].name, "report.md");
    assert_eq!(metas[0].latest_version, 2);
    assert!(metas[0].updated_at >= metas[0].created_at);
}

// T032: load_includes_custom_metadata
#[tokio::test]
async fn load_includes_custom_metadata() {
    let store = InMemoryArtifactStore::new();
    let mut metadata = HashMap::new();
    metadata.insert("author".to_string(), "agent-1".to_string());
    metadata.insert("source".to_string(), "web-scrape".to_string());

    let data = ArtifactData {
        content: b"some content".to_vec(),
        content_type: "text/plain".to_string(),
        metadata,
    };

    store.save("s1", "result.txt", data).await.unwrap();

    let (loaded, _) = store.load("s1", "result.txt").await.unwrap().unwrap();
    assert_eq!(loaded.metadata.get("author").unwrap(), "agent-1");
    assert_eq!(loaded.metadata.get("source").unwrap(), "web-scrape");
    assert_eq!(loaded.metadata.len(), 2);
}
