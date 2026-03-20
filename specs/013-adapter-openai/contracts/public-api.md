# Public API Contract: Adapter: OpenAI

**Feature**: 013-adapter-openai | **Date**: 2026-03-20

## Module: `swink_agent_adapters` (re-export from `openai`)

```rust
/// A StreamFn implementation for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, vLLM, LM Studio, Groq, Together, and any other provider
/// that implements the OpenAI chat completions SSE streaming format.
pub struct OpenAiStreamFn { /* private fields */ }

impl OpenAiStreamFn {
    /// Create a new OpenAI-compatible stream function.
    ///
    /// * `base_url` - API base URL (e.g. `https://api.openai.com`).
    /// * `api_key` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self;
}

impl Debug for OpenAiStreamFn {
    // Redacts api_key as "[REDACTED]"
}

impl StreamFn for OpenAiStreamFn {
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

- `OpenAiStreamFn` is `Send + Sync` (compile-time assertion enforced).
- `new()` accepts any `Into<String>` for base URL and API key. No validation is performed at construction time.
- `stream()` returns a pinned stream that emits events in the following order:
  1. `Start` (exactly once, first event)
  2. Zero or more content block sequences, each consisting of:
     - `TextStart` / `TextDelta` / `TextEnd` (for text content)
     - `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` (for tool calls)
  3. Terminal event (exactly one): `Done` or `Error`

---

## Event Stream Contract

### Content Block Events

| Event | When | Key Fields |
|-------|------|------------|
| `TextStart` | First non-empty `content` delta in a choice | `content_index` |
| `TextDelta` | Each non-empty `content` delta | `content_index`, `delta` |
| `TextEnd` | Tool calls arrive after text, or `finish_reason` received | `content_index` |
| `ToolCallStart` | First delta for a new tool call index | `content_index`, `id`, `name` |
| `ToolCallDelta` | Each non-empty `arguments` delta | `content_index`, `delta` |
| `ToolCallEnd` | `finish_reason` received or stream ends | `content_index` |

### Terminal Events

| Event | When | Key Fields |
|-------|------|------------|
| `Done` | `[DONE]` sentinel received, or stream ends with a valid `finish_reason` | `stop_reason`, `usage`, `cost` |
| `Error` | HTTP error, JSON parse error, or unexpected stream end without `finish_reason` | `stop_reason`, `error_message` |

---

## Error Classification Contract

| HTTP Status | Error Constructor | Retryable? |
|-------------|-------------------|------------|
| 401, 403 | `error_auth()` | No |
| 429 | `error_throttled()` | Yes |
| 500-599 | `error_network()` | Yes |
| Other 4xx | `error()` | No |
| Connection failure | `error_network()` | Yes |

---

## Message Conversion Contract

- Uses shared `OaiConverter` via the `MessageConverter` trait (defined in `openai_compat.rs`).
- System prompt is sent as a `system` role message in the messages array.
- Assistant messages include `tool_calls` array when tool call content blocks are present.
- Tool results are sent as `tool` role messages with `tool_call_id`.
- `CustomMessage` variants in the agent message log are skipped.

---

## Tool Call ID Contract

- If the provider includes an `id` field on the first tool call delta, it is used as-is.
- If the provider omits the `id` field, a UUID is auto-generated with the format `tc_{uuid}`.

---

## Finish Reason Mapping Contract

| Provider `finish_reason` | `StopReason` |
|--------------------------|--------------|
| `"tool_calls"` | `StopReason::ToolUse` |
| `"length"` | `StopReason::Length` |
| `"stop"` | `StopReason::Stop` |
| `"content_filter"` | `StopReason::Stop` |
| Any other / unrecognized | `StopReason::Stop` |
| Absent (stream ends without) | `StopReason::Stop` (if `[DONE]` received) |

---

## Re-export

```rust
// In adapters/src/lib.rs:
pub use openai::OpenAiStreamFn;
```

Consumers import via `swink_agent_adapters::OpenAiStreamFn`.
