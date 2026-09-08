//! Tests for `base`.
#![cfg(test)]

use super::*;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[test]
fn merge_extra_typed_keys_win() {
    let extra: std::collections::BTreeMap<String, serde_json::Value> = [
        ("temperature".to_string(), serde_json::json!(0.1)),
        ("top_k".to_string(), serde_json::json!(40)),
    ]
    .into_iter()
    .collect();
    let mut body = serde_json::Map::new();
    body.insert("temperature".to_string(), serde_json::json!(0.7));

    merge_extra(&mut body, &extra, &["temperature"]);

    assert_eq!(body["temperature"], serde_json::json!(0.7));
    assert_eq!(body["top_k"], serde_json::json!(40));
}

#[test]
fn merge_extra_overwrites_untyped_collisions() {
    let extra = std::collections::BTreeMap::from([("seed".to_string(), serde_json::json!(2))]);
    let mut body = serde_json::Map::new();
    body.insert("seed".to_string(), serde_json::json!(1));

    merge_extra(&mut body, &extra, &[]);

    assert_eq!(body["seed"], serde_json::json!(2));
}

#[test]
fn merge_extra_empty_is_noop() {
    let mut body = serde_json::Map::new();
    merge_extra(&mut body, &std::collections::BTreeMap::new(), &["model"]);
    assert!(body.is_empty());
}

#[test]
fn trailing_slash_stripped() {
    let base = AdapterBase::new("https://api.example.com/", "key");
    assert_eq!(base.base_url, "https://api.example.com");
}

#[test]
fn multiple_trailing_slashes_stripped() {
    let base = AdapterBase::new("https://api.example.com///", "key");
    assert_eq!(base.base_url, "https://api.example.com");
}

#[test]
fn no_trailing_slash_unchanged() {
    let base = AdapterBase::new("https://api.example.com", "key");
    assert_eq!(base.base_url, "https://api.example.com");
}

#[test]
fn pre_stream_error_prefixes_start() {
    let events = pre_stream_error(swink_agent::AssistantMessageEvent::error("boom"));
    assert!(matches!(
        events,
        [
            swink_agent::AssistantMessageEvent::Start,
            swink_agent::AssistantMessageEvent::Error { .. }
        ]
    ));
}

#[test]
fn cancelled_error_uses_aborted_stop_reason() {
    let event = cancelled_error("cancelled");
    assert!(matches!(
        event,
        swink_agent::AssistantMessageEvent::Error {
            stop_reason: swink_agent::StopReason::Aborted,
            ..
        }
    ));
}

#[cfg(any(
    feature = "ollama",
    feature = "azure",
    feature = "proxy",
    feature = "gemini",
    feature = "bedrock"
))]
#[tokio::test]
async fn race_pre_stream_cancellation_short_circuits() {
    let token = CancellationToken::new();
    token.cancel();

    let result =
        race_pre_stream_cancellation(&token, "cancelled", async { Ok::<_, _>("ok") }).await;

    assert!(matches!(
        result,
        Err(swink_agent::AssistantMessageEvent::Error {
            stop_reason: swink_agent::StopReason::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn read_error_body_returns_aborted_when_cancelled_mid_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (write_body_tx, write_body_rx) = oneshot::channel::<()>();
    let (body_written_tx, body_written_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let response = concat!(
                "HTTP/1.1 500 Internal Server Error\r\n",
                "Content-Length: 128\r\n\r\n",
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = write_body_rx.await;
            let _ = socket.write_all(b"partial").await;
            let _ = body_written_tx.send(());
            std::future::pending::<()>().await;
        }
    });

    ensure_default_crypto_provider();
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    let token = CancellationToken::new();
    let cancel = token.clone();

    let read_task =
        tokio::spawn(
            async move { read_error_body_or_cancelled(response, &token, "cancelled").await },
        );
    write_body_tx.send(()).unwrap();
    body_written_rx.await.unwrap();
    cancel.cancel();
    let result = read_task.await.unwrap();

    assert!(matches!(
        result,
        Err(swink_agent::AssistantMessageEvent::Error {
            stop_reason: swink_agent::StopReason::Aborted,
            ..
        })
    ));
}

#[tokio::test]
async fn read_error_body_is_size_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = "x".repeat(MAX_ERROR_BODY_BYTES + 16);

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let header = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
        }
    });

    ensure_default_crypto_provider();
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    let token = CancellationToken::new();

    let body = read_error_body_or_cancelled(response, &token, "cancelled")
        .await
        .unwrap();

    assert_eq!(body.len(), MAX_ERROR_BODY_BYTES + "...[truncated]".len());
    assert!(body.ends_with("...[truncated]"));
}

#[tokio::test]
async fn adapter_http_client_times_out_between_body_reads() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Length: 128\r\n\r\n",
                "partial",
            );
            let _ = socket.write_all(response.as_bytes()).await;
            std::future::pending::<()>().await;
        }
    });

    let client =
        adapter_http_client_with_timeouts(Duration::from_secs(1), Duration::from_millis(50));
    let response = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect("connect");

    let err = tokio::time::timeout(Duration::from_secs(2), response.bytes())
        .await
        .expect("body read should complete with a reqwest timeout")
        .expect_err("body read should time out");

    assert!(err.is_timeout(), "expected reqwest timeout, got: {err}");
}

#[cfg(feature = "ollama")]
#[test]
fn local_read_timeout_exceeds_hosted_default() {
    // The local-inference client exists specifically to outlast the hosted
    // default during model cold-load (issue #920 regression caveat); if
    // these constants ever converge the override is pointless.
    assert!(LOCAL_READ_TIMEOUT > DEFAULT_READ_TIMEOUT);
}

#[cfg(feature = "ollama")]
#[test]
fn local_adapter_http_client_builds() {
    let _client = local_adapter_http_client();
}
