use std::collections::HashMap;
use std::io::ErrorKind;

use bytes::Bytes;
use futures::StreamExt;
use futures::stream;

use swink_agent::{ArtifactData, ArtifactError, ArtifactStore, StreamingArtifactStore};
use swink_agent_artifacts::FileArtifactStore;

/// Create a deterministic byte pattern of the given size.
fn pattern_bytes(size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| u8::try_from(i % 256).expect("value is bounded to a byte"))
        .collect()
}

fn text_data(content: &str) -> ArtifactData {
    ArtifactData {
        content: content.as_bytes().to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    }
}

/// Collect a streaming load result into a `Vec<u8>`.
async fn collect_stream(
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Result<Bytes, ArtifactError>> + Send>>,
) -> Vec<u8> {
    let chunks: Vec<Bytes> = stream
        .map(|r| r.expect("stream chunk should succeed"))
        .collect()
        .await;
    chunks.into_iter().flat_map(|b| b.to_vec()).collect()
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

fn assert_storage_error_kind(err: ArtifactError, expected_kinds: &[ErrorKind]) {
    let ArtifactError::Storage(source) = err else {
        panic!("expected storage error, got {err:?}");
    };
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("storage error should wrap std::io::Error");
    assert!(
        expected_kinds.contains(&io.kind()),
        "expected one of {expected_kinds:?}, got {:?}",
        io.kind()
    );
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
    let input_stream = Box::pin(stream::iter(vec![Ok(Bytes::copy_from_slice(
        content.as_slice(),
    ))]));

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
    let input_stream2 = Box::pin(stream::iter(vec![Ok(Bytes::copy_from_slice(
        content2.as_slice(),
    ))]));

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

#[tokio::test]
async fn streaming_save_rolls_back_new_content_when_metadata_write_fails() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let initial_stream = Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"v1"))]));
    store
        .save_stream(
            "sess-rollback",
            "report.md",
            "text/plain".to_string(),
            HashMap::new(),
            initial_stream,
        )
        .await
        .expect("initial save_stream should succeed");

    let artifact_dir = tmpdir.path().join("sess-rollback").join("report.md");
    let meta_path = artifact_dir.join("meta.json");
    tokio::fs::remove_file(&meta_path)
        .await
        .expect("meta.json should be removable");
    tokio::fs::create_dir(&meta_path)
        .await
        .expect("directory replacement should succeed");

    let next_stream = Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"v2"))]));
    let err = store
        .save_stream(
            "sess-rollback",
            "report.md",
            "text/plain".to_string(),
            HashMap::new(),
            next_stream,
        )
        .await
        .expect_err("save_stream should fail when meta.json cannot be replaced");
    // Linux surfaces the failure as PermissionDenied; macOS (and other BSD
    // family kernels) surface it as IsADirectory. Both shapes are acceptable
    // — the contract is that the save errors, not the specific errno.
    assert_storage_error_kind(err, &[ErrorKind::PermissionDenied, ErrorKind::IsADirectory]);

    assert!(
        !artifact_dir.join("v2.bin").exists(),
        "new streamed content file should be rolled back on metadata write failure"
    );
    assert!(
        artifact_dir.join("v1.bin").exists(),
        "previous committed content must remain intact"
    );
}

#[tokio::test]
async fn streaming_load_returns_invalid_data_when_latest_content_file_is_missing() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let input_stream = Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"v1"))]));
    store
        .save_stream(
            "sess-missing-latest",
            "report.md",
            "text/plain".to_string(),
            HashMap::new(),
            input_stream,
        )
        .await
        .expect("save_stream should succeed");

    let content_path = tmpdir
        .path()
        .join("sess-missing-latest")
        .join("report.md")
        .join("v1.bin");
    tokio::fs::remove_file(&content_path)
        .await
        .expect("content file should be removable");

    let err = store
        .load_stream("sess-missing-latest", "report.md", None)
        .await
        .err()
        .expect("missing content should be surfaced as corruption");
    assert_invalid_data_storage_error(err, "metadata references missing content");
}

