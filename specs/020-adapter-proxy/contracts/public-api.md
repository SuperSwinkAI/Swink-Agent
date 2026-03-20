# Public API Contract: Adapter: Proxy

**Feature**: 020-adapter-proxy | **Date**: 2026-03-20

## Module: `swink_agent_adapters` (re-export from `proxy`)

```rust
/// A `StreamFn` implementation that proxies LLM calls over HTTP/SSE.
///
/// Sends a JSON POST to `{base_url}/v1/stream` with bearer token
/// authentication and parses the SSE response into `AssistantMessageEvent`
/// values.
pub struct ProxyStreamFn { /* private fields */ }

impl ProxyStreamFn {
    /// Create a new proxy stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the proxy server (without trailing slash).
    /// * `bearer_token` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self;
}

impl StreamFn for ProxyStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}

impl Debug for ProxyStreamFn {
    // Redacts bearer_token as "[redacted]"
}
```

**Contract**:

### Construction
- `ProxyStreamFn::new(base_url, bearer_token)` creates a ready-to-use adapter with a shared `reqwest::Client`.
- `base_url` must not include a trailing slash. The endpoint is `{base_url}/v1/stream`.

### Authentication
- Every request includes an `Authorization: Bearer {token}` header.
- If `StreamOptions.api_key` is `Some`, it overrides the stored bearer token for that request.
- The `Debug` implementation redacts the bearer token — it never appears in debug output.

### Request Format
- JSON POST body contains: `model` (string), `system` (string), `messages` (array of LLM messages), `options` (object with optional `temperature`, `max_tokens`, `session_id`).
- `CustomMessage` variants are filtered out — only `LlmMessage` variants are forwarded.
- Options fields that are `None` are omitted from the JSON body.

### SSE Response Protocol
- The proxy responds with an SSE stream where each `data:` field contains a JSON object with a `type` field.
- Supported event types: `start`, `text_start`, `text_delta`, `text_end`, `thinking_start`, `thinking_delta`, `thinking_end`, `tool_call_start`, `tool_call_delta`, `tool_call_end`, `done`, `error`.
- Each SSE event maps 1:1 to an `AssistantMessageEvent` variant. No state accumulation or delta reconstruction.
- The stream terminates on `done` or `error` events.

### Error Classification
- Connection failure (reqwest error on `.send()`) maps to `error_network`.
- HTTP 401/403 maps to `error_auth` (not retryable).
- HTTP 429 maps to `error_throttled` (retryable).
- HTTP 5xx maps to `error_network` (retryable).
- Malformed SSE JSON maps to `Error` with `stop_reason: Error` and diagnostic message including the parse error.
- SSE stream ending without a terminal event maps to `error_network("SSE stream ended unexpectedly")`.
- Cancellation via `CancellationToken` maps to `Error` with `stop_reason: Aborted`.

### Thread Safety
- `ProxyStreamFn` is `Send + Sync` (compile-time assertion).
- Multiple concurrent calls to `stream()` are safe — `reqwest::Client` handles connection pooling internally.
