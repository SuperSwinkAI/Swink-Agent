# Public API Contract: Adapter: Ollama

**Feature**: 014-adapter-ollama | **Date**: 2026-03-20

## Module: `swink_agent_adapters` (re-export from `ollama`)

```rust
/// A StreamFn implementation for Ollama's `/api/chat` endpoint.
///
/// Connects to a local or remote Ollama instance and streams responses
/// as `AssistantMessageEvent` values via NDJSON (newline-delimited JSON).
pub struct OllamaStreamFn { /* private fields */ }

impl OllamaStreamFn {
    /// Create a new Ollama stream function.
    ///
    /// * `base_url` - Ollama server URL (e.g. `http://localhost:11434`).
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self;
}

impl Debug for OllamaStreamFn {
    // Shows base_url, omits client internals via finish_non_exhaustive()
}

impl StreamFn for OllamaStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}
```

**Contract**:

- `OllamaStreamFn` is `Send + Sync` (compile-time assertion enforced).
- `new()` accepts any `Into<String>` for the base URL. No authentication is required (Ollama has no auth layer).
- `stream()` returns a pinned stream that emits events in the following order:
  1. `Start` (exactly once, first event)
  2. Zero or more content block sequences, each consisting of:
     - `ThinkingStart` / `ThinkingDelta` / `ThinkingEnd` (for thinking/reasoning content, model-dependent)
     - `TextStart` / `TextDelta` / `TextEnd` (for text content)
     - `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` (for tool calls)
  3. Terminal event (exactly one): `Done` or `Error`

---

## Event Stream Contract

### Content Block Events

| Event | When | Key Fields |
|-------|------|------------|
| `ThinkingStart` | First non-empty `thinking` field in a chunk | `content_index` |
| `ThinkingDelta` | Each non-empty `thinking` field | `content_index`, `delta` |
| `ThinkingEnd` | Text content arrives after thinking, or stream ends | `content_index` |
| `TextStart` | First non-empty `content` field in a chunk | `content_index` |
| `TextDelta` | Each non-empty `content` field | `content_index`, `delta` |
| `TextEnd` | Tool calls arrive after text, or `done: true` | `content_index` |
| `ToolCallStart` | First occurrence of a tool call (by name) | `content_index`, `id`, `name` |
| `ToolCallDelta` | Same event as start (complete arguments in one delta) | `content_index`, `delta` |
| `ToolCallEnd` | Immediately after delta (complete tool call in one chunk) | `content_index` |

### Terminal Events

| Event | When | Key Fields |
|-------|------|------------|
| `Done` | Chunk with `done: true` received | `stop_reason`, `usage`, `cost` |
| `Error` | HTTP error, connection failure, JSON parse error, or unexpected stream end | `stop_reason`, `error_message` |

---

## Error Classification Contract

| Condition | Error Constructor | Retryable? |
|-----------|-------------------|------------|
| Connection refused (Ollama not running) | `error_network()` | Yes |
| Network timeout | `error_network()` | Yes |
| Non-success HTTP status | `error_network()` | Yes |
| NDJSON parse error mid-stream | `error()` | No |
| Unexpected stream end (no `done: true`) | `error()` | No |

---

## Message Conversion Contract

- Uses `OllamaConverter` via the `MessageConverter` trait.
- System prompt is sent as a `system` role message in the messages array.
- Assistant messages include `tool_calls` array when tool call content blocks are present.
- Tool results are sent as `tool` role messages with the tool result content.
- `CustomMessage` variants in the agent message log are skipped (handled by `convert_messages`).

---

## Done Reason Mapping Contract

| Ollama `done_reason` | `StopReason` |
|----------------------|--------------|
| `"tool_calls"` | `StopReason::ToolUse` |
| `"length"` | `StopReason::Length` |
| `"stop"` | `StopReason::Stop` |
| Any other / absent | `StopReason::Stop` |

---

## Cost Contract

Ollama is a local/free service. All cost fields (`input`, `output`, `cache_read`, `cache_write`, `total`) are `0.0`.

---

## Re-export

```rust
// In adapters/src/lib.rs:
pub use ollama::OllamaStreamFn;
```

Consumers import via `swink_agent_adapters::OllamaStreamFn`.