#[tokio::test]
async fn streaming_load_returns_invalid_data_for_orphaned_explicit_version_file() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    store
        .save(
            "sess-orphan",
            "report.md",
            ArtifactData {
                content: b"v1".to_vec(),
                content_type: "text/plain".to_string(),
                metadata: HashMap::new(),
            },
        )
        .await
        .expect("save should succeed");

    let artifact_dir = tmpdir.path().join("sess-orphan").join("report.md");
    tokio::fs::write(artifact_dir.join("v2.bin"), b"orphan")
        .await
        .expect("orphaned content file should be creatable");

    let err = store
        .load_stream("sess-orphan", "report.md", Some(2))
        .await
        .err()
        .expect("orphaned content should be surfaced as corruption");
    assert_invalid_data_storage_error(err, "without metadata membership");
}

#[tokio::test]
async fn streaming_load_latest_returns_invalid_data_for_orphaned_version_file_without_meta() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let artifact_dir = tmpdir
        .path()
        .join("sess-stream-orphan-latest")
        .join("report.md");
    tokio::fs::create_dir_all(&artifact_dir)
        .await
        .expect("artifact directory should be creatable");
    tokio::fs::write(artifact_dir.join("v1.bin"), b"orphan")
        .await
        .expect("orphaned content file should be creatable");

    let err = store
        .load_stream("sess-stream-orphan-latest", "report.md", None)
        .await
        .err()
        .expect("orphaned latest content should be surfaced as corruption");
    assert_invalid_data_storage_error(err, "without metadata membership");
}

#[tokio::test]
async fn streaming_save_refuses_to_overwrite_orphaned_next_version_file() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    store
        .save("sess-stream-orphan-save", "report.md", text_data("v1"))
        .await
        .expect("initial save should succeed");

    let artifact_dir = tmpdir
        .path()
        .join("sess-stream-orphan-save")
        .join("report.md");
    let orphan_path = artifact_dir.join("v2.bin");
    tokio::fs::write(&orphan_path, b"orphan")
        .await
        .expect("orphaned content file should be creatable");

    let err = store
        .save_stream(
            "sess-stream-orphan-save",
            "report.md",
            "text/plain".to_string(),
            HashMap::new(),
            Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"v2"))])),
        )
        .await
        .expect_err("save_stream should fail instead of overwriting orphaned content");
    assert_invalid_data_storage_error(err, "without metadata membership");

    let orphan = tokio::fs::read(&orphan_path)
        .await
        .expect("orphaned content should remain for diagnosis");
    assert_eq!(orphan, b"orphan");
}

// T065: streaming_save_error_does_not_publish_partial_version
// A failed stream write must not leave temp files behind or consume a version number.
#[tokio::test]
async fn streaming_save_error_does_not_publish_partial_version() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let input_stream = Box::pin(stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(ArtifactError::Storage(Box::new(std::io::Error::other(
            "stream failed",
        )))),
    ]));

    let result = store
        .save_stream(
            "sess4",
            "broken",
            "application/octet-stream".to_string(),
            HashMap::new(),
            input_stream,
        )
        .await;

    assert!(result.is_err());
    assert!(
        store.load("sess4", "broken").await.unwrap().is_none(),
        "failed stream must not publish a readable artifact"
    );

    let artifact_dir = tmpdir.path().join("sess4").join("broken");
    let entries: Vec<_> = std::fs::read_dir(&artifact_dir).unwrap().collect();
    assert!(
        entries.is_empty(),
        "failed stream should clean up temp files and avoid writing metadata"
    );

    let version = store
        .save_stream(
            "sess4",
            "broken",
            "application/octet-stream".to_string(),
            HashMap::new(),
            Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"recovered"))])),
        )
        .await
        .expect("subsequent save_stream should still succeed");

    assert_eq!(version.version, 1);
}
