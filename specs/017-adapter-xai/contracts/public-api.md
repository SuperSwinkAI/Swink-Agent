# Public API Contract: swink-agent-adapters (xAI)

**Feature**: 017-adapter-xai | **Date**: 2026-04-02

## Feature Gate

```toml
[dependencies]
swink-agent-adapters = { features = ["xai"] }
```

## Public Type

### `XAiStreamFn`

```rust
pub struct XAiStreamFn { /* private */ }

impl XAiStreamFn {
    /// Create a new xAI stream function.
    ///
    /// # Arguments
    /// * `base_url` - xAI API base URL (e.g. `https://api.x.ai`)
    /// * `api_key` - Bearer token for authentication
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self;
}

impl StreamFn for XAiStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}

impl Debug for XAiStreamFn { /* redacted credentials */ }
// Send + Sync enforced at compile time
```

## Usage Example

```rust
use swink_agent_adapters::XAiStreamFn;

let stream_fn = XAiStreamFn::new("https://api.x.ai", "xai-api-key-here");
// Use with Agent::new() or directly via stream_fn.stream()
```

## Wire Protocol

- **Endpoint**: `POST {base_url}/v1/chat/completions`
- **Auth**: `Authorization: Bearer {api_key}`
- **Request body**: OpenAI chat completions format (`OaiChatRequest`)
- **Response**: SSE stream of `chat.completion.chunk` objects
- **Terminal**: `data: [DONE]`

## Behavioral Contract

1. Text deltas arrive as individual `AssistantMessageEvent::TextDelta` events
2. Tool calls emit `ToolCallStart`, `ToolCallDelta`, then accumulated on `[DONE]`
3. HTTP errors classified via shared classifier (429→throttled, 401/403→auth, 5xx→network)
4. Network errors produce `AssistantMessageEvent::error_network()`
5. Cancellation token terminates the stream immediately
6. Usage data requested via `stream_options` and surfaced in terminal event
