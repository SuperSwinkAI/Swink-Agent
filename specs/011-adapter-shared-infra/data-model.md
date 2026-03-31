# Data Model: Adapter Shared Infrastructure

**Feature**: 011-adapter-shared-infra | **Date**: 2026-03-20

## Entity: MessageConverter (trait, defined in core)

**Location**: `src/convert.rs` (core), re-exported from `adapters/src/convert.rs`

| Method | Signature | Purpose |
|--------|-----------|---------|
| *Trait methods* | Per-adapter | Convert agent messages, content blocks, tool calls, and tool results to provider-specific JSON structures |

**Companion function**: `convert_messages<C: MessageConverter>(converter: &C, messages: &[Message]) -> Vec<ProviderMessage>` — generic driver that iterates messages and delegates to the converter.

**Companion function**: `extract_tool_schemas(tools: &[Arc<dyn AgentTool>]) -> Vec<ToolSchema>` — extracts JSON schemas from agent tools for provider tool-use payloads.

**Re-export**: `adapters/src/convert.rs` re-exports `MessageConverter`, `convert_messages`, and `extract_tool_schemas` from core so adapters import from one place.

---

## Entity: HttpErrorKind (enum)

**Location**: `adapters/src/classify.rs`

| Variant | Maps to | Retryable? |
|---------|---------|------------|
| `Auth` | 401, 403 — authentication/authorization failure | No |
| `Throttled` | 429 — rate limit | Yes |
| `Network` | 500–599 — server/network error | Yes |

**Derives**: `Debug, Clone, PartialEq, Eq`

**Functions**:

| Function | Signature | Notes |
|----------|-----------|-------|
| `classify_http_status` | `const fn(u16) -> Option<HttpErrorKind>` | Default mapping |
| `classify_with_overrides` | `fn(u16, &[(u16, HttpErrorKind)]) -> Option<HttpErrorKind>` | Provider overrides checked first |

---

## Entity: SseLine (enum)

**Location**: `adapters/src/sse.rs`

| Variant | Data | Purpose |
|---------|------|---------|
| `Event(String)` | Event type label | e.g., `event: message_start` |
| `Data(String)` | JSON payload | e.g., `data: {"text":"hello"}` |
| `Done` | — | Terminal signal `data: [DONE]` |
| `Empty` | — | Blank line (event separator) |

**Derives**: `Debug, PartialEq, Eq`

---

## Entity: SseStreamParser (struct)

**Location**: `adapters/src/sse.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `buffer` | `String` | Accumulates partial UTF-8 chunks between `feed()` calls |

| Method | Signature | Purpose |
|--------|-----------|---------|
| `new()` | `const fn() -> Self` | Empty parser |
| `feed(&mut self, &[u8]) -> Vec<SseLine>` | Feed bytes, yield complete lines |
| `flush(&mut self) -> Vec<SseLine>` | Drain remaining buffer at stream end |

**Companion function**: `sse_data_lines(byte_stream) -> Pin<Box<dyn Stream<Item = SseLine>>>` — stream combinator that filters to `Data` and `Done` variants.

---

## Entity: RemotePresetKey (struct)

**Location**: `adapters/src/remote_presets.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `provider_key` | `&'static str` | Provider identifier (e.g., `"anthropic"`) |
| `preset_id` | `&'static str` | Preset identifier (e.g., `"sonnet_46"`) |

**Derives**: `Debug, Clone, Copy, PartialEq, Eq, Hash`

---

## Entity: RemoteModelConnectionError (enum)

**Location**: `adapters/src/remote_presets.rs`

| Variant | Fields | Purpose |
|---------|--------|---------|
| `UnknownPreset` | `provider_key, preset_id` | Preset not found in catalog |
| `NotRemotePreset` | `provider_key, preset_id` | Preset exists but is not remote |
| `MissingCredential` | `preset, env_var` | API key env var not set |
| `MissingBaseUrl` | `preset, env_var` | Base URL env var not set |
| `MissingRegion` | `preset, env_var` | AWS region env var not set |
| `MissingAwsCredentials` | `preset` | AWS access key or secret not set |

**Derives**: `Debug, Error (thiserror), PartialEq, Eq`

---

## Entity: remote_preset_keys (module of constants)

**Location**: `adapters/src/remote_presets.rs`

Nested modules (`anthropic`, `openai`, `google`, `azure`, `xai`, `mistral`, `bedrock`) each containing `const RemotePresetKey` values for every known model preset. These are compile-time constants used as keys for `build_remote_connection()`.

---

## Entity: CacheStrategy (enum, defined in core)

**Location**: `src/stream.rs` (core), as a field on `StreamOptions`

| Variant | Data | Purpose |
|---------|------|---------|
| `None` | — | No caching (default) |
| `Auto` | — | Adapter decides optimal cache points |
| `Anthropic` | — | Anthropic-specific `cache_control` blocks |
| `Google` | `ttl: Duration` | Google context caching with explicit TTL |

**Derives**: `Debug, Clone, Default` (default = `None`)

**Flow**: `AgentOptions` → `StreamOptions.cache_strategy` → adapter's `apply_cache_strategy()`

---

## Entity: ProxyStreamFn (struct)

**Location**: `adapters/src/proxy.rs` (or new `adapters/src/proxy_raw.rs`)

| Field | Type | Purpose |
|-------|------|---------|
| `base` | `AdapterBase` | Shared HTTP infrastructure |
| `target_provider` | `String` | Provider format for URL/auth routing |

**Returns**: `Stream<Item = Result<Bytes, reqwest::Error>>` — raw SSE bytes, not parsed events

**Contract**: Does NOT implement `StreamFn` (which returns `AssistantMessageEvent`). Instead provides a separate method or trait for raw byte streaming.

---

## Entity: OnRawPayload (type alias, defined in core)

**Location**: `src/stream.rs` (core), as a field on `StreamOptions`

```
pub type OnRawPayload = Arc<dyn Fn(&str) + Send + Sync>;
```

**Field on StreamOptions**: `on_raw_payload: Option<OnRawPayload>`

**Contract**: Called with each raw SSE `data:` line string before event parsing. Panics caught via `catch_unwind`. Must return quickly.

---

## Relationship Diagram

```text
swink-agent (core)
  └── src/convert.rs
        ├── MessageConverter (trait)
        ├── convert_messages (generic fn)
        └── extract_tool_schemas (fn)

swink-agent-adapters
  ├── src/convert.rs        ──► re-exports core::convert::*
  ├── src/classify.rs       ──► HttpErrorKind, classify_http_status, classify_with_overrides
  ├── src/sse.rs            ──► SseLine, SseStreamParser, sse_data_lines
  ├── src/remote_presets.rs ──► RemotePresetKey, remote_preset_keys, build_remote_connection
  └── src/lib.rs            ──► re-exports all public types

swink-agent (core)
  └── src/stream.rs
        ├── StreamOptions.cache_strategy: CacheStrategy
        └── StreamOptions.on_raw_payload: Option<OnRawPayload>

CacheStrategy ──► flows via StreamOptions ──► adapter apply_cache_strategy()
OnRawPayload  ──► called in sse_data_lines() or adapter stream loop before event parsing
ProxyStreamFn ──► uses AdapterBase for HTTP ──► returns raw Bytes stream
```
