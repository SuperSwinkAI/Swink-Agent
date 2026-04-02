# Quickstart: Artifact Service

**Feature**: 036-artifact-service | **Date**: 2026-04-02

## Enable the Feature

```toml
# In your Cargo.toml
[dependencies]
swink-agent = { version = "0.1", features = ["artifact-store"] }
swink-agent-artifacts = "0.1"

# If you also want built-in LLM tools:
swink-agent = { version = "0.1", features = ["artifact-store", "artifact-tools"] }
```

## Basic Usage: Programmatic Artifact Storage

```rust
use swink_agent::{ArtifactStore, ArtifactData};
use swink_agent_artifacts::FileArtifactStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a filesystem-backed store
    let store = FileArtifactStore::new("./artifacts");

    let session_id = "session-abc";

    // Save an artifact
    let version = store.save(session_id, "report.md", ArtifactData {
        content: b"# My Report\nHello world".to_vec(),
        content_type: "text/markdown".into(),
        metadata: Default::default(),
    }).await?;

    println!("Saved version {}", version.version); // "Saved version 1"

    // Save a second version
    let v2 = store.save(session_id, "report.md", ArtifactData {
        content: b"# My Report\nUpdated content".to_vec(),
        content_type: "text/markdown".into(),
        metadata: Default::default(),
    }).await?;

    println!("Saved version {}", v2.version); // "Saved version 2"

    // Load latest version
    if let Some((data, ver)) = store.load(session_id, "report.md").await? {
        println!("Latest: v{}, {} bytes", ver.version, data.content.len());
    }

    // Load specific version
    if let Some((data, _)) = store.load_version(session_id, "report.md", 1).await? {
        println!("v1: {}", String::from_utf8_lossy(&data.content));
    }

    // List all artifacts
    let artifacts = store.list(session_id).await?;
    for meta in &artifacts {
        println!("{}: v{} ({})", meta.name, meta.latest_version, meta.content_type);
    }

    // Delete an artifact
    store.delete(session_id, "report.md").await?;

    Ok(())
}
```

## With an Agent: Built-in Tools

```rust
use std::sync::Arc;
use swink_agent::{Agent, artifact_tools};
use swink_agent_artifacts::InMemoryArtifactStore;

let store = Arc::new(InMemoryArtifactStore::new());

// Create artifact tools that the LLM can call
let tools = artifact_tools(store.clone());

let agent = Agent::new("You are a helpful assistant that can save and load files.")
    .tools(tools)
    // ... configure stream function, etc.
    .build();

// The LLM can now call:
//   save_artifact(name: "notes.md", content: "...", content_type: "text/markdown")
//   load_artifact(name: "notes.md")
//   list_artifacts()
```

## Testing with InMemoryArtifactStore

```rust
use swink_agent_artifacts::InMemoryArtifactStore;
use swink_agent::{ArtifactStore, ArtifactData};

#[tokio::test]
async fn artifact_round_trip() {
    let store = InMemoryArtifactStore::new();

    let v = store.save("test-session", "data.csv", ArtifactData {
        content: b"col1,col2\na,b".to_vec(),
        content_type: "text/csv".into(),
        metadata: [("source".into(), "test".into())].into(),
    }).await.unwrap();

    assert_eq!(v.version, 1);
    assert_eq!(v.size, 14);

    let (loaded, _) = store.load("test-session", "data.csv").await.unwrap().unwrap();
    assert_eq!(loaded.content, b"col1,col2\na,b");
    assert_eq!(loaded.metadata["source"], "test");
}
```

## Streaming Large Artifacts

```rust
use swink_agent::StreamingArtifactStore;
use swink_agent_artifacts::FileArtifactStore;
use futures::stream;
use bytes::Bytes;

let store = FileArtifactStore::new("./artifacts");

// Save from a byte stream (e.g., from an HTTP response)
let chunks = stream::iter(vec![
    Ok(Bytes::from(vec![0u8; 1_000_000])),
    Ok(Bytes::from(vec![1u8; 1_000_000])),
]);

let version = store.save_stream(
    "session-1",
    "large-export.bin",
    "application/octet-stream".into(),
    Default::default(),
    Box::pin(chunks),
).await?;

// Load as a stream
if let Some(stream) = store.load_stream("session-1", "large-export.bin", None).await? {
    // Process chunks incrementally...
}
```
