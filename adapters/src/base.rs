#![forbid(unsafe_code)]

#[cfg(any(
    feature = "ollama",
    feature = "azure",
    feature = "proxy",
    feature = "gemini",
    feature = "bedrock"
))]
use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Per-read idle timeout for local-inference servers (e.g. Ollama).
///
/// Cold-loading a large model into VRAM — or prefilling a huge prompt — can
/// legitimately sit silent for well over [`DEFAULT_READ_TIMEOUT`] before the
/// first streamed byte arrives (the regression caveat on issue #920). 600s is
/// generous enough for those cases while still bounding a truly wedged server.
#[cfg(feature = "ollama")]
const LOCAL_READ_TIMEOUT: Duration = Duration::from_secs(600);

/// Shared base for remote HTTP/SSE stream adapters.
///
/// Bundles the three fields that every reqwest-based adapter carries:
/// an endpoint base URL, an API key, and a shared HTTP client.  Using
/// this struct eliminates the repetitive `new()` constructor and
/// redacted [`std::fmt::Debug`] implementation across adapters.
///
/// ## Why `send_request` is not consolidated here
///
/// Each adapter has a `send_request` that follows a similar pattern (URL
/// construction, logging, serialize body, POST, check status) but the
/// differences are too significant for a safe shared abstraction:
///
/// - **Auth headers vary:** Anthropic uses `x-api-key`, `OpenAI` uses
///   `Authorization: Bearer`, Azure uses `api-key`, Google uses
///   `x-goog-api-key`, Bedrock uses AWS `SigV4` signing, Ollama uses none.
/// - **URL patterns differ:** Anthropic appends `/v1/messages`, `OpenAI`
///   appends `/v1/chat/completions`, Google encodes the model ID in the
///   path, Bedrock uses `/model/{id}/converse`.
/// - **Request body types are unique:** each adapter serializes a
///   provider-specific struct (not a shared type).
/// - **Bedrock uses the `ConverseStream` API** and requires `SigV4` request
///   signing — fundamentally different from the other adapters.
/// - **Proxy** doesn't use `AdapterBase` at all.
///
/// A generic helper would need a trait with associated types for the URL
/// builder, auth header builder, and request body — adding complexity
/// that exceeds the boilerplate it removes. HTTP status classification
/// (the truly duplicated logic) is handled by
/// [`classify::error_event_from_status`](crate::classify::error_event_from_status).
#[allow(dead_code)]
pub struct AdapterBase {
    pub base_url: String,
    pub api_key: String,
    pub client: reqwest::Client,
}

impl AdapterBase {
    #[allow(dead_code)]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: adapter_http_client(),
        }
    }
}

