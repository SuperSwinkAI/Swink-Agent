//! Tests for `download`: wiremock stands in for the hub and its CDN.
#![cfg(test)]

use std::sync::{Arc, Mutex};

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const BODY: &[u8] = b"GGUF-bytes";

fn recording_callback() -> (Arc<Mutex<Vec<ProgressEvent>>>, ProgressCallbackFn) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let cb: ProgressCallbackFn = Arc::new(move |event| sink.lock().unwrap().push(event));
    (events, cb)
}

/// Hub shape: `HEAD /resolve` answers with the metadata headers and a 302 to
/// the CDN; `GET /resolve` 302s to the CDN, which serves the bytes.
async fn hub_with_file(server: &MockServer) {
    hub_metadata(server).await;
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
        .mount(server)
        .await;
}

/// The hub half of [`hub_with_file`]; tests mount their own CDN behavior.
async fn hub_metadata(server: &MockServer) {
    let cdn = format!("{}/cdn/blob", server.uri());
    Mock::given(method("HEAD"))
        .and(path("/unsloth/synthetic-model/resolve/main/model.gguf"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", cdn.as_str())
                .insert_header("x-repo-commit", COMMIT)
                .insert_header("etag", "\"pointer-etag\"")
                .insert_header("x-linked-etag", "\"blob-etag\"")
                .insert_header("x-linked-size", BODY.len().to_string().as_str()),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unsloth/synthetic-model/resolve/main/model.gguf"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", cdn.as_str()))
        .mount(server)
        .await;
}

fn blob_dir(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("models--unsloth--synthetic-model/blobs")
}

/// Leave a partial download behind, as a failed earlier attempt would.
fn write_partial(temp: &tempfile::TempDir, bytes: &[u8]) {
    std::fs::create_dir_all(blob_dir(temp)).unwrap();
    std::fs::write(blob_dir(temp).join("blob-etag.incomplete"), bytes).unwrap();
}

fn no_range(req: &wiremock::Request) -> bool {
    !req.headers.contains_key("range")
}

async fn cdn_requests(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/cdn/blob")
        .collect()
}

#[tokio::test]
async fn downloads_into_the_hub_cache_layout_and_reports_progress() {
    let server = MockServer::start().await;
    hub_with_file(&server).await;
    let temp = tempfile::tempdir().unwrap();
    let client = HubClient::new(server.uri(), temp.path(), None);
    let (events, cb) = recording_callback();

    let resolved = client
        .download("unsloth/synthetic-model", "model.gguf", Some(cb))
        .await
        .unwrap();

    let repo = temp.path().join("models--unsloth--synthetic-model");
    assert_eq!(
        resolved,
        repo.join("snapshots").join(COMMIT).join("model.gguf")
    );
    assert_eq!(std::fs::read(&resolved).unwrap(), BODY);
    assert_eq!(
        std::fs::read_to_string(repo.join("refs/main")).unwrap(),
        COMMIT
    );
    assert_eq!(
        std::fs::read(repo.join("blobs/blob-etag")).unwrap(),
        BODY,
        "blob is keyed by the linked (LFS) etag without quotes"
    );
    let progress: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            ProgressEvent::DownloadProgress {
                bytes_downloaded,
                total_bytes,
            } => Some((*bytes_downloaded, *total_bytes)),
            _ => None,
        })
        .collect();
    assert_eq!(progress.first(), Some(&(0, Some(BODY.len() as u64))));
    assert_eq!(
        progress.last(),
        Some(&(BODY.len() as u64, Some(BODY.len() as u64)))
    );

    // Warm cache: the hub says 304 to the cached etag and the CDN is not hit
    // again.
    Mock::given(method("HEAD"))
        .and(path("/unsloth/synthetic-model/resolve/main/model.gguf"))
        .and(header("if-none-match", "\"blob-etag\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let again = client
        .download("unsloth/synthetic-model", "model.gguf", None)
        .await
        .unwrap();
    assert_eq!(again, resolved);
    let cdn_hits = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/cdn/blob")
        .count();
    assert_eq!(cdn_hits, 1, "revalidation must not re-download");
}

#[tokio::test]
async fn reuses_an_existing_blob_without_downloading() {
    let server = MockServer::start().await;
    hub_with_file(&server).await;
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("models--unsloth--synthetic-model");
    std::fs::create_dir_all(repo.join("blobs")).unwrap();
    std::fs::write(repo.join("blobs/blob-etag"), b"already here").unwrap();
    // Override: this run must not fetch from the CDN at all.
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let resolved = HubClient::new(server.uri(), temp.path(), None)
        .download("unsloth/synthetic-model", "model.gguf", None)
        .await
        .unwrap();
    assert_eq!(std::fs::read(resolved).unwrap(), b"already here");
}

#[tokio::test]
async fn falls_back_to_the_cache_when_the_hub_is_unreachable() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("models--unsloth--synthetic-model");
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs/main"), COMMIT).unwrap();
    let snapshot = repo.join("snapshots").join(COMMIT);
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::write(snapshot.join("model.gguf"), b"cached").unwrap();

    let client = HubClient::new("http://127.0.0.1:1", temp.path(), None);
    let resolved = client
        .download("unsloth/synthetic-model", "model.gguf", None)
        .await
        .unwrap();
    assert_eq!(resolved, snapshot.join("model.gguf"));

    let err = client
        .download("unsloth/synthetic-model", "other.gguf", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::Offline { .. }), "{err:?}");
}

