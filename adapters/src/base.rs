#![forbid(unsafe_code)]

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

/// Build a terminal `Error` event carrying the `Aborted` stop reason.
///
/// Used by adapters when cancellation is observed before any stream content
/// has been emitted (e.g., before the HTTP send completes). The matching
/// semantic vocabulary is [`swink_agent::StopReason::Aborted`] — a plain
/// `AssistantMessageEvent::error(..)` masks intentional aborts as runtime
/// failures to the core loop.
#[must_use]
pub fn aborted_event(message: impl Into<String>) -> swink_agent::AssistantMessageEvent {
    swink_agent::AssistantMessageEvent::Error {
        stop_reason: swink_agent::StopReason::Aborted,
        error_message: message.into(),
        usage: None,
        error_kind: None,
    }
}

/// Build the two-event pre-stream sequence for a cancellation observed
/// before any content is emitted: `Start` followed by an
/// [`aborted_event`].
#[must_use]
pub fn pre_stream_aborted(message: impl Into<String>) -> [swink_agent::AssistantMessageEvent; 2] {
    pre_stream_error(aborted_event(message))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn aborted_event_uses_aborted_stop_reason() {
        let ev = aborted_event("cancelled");
        match ev {
            swink_agent::AssistantMessageEvent::Error {
                stop_reason,
                error_message,
                usage,
                error_kind,
            } => {
                assert_eq!(stop_reason, swink_agent::StopReason::Aborted);
                assert_eq!(error_message, "cancelled");
                assert!(usage.is_none());
                assert!(error_kind.is_none());
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn pre_stream_aborted_emits_start_then_aborted() {
        let events = pre_stream_aborted("cancelled");
        assert!(matches!(events[0], swink_agent::AssistantMessageEvent::Start));
        match &events[1] {
            swink_agent::AssistantMessageEvent::Error { stop_reason, .. } => {
                assert_eq!(*stop_reason, swink_agent::StopReason::Aborted);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
