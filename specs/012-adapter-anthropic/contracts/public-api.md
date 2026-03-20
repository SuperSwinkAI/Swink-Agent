# Public API Contract: Adapter: Anthropic

**Feature**: 012-adapter-anthropic | **Date**: 2026-03-20

## Module: `swink_agent_adapters` (re-export from `anthropic`)

```rust
/// A StreamFn implementation for the Anthropic Messages API.
///
/// Connects to the Anthropic API (or a compatible endpoint) and streams
/// responses as AssistantMessageEvent values. Supports text, thinking,
/// and tool-use content blocks.
pub struct AnthropicStreamFn { /* private fields */ }

impl AnthropicStreamFn {
    /// Create a new Anthropic stream function.
    ///
    /// * `base_url` - API base URL (e.g. `https://api.anthropic.com`).
    /// * `api_key` - Anthropic API key for `x-api-key` header authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self;
}

impl Debug for AnthropicStreamFn {
    // Redacts api_key as "[REDACTED]"
}

impl StreamFn for AnthropicStreamFn {
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

- `AnthropicStreamFn` is `Send + Sync` (compile-time assertion enforced).
- `new()` accepts any `Into<String>` for base URL and API key. No validation is performed at construction time.
- `stream()` returns a pinned stream that emits events in the following order:
  1. `Start` (exactly once, first event)
  2. Zero or more content block sequences, each consisting of:
     - `ThinkingStart` / `ThinkingDelta` / `ThinkingEnd` (if thinking is enabled)
     - `TextStart` / `TextDelta` / `TextEnd` (for text content)
     - `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` (for tool calls)
  3. Terminal event (exactly one): `Done` or `Error`

---

## Event Stream Contract

### Content Block Events

| Event | When | Key Fields |
|-------|------|------------|
| `TextStart` | `content_block_start` with `type: "text"` | `content_index` |
| `TextDelta` | `content_block_delta` with `type: "text_delta"` | `content_index`, `delta` |
| `TextEnd` | `content_block_stop` for text block | `content_index` |
| `ThinkingStart` | `content_block_start` with `type: "thinking"` | `content_index` |
| `ThinkingDelta` | `content_block_delta` with `type: "thinking_delta"` | `content_index`, `delta` |
| `ThinkingEnd` | `content_block_stop` for thinking block | `content_index`, `signature` (optional) |
| `ToolCallStart` | `content_block_start` with `type: "tool_use"` | `content_index`, `id`, `name` |
| `ToolCallDelta` | `content_block_delta` with `type: "input_json_delta"` | `content_index`, `delta` |
| `ToolCallEnd` | `content_block_stop` for tool-use block | `content_index` |

### Terminal Events

| Event | When | Key Fields |
|-------|------|------------|
| `Done` | `message_stop` SSE event | `stop_reason`, `usage`, `cost` |
| `Error` | HTTP error, SSE error event, or unexpected stream end | `stop_reason`, `error_message` |

---

## Error Classification Contract

| HTTP Status | Error Constructor | Retryable? |
|-------------|-------------------|------------|
| 401 | `error_auth()` | No |
| 429 | `error_throttled()` | Yes |
| 529 | `error_network()` | Yes |
| 504 | `error_network()` | Yes |
| 400-499 (other) | `error()` | No |
| 500-599 (other) | `error_network()` | Yes |
| Connection failure | `error_network()` | Yes |

---

## Message Conversion Contract

- System prompt is sent as the top-level `system` field, not as a message.
- Thinking blocks in assistant messages are stripped (Anthropic rejects them in outgoing requests).
- Empty text blocks in assistant messages are stripped.
- Consecutive tool results are merged into a single `user` message with multiple `tool_result` content blocks.
- `CustomMessage` variants in the agent message log are skipped.

---

## Thinking Configuration Contract

- Thinking is enabled when `ModelSpec.thinking_level != ThinkingLevel::Off`.
- Budget is resolved from `ModelSpec.thinking_budgets` map (keyed by `ThinkingLevel`), falling back to hardcoded defaults.
- Budget is capped to `max_tokens - 1` (Anthropic API requirement: budget < max_tokens).
- When thinking is enabled, `temperature` is forced to `None` (Anthropic API requirement).

---

## Re-export

```rust
// In adapters/src/lib.rs:
pub use anthropic::AnthropicStreamFn;
```

Consumers import via `swink_agent_adapters::AnthropicStreamFn`.
