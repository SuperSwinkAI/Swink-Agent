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
    /// Static headers sent on every request this adapter issues.
    ///
    /// Empty by default, so an adapter that sets none produces a
    /// byte-identical request to one built before this field existed. A
    /// transport applies these *instead of* its own `Authorization` header
    /// when the map carries one, which lets a provider use a non-Bearer
    /// scheme without a bespoke transport.
    ///
    /// Static per-adapter values only — a per-request header is the
    /// adapter's business to generate, not the caller's.
    pub headers: reqwest::header::HeaderMap,
}

impl AdapterBase {
    #[allow(dead_code)]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: adapter_http_client(),
            headers: reqwest::header::HeaderMap::new(),
        }
    }

    /// Add one static header, replacing any previous value for that name.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }
}

impl std::fmt::Debug for AdapterBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Header *values* can carry credentials (a caller-supplied
        // `Authorization`, an account id), so only the names are printed.
        let header_names: Vec<&str> = self
            .headers
            .keys()
            .map(reqwest::header::HeaderName::as_str)
            .collect();
        f.debug_struct("AdapterBase")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("headers", &header_names)
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

/// Hand the response headers to `on_rate_limit`, if set.
///
/// Called exactly once per request, right after the response arrives and
/// before the status check, so a 429's quota headers reach the caller too.
/// Non-UTF-8 header values are skipped rather than failing the turn.
#[allow(dead_code)]
pub(crate) fn report_rate_limit(
    headers: &reqwest::header::HeaderMap,
    on_rate_limit: Option<&swink_agent::OnRateLimit>,
) {
    let Some(callback) = on_rate_limit else {
        return;
    };
    let snapshot = swink_agent::RateLimitSnapshot::from_headers(
        headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str(), v))),
    );
    callback(&snapshot);
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
    feature = "bedrock",
    feature = "responses"
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
#[path = "base_tests.rs"]
mod tests;
