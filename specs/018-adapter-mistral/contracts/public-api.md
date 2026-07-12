# Public API Contract: Adapter — Mistral

**Feature**: 018-adapter-mistral
**Date**: 2026-03-30

## Exported Types

### `MistralStreamFn`

```rust
/// Mistral chat completions adapter with request/response normalization.
///
/// Handles Mistral-specific API divergences from the OpenAI protocol:
/// - Tool call ID format (9-char alphanumeric)
/// - `max_tokens` instead of `max_completion_tokens`
/// - No `stream_options` parameter
/// - `model_length` finish reason mapping
/// - Message ordering constraints (no user after tool)
pub struct MistralStreamFn { /* private fields */ }

impl MistralStreamFn {
    /// Create a new Mistral adapter.
    ///
    /// # Arguments
    /// - `base_url`: Mistral API base URL (e.g., `https://api.mistral.ai`)
    /// - `api_key`: Mistral API key for Bearer authentication
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self;
}

impl Debug for MistralStreamFn { /* redacts api_key */ }

impl StreamFn for MistralStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}
```

## Feature Gate

```toml
[features]
mistral = []  # No additional dependencies
```

Enabled by default via `all` feature. When disabled, `MistralStreamFn` is not compiled.

## Re-export Path

```rust
// In adapters/src/lib.rs
#[cfg(feature = "mistral")]
pub use mistral::MistralStreamFn;
```

Consumer import: `use swink_agent_adapters::MistralStreamFn;`

## Preset Keys

Mistral presets are not exposed as named constants. They are looked up
dynamically from the model catalog via `RemotePresetKey::new`:

```rust
// In remote_presets module
RemotePresetKey::new("mistral", "mistral_large")
RemotePresetKey::new("mistral", "mistral_medium")
RemotePresetKey::new("mistral", "mistral_small")
RemotePresetKey::new("mistral", "codestral")
RemotePresetKey::new("mistral", "devstral")
RemotePresetKey::new("mistral", "pixtral_large")
RemotePresetKey::new("mistral", "pixtral_12b")
RemotePresetKey::new("mistral", "ministral_3b")
RemotePresetKey::new("mistral", "ministral_8b")
RemotePresetKey::new("mistral", "ministral_14b")
RemotePresetKey::new("mistral", "magistral_medium")
RemotePresetKey::new("mistral", "magistral_small")
```

Pass the resulting key to `build_remote_connection()`, or look up a preset by
`model_id` directly via `preset("mistral-large-latest")`. The catalog is the
source of truth for the current set of preset IDs (`src/model_catalog.toml`).

## Wire Protocol

| Property | Value |
|---|---|
| Endpoint | `{base_url}/v1/chat/completions` |
| Method | POST |
| Auth | `Authorization: Bearer {api_key}` |
| Content-Type | `application/json` |
| Streaming | SSE (`text/event-stream`) |
| Sentinel | `data: [DONE]` |

## Behavioral Contract

1. Events emitted follow the `AssistantMessageEvent` sequence: `Start` → (`TextStart`/`TextDelta`/`TextEnd` | `ToolCallStart`/`ToolCallDelta`/`ToolCallEnd`)* → `Done`/`Error`
2. Tool call IDs in emitted events use harness format (`call_*`), never Mistral's 9-char format
3. `finish_reason: "model_length"` transparently maps to `StopReason::Length`
4. Usage is extracted from the final SSE chunk (no `stream_options` sent)
5. Stream cancellation via `CancellationToken` closes open blocks via shared finalization
