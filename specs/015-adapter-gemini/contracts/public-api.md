# Public API Contract: Adapter: Google Gemini

**Feature**: 015-adapter-gemini | **Date**: 2026-03-24

## Module: `swink_agent_adapters` (re-export from `google`)

```rust
/// A StreamFn implementation for the Google Generative Language API.
///
/// Connects to the Gemini streaming endpoint and streams responses as
/// AssistantMessageEvent values. Supports text, thinking, and tool-use
/// content blocks via Server-Sent Events.
pub struct GeminiStreamFn { /* private fields */ }

impl GeminiStreamFn {
    /// Create a new Gemini stream function.
    ///
    /// * `base_url` - API base URL (e.g. `https://generativelanguage.googleapis.com`).
    /// * `api_key` - Google API key for `x-goog-api-key` header authentication.
    /// * `api_version` - `ApiVersion::V1` or `ApiVersion::V1beta`.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        api_version: ApiVersion,
    ) -> Self;
}

impl Debug for GeminiStreamFn {
    // Redacts api_key as "[REDACTED]"
}

impl StreamFn for GeminiStreamFn {
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

- `GeminiStreamFn` is `Send + Sync` (compile-time assertion enforced).
- `new()` accepts any `Into<String>` for base URL and API key. Trailing `/` is stripped from `base_url`. No validation is performed at construction time.
- `stream()` returns a pinned stream that emits events in the following order:
  1. `Start` (exactly once, first event)
  2. Zero or more content block sequences, each consisting of:
     - `ThinkingStart` / `ThinkingDelta` / `ThinkingEnd` (if thinking is active)
     - `TextStart` / `TextDelta` / `TextEnd` (for text content)
     - `ToolCallStart` / `ToolCallDelta` / `ToolCallEnd` (for function calls)
  3. Terminal event (exactly one): `Done` or `Error`

---

## Event Stream Contract

### Content Block Events

| Event | When | Key Fields |
|-------|------|------------|
| `TextStart` | First text part in a chunk | `content_index` |
| `TextDelta` | Text part with non-empty text | `content_index`, `delta` |
| `TextEnd` | Next chunk switches to different block type or stream ends | `content_index` |
| `ThinkingStart` | Part with `thought: true` | `content_index` |
| `ThinkingDelta` | Thinking part with non-empty text | `content_index`, `delta` |
| `ThinkingEnd` | Next chunk switches to different block type or stream ends | `content_index`, `signature` (optional) |
| `ToolCallStart` | Part with `functionCall` | `content_index`, `id`, `name` |
| `ToolCallDelta` | Function call with growing `args` | `content_index`, `delta` |
| `ToolCallEnd` | Stream finalization (via `StreamFinalize`) | `content_index` |

### Terminal Events

| Event | When | Key Fields |
|-------|------|------------|
| `Done` | `[DONE]` SSE sentinel or stream end | `stop_reason`, `usage`, `cost` |
| `Error` | HTTP error, safety block, JSON parse error, or network failure | `stop_reason`, `error_message` |

---

## Error Classification Contract

| HTTP Status | Error Constructor | Retryable? |
|-------------|-------------------|------------|
| 401 | `error_auth()` | No |
| 403 | `error_auth()` | No |
| 429 | `error_throttled()` | Yes |
| 400-499 (other) | `error()` | No |
| 500-599 | `error_network()` | Yes |
| Connection failure | `error_network()` | Yes |

### Safety Filter Contract

| Condition | Behavior |
|-----------|----------|
| `finish_reason == "SAFETY"` | Emit `AssistantMessageEvent::error()` with descriptive message |

---

## Message Conversion Contract

- System prompt is sent as the top-level `systemInstruction` field, not a message.
- User messages → `role: "user"` content with text and `inlineData` parts.
- Assistant messages → `role: "model"` content with text, thinking, and `functionCall` parts.
- Tool results → `role: "user"` content with `functionResponse` parts.
- Thinking blocks are preserved in outgoing requests (with `thought: true` and `thoughtSignature`).
- Images are converted to `inlineData` parts with `mimeType` + base64 `data`, or `fileData` with `fileUri`.
- `CustomMessage` variants in the agent message log are skipped.
- Only the first candidate in multi-candidate responses is processed.
- Tool definition schemas are passed through as-is to `functionDeclarations`.

---

## Thinking Configuration Contract

- Thinking is enabled when the conversation history contains thinking blocks, tool calls, or tool definitions are present.
- When enabled, `generationConfig.thinkingConfig.includeThoughts` is set to `true`.
- Thinking parts are identified by the `thought: true` flag on response parts.
- `thoughtSignature` is buffered and emitted with `ThinkingEnd`.

---

## Re-export

```rust
// In adapters/src/lib.rs:
pub use google::GeminiStreamFn;
```

Consumers import via `swink_agent_adapters::GeminiStreamFn`.
