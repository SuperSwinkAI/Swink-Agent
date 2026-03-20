# Public API Contract: Adapter Shared Infrastructure

**Feature**: 011-adapter-shared-infra | **Date**: 2026-03-20

## Module: `swink_agent_adapters::convert`

Re-exports from `swink_agent::convert`:

```rust
/// Trait for converting agent messages to provider-specific formats.
pub trait MessageConverter {
    // Adapter-defined associated types and methods for message conversion.
}

/// Generic driver: iterates agent messages and delegates to the converter.
pub fn convert_messages<C: MessageConverter>(
    converter: &C,
    messages: &[Message],
) -> Vec<serde_json::Value>;

/// Extracts JSON tool schemas from agent tools for provider payloads.
pub fn extract_tool_schemas(tools: &[Arc<dyn AgentTool>]) -> Vec<ToolSchema>;
```

**Contract**: Every adapter that converts messages MUST implement `MessageConverter`. The `convert_messages` function is the canonical entry point — adapters do not roll their own iteration logic (exception: Anthropic, which has structural differences in system prompt handling).

---

## Module: `swink_agent_adapters::classify`

```rust
/// Classification of HTTP error status codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpErrorKind {
    Auth,       // 401, 403 — not retryable
    Throttled,  // 429 — retryable
    Network,    // 5xx — retryable
}

/// Default classification. Returns None for non-error codes.
pub const fn classify_http_status(code: u16) -> Option<HttpErrorKind>;

/// Classification with provider-specific overrides applied first.
pub fn classify_with_overrides(
    code: u16,
    overrides: &[(u16, HttpErrorKind)],
) -> Option<HttpErrorKind>;
```

**Contract**:
- `classify_http_status(429)` always returns `Some(HttpErrorKind::Throttled)`.
- `classify_http_status(401)` and `classify_http_status(403)` always return `Some(HttpErrorKind::Auth)`.
- `classify_http_status(500..=599)` always returns `Some(HttpErrorKind::Network)`.
- `classify_http_status(200)` returns `None`.
- `classify_with_overrides` checks the override slice before falling back to `classify_http_status`.

---

## Module: `swink_agent_adapters::sse`

```rust
/// Parsed SSE line variant.
#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    Event(String),  // event type label
    Data(String),   // JSON data payload
    Done,           // terminal signal: data: [DONE]
    Empty,          // blank line (event separator)
}

/// Streaming SSE parser that buffers bytes and yields parsed lines.
pub struct SseStreamParser { /* buffer: String */ }

impl SseStreamParser {
    pub const fn new() -> Self;
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseLine>;
    pub fn flush(&mut self) -> Vec<SseLine>;
}

impl Default for SseStreamParser { /* delegates to new() */ }

/// Stream combinator: byte stream -> filtered stream of Data and Done lines.
pub fn sse_data_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>>;
```

**Contract**:
- `feed()` accumulates bytes and yields complete SSE lines as they become available.
- Partial lines are buffered until the next `feed()` or `flush()`.
- SSE comments (lines starting with `:`) are silently skipped.
- `data: [DONE]` yields `SseLine::Done`, not `SseLine::Data("[DONE]")`.
- `sse_data_lines` filters out `Event` and `Empty` variants, yielding only `Data` and `Done`.

---

## Module: `swink_agent_adapters` (root re-exports from `remote_presets`)

```rust
/// Compile-time key for a remote model preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemotePresetKey {
    pub provider_key: &'static str,
    pub preset_id: &'static str,
}

impl RemotePresetKey {
    pub const fn new(provider_key: &'static str, preset_id: &'static str) -> Self;
}

/// Nested modules of compile-time preset constants.
pub mod remote_preset_keys {
    pub mod anthropic { /* OPUS_46, SONNET_46, HAIKU_45 */ }
    pub mod openai { /* GPT_4O, GPT_4_1, GPT_4O_MINI, GPT_4_1_MINI, O3_MINI, O1 */ }
    pub mod google { /* GEMINI_3_1_PRO, GEMINI_3_1_DEEP_THINK, GEMINI_3_FLASH, GEMINI_3_1_FLASH_LITE */ }
    pub mod azure { /* GPT_4O, GPT_4O_MINI, PHI_4 */ }
    pub mod xai { /* GROK_3, GROK_3_FAST */ }
    pub mod mistral { /* MISTRAL_MEDIUM, MISTRAL_SMALL, CODESTRAL */ }
    pub mod bedrock { /* ANTHROPIC_CLAUDE_SONNET_45, META_LLAMA_4_MAVERICK, MISTRAL_PIXTRAL_LARGE, AMAZON_NOVA_PRO, AI21_JAMBA_1_5_LARGE */ }
}

/// Error type for remote connection construction.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteModelConnectionError {
    UnknownPreset { provider_key, preset_id },
    NotRemotePreset { provider_key, preset_id },
    MissingCredential { preset, env_var },
    MissingBaseUrl { preset, env_var },
    MissingRegion { preset, env_var },
    MissingAwsCredentials { preset },
}

/// List all remote presets, optionally filtered by provider.
pub fn remote_presets(provider_key: Option<&str>) -> Vec<CatalogPreset>;

/// Build a fully configured ModelConnection from a preset key.
pub fn build_remote_connection(
    key: RemotePresetKey,
) -> Result<ModelConnection, RemoteModelConnectionError>;

/// Find a preset by model_id string (e.g., "claude-sonnet-4-6").
pub fn preset(model_id: &str) -> Option<CatalogPreset>;
```

**Contract**:
- `build_remote_connection` resolves credentials from environment variables. Returns `MissingCredential` if the required env var is unset.
- `remote_presets(None)` returns all remote presets across all providers.
- `remote_presets(Some("anthropic"))` returns only Anthropic presets.
- `preset("claude-sonnet-4-6")` returns the matching `CatalogPreset` or `None`.
- Adding a new provider requires: one new `remote_preset_keys` sub-module, one new match arm in `build_remote_connection_from_values`, and the corresponding `StreamFn` import.
