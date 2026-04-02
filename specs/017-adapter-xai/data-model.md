# Data Model: Adapter xAI

**Feature**: 017-adapter-xai | **Date**: 2026-04-02

## Entities

### XAiStreamFn

The sole public type. A thin wrapper that delegates to `OpenAiStreamFn` for all transport logic.

| Field | Type | Description |
|-------|------|-------------|
| `inner` | `OpenAiStreamFn` | Wrapped OpenAI-compatible stream function |

**Constructor**: `XAiStreamFn::new(base_url, api_key)` — creates inner `OpenAiStreamFn` with provided credentials.

**Trait implementations**:
- `StreamFn` — delegates to `inner.stream()`
- `Debug` — shows struct name with redacted credentials
- `Send + Sync` — enforced via compile-time assertion

### Model Catalog Presets (TOML, not Rust types)

Provider entry in `src/model_catalog.toml`:

| Field | Value |
|-------|-------|
| `key` | `"xai"` |
| `display_name` | `"xAI"` |
| `kind` | `"remote"` |
| `auth_mode` | `"bearer"` |
| `credential_env_var` | `"XAI_API_KEY"` |
| `base_url_env_var` | `"XAI_BASE_URL"` |
| `default_base_url` | `"https://api.x.ai"` |

Preset entries (5 models):

| Preset ID | Model ID | Context | Max Output | Cost In/Out |
|-----------|----------|---------|------------|-------------|
| `grok_4_20_reasoning` | `grok-4.20-0309-reasoning` | 2M | 16384 | $2.00/$6.00 |
| `grok_4_20_non_reasoning` | `grok-4.20-0309-non-reasoning` | 2M | 16384 | $2.00/$6.00 |
| `grok_4_1_fast_reasoning` | `grok-4-1-fast-reasoning` | 2M | 16384 | $0.20/$0.50 |
| `grok_4_1_fast_non_reasoning` | `grok-4-1-fast-non-reasoning` | 2M | 16384 | $0.20/$0.50 |
| `grok_4_20_multi_agent` | `grok-4.20-multi-agent-0309` | 2M | 16384 | $2.00/$6.00 |

All presets share: `capabilities = ["text", "tools", "images_in", "streaming", "structured_output"]`, `status = "ga"`, `api_version = "v1"`.

## Relationships

```
XAiStreamFn --wraps--> OpenAiStreamFn --uses--> AdapterBase
                                       --uses--> OaiConverter (message conversion)
                                       --uses--> OaiChatRequest (request body)
                                       --uses--> parse_oai_sse_stream() (response parsing)
```

## State Transitions

None — `XAiStreamFn` is stateless. Each `stream()` call is independent.

## Validation Rules

- `base_url` must not be empty (enforced by `AdapterBase`)
- `api_key` must not be empty (enforced by `AdapterBase`)
- Model ID is passed through to the API (xAI validates server-side)
