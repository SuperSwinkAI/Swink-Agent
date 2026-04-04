use std::collections::HashMap;

use bytes::Bytes;
use futures::StreamExt;
use futures::stream;

use swink_agent::{ArtifactData, ArtifactError, ArtifactStore, StreamingArtifactStore};
use swink_agent_artifacts::FileArtifactStore;

/// Create a deterministic byte pattern of the given size.
fn pattern_bytes(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Collect a streaming load result into a `Vec<u8>`.
async fn collect_stream(
    stream: std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<Bytes, ArtifactError>> + Send>,
    >,
) -> Vec<u8> {
    let chunks: Vec<Bytes> = stream
        .map(|r| r.expect("stream chunk should succeed"))
        .collect()
        .await;
    chunks.into_iter().flat_map(|b| b.to_vec()).collect()
}

// T061: streaming_save_round_trip
// Stream 10MB in 64KB chunks, load via load_stream, verify content matches byte-for-byte.
#[tokio::test]
async fn streaming_save_round_trip() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let data = pattern_bytes(10_485_760); // 10 MiB

    // Build a stream of 64KB chunks.
    let chunk_size = 64 * 1024;
    let chunks: Vec<Result<Bytes, ArtifactError>> = data
        .chunks(chunk_size)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    let input_stream = Box::pin(stream::iter(chunks));

    let version = store
        .save_stream(
            "sess1",
            "big-file",
            "application/octet-stream".to_string(),
            HashMap::new(),
            input_stream,
        )
        .await
        .expect("save_stream should succeed");

    assert_eq!(version.version, 1);
    assert_eq!(version.size, 10_485_760);

    // Load back via streaming.
    let loaded_stream = store
        .load_stream("sess1", "big-file", None)
        .await
        .expect("load_stream should succeed")
        .expect("artifact should exist");

    let loaded = collect_stream(loaded_stream).await;
    assert_eq!(loaded.len(), data.len());
    assert_eq!(loaded, data);
}

// T062: streaming_save_creates_version
// Save via stream, verify ArtifactVersion returned with correct size.
#[tokio::test]
async fn streaming_save_creates_version() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let content = b"streaming version test";
    let input_stream = Box::pin(stream::iter(vec![
        Ok(Bytes::copy_from_slice(content.as_slice())),
    ]));

    let v1 = store
        .save_stream(
            "sess2",
            "versioned",
            "text/plain".to_string(),
            {
                let mut m = HashMap::new();
                m.insert("author".to_string(), "test".to_string());
                m
            },
            input_stream,
        )
        .await
        .expect("save_stream v1 should succeed");

    assert_eq!(v1.version, 1);
    assert_eq!(v1.size, content.len());
    assert_eq!(v1.content_type, "text/plain");

    // Save a second version.
    let content2 = b"updated content";
    let input_stream2 = Box::pin(stream::iter(vec![
        Ok(Bytes::copy_from_slice(content2.as_slice())),
    ]));

    let v2 = store
        .save_stream(
            "sess2",
            "versioned",
            "text/plain".to_string(),
            HashMap::new(),
            input_stream2,
        )
        .await
        .expect("save_stream v2 should succeed");

    assert_eq!(v2.version, 2);
    assert_eq!(v2.size, content2.len());

    // load_stream with version=None should return v2.
    let loaded = store
        .load_stream("sess2", "versioned", None)
        .await
        .unwrap()
        .unwrap();
    let bytes = collect_stream(loaded).await;
    assert_eq!(bytes, content2);

    // load_stream with version=Some(1) should return v1.
    let loaded_v1 = store
        .load_stream("sess2", "versioned", Some(1))
        .await
        .unwrap()
        .unwrap();
    let bytes_v1 = collect_stream(loaded_v1).await;
    assert_eq!(bytes_v1, content);
}

// T063: streaming_load_nonexistent_returns_none
// load_stream on unknown artifact returns None.
#[tokio::test]
async fn streaming_load_nonexistent_returns_none() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let result = store
        .load_stream("no-session", "no-artifact", None)
        .await
        .expect("load_stream should not error");

    assert!(result.is_none());

    // Also test with a specific version that doesn't exist.
    let result2 = store
        .load_stream("no-session", "no-artifact", Some(42))
        .await
        .expect("load_stream should not error");

    assert!(result2.is_none());
}

// T064: non_streaming_api_still_works
// Save via base Vec<u8> API (ArtifactStore::save), load via streaming (load_stream), verify compatible.
#[tokio::test]
async fn non_streaming_api_still_works() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let content = b"saved via regular API";
    let data = ArtifactData {
        content: content.to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    };

    let version = store
        .save("sess3", "compat-test", data)
        .await
        .expect("regular save should succeed");

    assert_eq!(version.version, 1);

    // Load via streaming API.
    let loaded_stream = store
        .load_stream("sess3", "compat-test", None)
        .await
        .expect("load_stream should succeed")
        .expect("artifact should exist");

    let loaded = collect_stream(loaded_stream).await;
    assert_eq!(loaded, content);

    // Also verify load_stream with explicit version.
    let loaded_v1 = store
        .load_stream("sess3", "compat-test", Some(1))
        .await
        .unwrap()
        .unwrap();
    let bytes_v1 = collect_stream(loaded_v1).await;
    assert_eq!(bytes_v1, content);
}
