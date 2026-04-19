//! Regression tests for `session_id` validation and canonical-root
//! containment in `FileArtifactStore`. These guard against path-traversal
//! attacks via a crafted `session_id` (see issue #609).

use std::collections::HashMap;

use bytes::Bytes;
use futures::stream;

use swink_agent::{ArtifactData, ArtifactError, ArtifactStore, StreamingArtifactStore};
use swink_agent_artifacts::FileArtifactStore;

fn text_data(content: &str) -> ArtifactData {
    ArtifactData {
        content: content.as_bytes().to_vec(),
        content_type: "text/plain".to_string(),
        metadata: HashMap::new(),
    }
}

fn stream_of(bytes: &'static [u8]) -> swink_agent::ArtifactByteStream {
    Box::pin(stream::iter(vec![Ok(Bytes::from_static(bytes))]))
}

fn assert_invalid_session_id(err: ArtifactError) {
    match err {
        ArtifactError::InvalidSessionId { .. } => {}
        other => panic!("expected InvalidSessionId, got: {other:?}"),
    }
}

// ─── Save: malicious session_id rejected ───────────────────────────────────

#[tokio::test]
async fn save_rejects_session_id_with_dotdot() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("../escape", "report.md", text_data("pwn"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);

    // Nothing escaped the tmp dir.
    let parent = tmpdir.path().parent().unwrap();
    let escape_path = parent.join("escape");
    assert!(
        !escape_path.exists(),
        "save with '../escape' must not materialize {escape_path:?}"
    );
}

#[tokio::test]
async fn save_rejects_session_id_with_forward_slash() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("nested/path", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn save_rejects_session_id_with_backslash() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("windows\\path", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn save_rejects_absolute_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    // Leading `/` tripping absolute-path interpretation on Unix.
    let err = store
        .save("/tmp/absolute", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);

    // Windows-style drive prefix.
    let err = store
        .save("C:\\absolute", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn save_rejects_session_id_with_null_byte() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("has\0null", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn save_rejects_session_id_with_control_chars() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("has\ttab", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);

    let err = store
        .save("has\nnewline", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn save_rejects_empty_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save("", "report.md", text_data("nope"))
        .await
        .unwrap_err();
    assert_invalid_session_id(err);
}

// ─── Load / list / delete also validate ────────────────────────────────────

#[tokio::test]
async fn load_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store.load("../etc", "passwd").await.unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn load_version_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store.load_version("../etc", "passwd", 1).await.unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn list_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store.list("../etc").await.unwrap_err();
    assert_invalid_session_id(err);
}

#[tokio::test]
async fn delete_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store.delete("../etc", "passwd").await.unwrap_err();
    assert_invalid_session_id(err);
}

// ─── Streaming APIs enforce the same validation ────────────────────────────

#[tokio::test]
async fn save_stream_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = store
        .save_stream(
            "../pwn",
            "evil.bin",
            "application/octet-stream".to_string(),
            HashMap::new(),
            stream_of(b"data"),
        )
        .await
        .unwrap_err();
    assert_invalid_session_id(err);

    // Ensure nothing landed on disk outside the artifact root.
    let parent = tmpdir.path().parent().unwrap();
    let escape_path = parent.join("pwn");
    assert!(
        !escape_path.exists(),
        "save_stream with '../pwn' must not materialize {escape_path:?}"
    );
}

#[tokio::test]
async fn load_stream_rejects_malicious_session_id() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());

    let err = match store.load_stream("../etc", "passwd", None).await {
        Ok(_) => panic!("load_stream should reject traversal session id"),
        Err(e) => e,
    };
    assert_invalid_session_id(err);
}

// ─── Happy path: a well-formed session_id still works across all APIs ──────

#[tokio::test]
async fn valid_session_id_round_trips() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());
    let session = "20260417_120000_abc123";

    // save
    store
        .save(session, "notes.md", text_data("v1"))
        .await
        .unwrap();

    // load
    let (data, version) = store.load(session, "notes.md").await.unwrap().unwrap();
    assert_eq!(data.content, b"v1");
    assert_eq!(version.version, 1);

    // load_version
    let (data_v1, _) = store
        .load_version(session, "notes.md", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(data_v1.content, b"v1");

    // list
    let metas = store.list(session).await.unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].name, "notes.md");

    // delete
    store.delete(session, "notes.md").await.unwrap();
    assert!(store.load(session, "notes.md").await.unwrap().is_none());
}

#[tokio::test]
async fn valid_session_id_round_trips_via_streaming() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let store = FileArtifactStore::new(tmpdir.path());
    let session = "session_abc_123";

    let version = store
        .save_stream(
            session,
            "big.bin",
            "application/octet-stream".to_string(),
            HashMap::new(),
            stream_of(b"streamed-content"),
        )
        .await
        .unwrap();
    assert_eq!(version.version, 1);

    let loaded = store
        .load_stream(session, "big.bin", None)
        .await
        .unwrap()
        .unwrap();

    use futures::StreamExt;
    let chunks: Vec<_> = loaded.map(|r| r.unwrap()).collect().await;
    let bytes: Vec<u8> = chunks.into_iter().flat_map(|b| b.to_vec()).collect();
    assert_eq!(bytes, b"streamed-content");
}