#[tokio::test]
async fn rejects_bad_ids_and_surfaces_hub_errors() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    let client = HubClient::new(server.uri(), temp.path(), Some("secret".into()));

    let err = client
        .download("no-owner", "model.gguf", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::InvalidRepoId(_)), "{err:?}");
    let err = client
        .download("owner/name", "../escape.gguf", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::InvalidFilename(_)), "{err:?}");
    let err = client
        .download("owner/name", "missing.gguf", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, DownloadError::Status { status: 404, .. }),
        "{err:?}"
    );
    let sent = server.received_requests().await.unwrap();
    assert_eq!(
        sent[0].headers.get("authorization").unwrap(),
        "Bearer secret",
        "token goes to the hub"
    );
}

#[tokio::test]
async fn resumes_a_partial_download_with_a_range_request() {
    let server = MockServer::start().await;
    hub_metadata(&server).await;
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .and(header("range", "bytes=4-"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-9/10")
                .set_body_bytes(&BODY[4..]),
        )
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    write_partial(&temp, &BODY[..4]);
    let (events, cb) = recording_callback();

    let resolved = HubClient::new(server.uri(), temp.path(), None)
        .download("unsloth/synthetic-model", "model.gguf", Some(cb))
        .await
        .unwrap();

    assert_eq!(std::fs::read(&resolved).unwrap(), BODY);
    assert!(!blob_dir(&temp).join("blob-etag.incomplete").exists());
    assert_eq!(
        cdn_requests(&server).await.len(),
        1,
        "only the tail is fetched"
    );
    let first = events.lock().unwrap().first().cloned();
    assert!(
        matches!(
            first,
            Some(ProgressEvent::DownloadProgress {
                bytes_downloaded: 4,
                total_bytes: Some(10)
            })
        ),
        "progress starts at the resumed offset: {first:?}"
    );
}

#[tokio::test]
async fn restarts_when_the_server_ignores_the_range() {
    let server = MockServer::start().await;
    hub_with_file(&server).await;
    let temp = tempfile::tempdir().unwrap();
    write_partial(&temp, b"stale");

    let resolved = HubClient::new(server.uri(), temp.path(), None)
        .download("unsloth/synthetic-model", "model.gguf", None)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(resolved).unwrap(),
        BODY,
        "a 200 replaces the partial instead of appending to it"
    );
}

#[tokio::test]
async fn restarts_when_the_partial_is_not_resumable() {
    let server = MockServer::start().await;
    hub_metadata(&server).await;
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .and(header("range", "bytes=5-"))
        .respond_with(ResponseTemplate::new(416))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .and(no_range)
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    write_partial(&temp, b"stale");

    let resolved = HubClient::new(server.uri(), temp.path(), None)
        .download("unsloth/synthetic-model", "model.gguf", None)
        .await
        .unwrap();

    assert_eq!(std::fs::read(resolved).unwrap(), BODY);
    assert_eq!(
        cdn_requests(&server).await.len(),
        2,
        "range attempt, then full"
    );
}

#[tokio::test]
async fn concurrent_downloads_of_one_blob_fetch_it_once() {
    let server = MockServer::start().await;
    hub_metadata(&server).await;
    Mock::given(method("GET"))
        .and(path("/cdn/blob"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(BODY)
                .set_delay(std::time::Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    // Separate clients share nothing in memory, like separate processes; the
    // blob lock is the only thing that can serialize them.
    let a = HubClient::new(server.uri(), temp.path(), None);
    let b = HubClient::new(server.uri(), temp.path(), None);

    let (first, second) = tokio::join!(
        a.download("unsloth/synthetic-model", "model.gguf", None),
        b.download("unsloth/synthetic-model", "model.gguf", None),
    );

    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(
        cdn_requests(&server).await.len(),
        1,
        "the waiter reuses the finished blob"
    );
}