impl std::fmt::Debug for AdapterBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterBase")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Merge [`ServingOptions::extra`] entries into a JSON request-body map.
///
/// Single implementation of the documented merge rule shared by every
/// adapter: **typed request fields win over colliding `extra` keys**, so any
/// `extra` entry whose key appears in `typed_keys` is discarded. Every other
/// entry is inserted verbatim, overwriting a pre-existing entry with the same
/// key.
///
/// Callers decide what "typed" means for their wire format:
/// - adapters that serialize a typed struct (OAI transport, Mistral,
///   Anthropic, Gemini, Bedrock) pass the static list of field names the
///   struct can emit;
/// - adapters that build the map imperatively (Ollama) pass only the keys
///   they are about to insert, so an *unset* typed knob leaves the matching
///   `extra` entry intact.
///
/// `allow(dead_code)`: live only under provider features that build JSON
/// request bodies (same rationale as [`AdapterBase`]).
///
/// [`ServingOptions::extra`]: swink_agent::ServingOptions
#[allow(dead_code)]
pub(crate) fn merge_extra(
    body: &mut serde_json::Map<String, serde_json::Value>,
    extra: &std::collections::BTreeMap<String, serde_json::Value>,
    typed_keys: &[&str],
) {
    for (key, value) in extra {
        if typed_keys.contains(&key.as_str()) {
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
}

/// Prefix a pre-stream terminal error with `Start` so the core accumulator
/// still receives a valid stream envelope.
#[must_use]
pub const fn pre_stream_error(
    event: swink_agent::AssistantMessageEvent,
) -> [swink_agent::AssistantMessageEvent; 2] {
    [swink_agent::AssistantMessageEvent::Start, event]
}

/// Build a standard non-retryable cancellation terminal for pre-stream exits.
#[must_use]
pub fn cancelled_error(message: impl Into<String>) -> swink_agent::AssistantMessageEvent {
    swink_agent::AssistantMessageEvent::Error {
        stop_reason: swink_agent::StopReason::Aborted,
        error_message: message.into(),
        usage: None,
        error_kind: None,
        retry_after: None,
    }
}

/// If `started` is false, mark it true and prefix `event` with a synthetic
/// `Start` (via [`pre_stream_error`]); otherwise return `event` unprefixed.
#[cfg(any(feature = "bedrock", feature = "proxy"))]
#[must_use]
pub fn prefix_start_if_unstarted(
    event: swink_agent::AssistantMessageEvent,
    started: &mut bool,
) -> Vec<swink_agent::AssistantMessageEvent> {
    if *started {
        return vec![event];
    }
    *started = true;
    Vec::from(pre_stream_error(event))
}

/// Ensure a process-wide default rustls crypto provider is installed.
///
/// The workspace builds reqwest with `rustls-no-provider` so that the
/// default aws-lc-rs provider — whose `aws-lc-sys` build requires `cc` and
/// CMake (plus NASM on Windows) — never enters a consumer's dependency
/// tree (#1110). In that configuration reqwest refuses to construct a
/// `Client` (it panics in `ClientBuilder::build`) until a process default
/// [`rustls::crypto::CryptoProvider`] exists, so this installs ring.
///
/// Idempotent and race-safe: if a provider is already installed —
/// including a different one chosen by the host application, e.g.
/// aws-lc-rs for FIPS — the existing installation wins and this is a
/// no-op. Every adapter constructor calls it before building its HTTP
/// client; hosts that build their own `reqwest::Client` against the same
/// feature unification should call it too.
pub fn ensure_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Build the default HTTP client used by remote adapters.
///
/// Streaming endpoints should not use an overall request deadline, because a
/// valid response can run for minutes. Connect and per-read timeouts still keep
/// dead sockets from pinning a turn forever.
#[must_use]
pub(crate) fn adapter_http_client() -> reqwest::Client {
    adapter_http_client_with_timeouts(DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT)
}

/// Build the HTTP client used by local-inference adapters (e.g. Ollama).
///
/// Keeps the same connect timeout as [`adapter_http_client`] but uses the far
/// more generous [`LOCAL_READ_TIMEOUT`] per-read idle timeout, so model
/// cold-load or long prompt prefill does not trip the hosted-provider default.
#[cfg(feature = "ollama")]
#[must_use]
pub(crate) fn local_adapter_http_client() -> reqwest::Client {
    adapter_http_client_with_timeouts(DEFAULT_CONNECT_TIMEOUT, LOCAL_READ_TIMEOUT)
}

pub(crate) fn adapter_http_client_with_timeouts(
    connect_timeout: Duration,
    read_timeout: Duration,
) -> reqwest::Client {
    ensure_default_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .build()
        .expect("adapter HTTP client builder should be valid")
}

/// Race a pre-stream async operation against cancellation.
///
/// Adapters should use this around the initial HTTP send so cancellation can
/// short-circuit before any provider bytes arrive.
#[cfg(any(
    feature = "ollama",
    feature = "azure",
    feature = "proxy",
    feature = "gemini",
    feature = "bedrock"
))]
pub async fn race_pre_stream_cancellation<T, F>(
    cancellation_token: &CancellationToken,
    cancelled_message: &'static str,
    operation: F,
) -> Result<T, swink_agent::AssistantMessageEvent>
where
    F: Future<Output = Result<T, swink_agent::AssistantMessageEvent>>,
{
    if cancellation_token.is_cancelled() {
        return Err(cancelled_error(cancelled_message));
    }

    tokio::select! {
        () = cancellation_token.cancelled() => Err(cancelled_error(cancelled_message)),
        result = operation => result,
    }
}

/// Read an HTTP error response body without letting cancellation or very large
/// bodies keep an adapter alive indefinitely.
pub async fn read_error_body_or_cancelled(
    mut response: reqwest::Response,
    cancellation_token: &CancellationToken,
    cancelled_message: &'static str,
) -> Result<String, swink_agent::AssistantMessageEvent> {
    let mut bytes = Vec::new();
    let mut truncated = false;

    loop {
        tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                return Err(cancelled_error(cancelled_message));
            }
            chunk = response.chunk() => {
                match chunk {
                    Ok(Some(chunk)) => {
                        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
                        if remaining == 0 {
                            truncated = true;
                            break;
                        }
                        if remaining > 0 {
                            let take = remaining.min(chunk.len());
                            bytes.extend_from_slice(&chunk[..take]);
                        }
                        if chunk.len() > remaining {
                            truncated = true;
                            break;
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }

    let mut body = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        body.push_str("...[truncated]");
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
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

        let read_task = tokio::spawn(async move {
            read_error_body_or_cancelled(response, &token, "cancelled").await
        });
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
}
