#![forbid(unsafe_code)]

use std::future::Future;

use tokio_util::sync::CancellationToken;

const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

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
/// - **Bedrock is non-streaming** and requires `SigV4` request signing —
///   fundamentally different from the other adapters.
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
            client: reqwest::Client::new(),
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
    }
}

/// Race a pre-stream async operation against cancellation.
///
/// Adapters should use this around the initial HTTP send so cancellation can
/// short-circuit before any provider bytes arrive.
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

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                let response = concat!(
                    "HTTP/1.1 500 Internal Server Error\r\n",
                    "Content-Length: 128\r\n\r\n",
                    "partial",
                );
                let _ = socket.write_all(response.as_bytes()).await;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            cancel.cancel();
        });

        let result = read_error_body_or_cancelled(response, &token, "cancelled").await;

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
}
