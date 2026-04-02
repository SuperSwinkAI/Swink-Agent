# Research: 016-adapter-azure

**Date**: 2026-04-02

## 1. Azure OpenAI v1 GA API

**Decision**: Target v1 GA API (August 2025+) — `api-version` query parameter no longer required.

**Rationale**: v1 GA is the forward-looking API generation with faster feature cadence. The project targets 2026+ usage. Legacy versioned API (last GA: `2024-10-21`) is not supported.

**Alternatives considered**: Legacy versioned API (broadest compat but deprecated path), auto-detect (complexity not justified).

**URL format**: `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions`
- No `api-version` query param needed on v1 GA
- Deployment name is in the URL path (not in the request body like standard OpenAI)
- The `model` field in the request body is still sent but Azure uses the deployment's configured model

## 2. Existing Adapter Baseline

**Decision**: Extend the existing `azure.rs` (currently a stub with basic API key auth).

**Rationale**: The file already follows all project patterns — `AdapterBase`, `openai_compat` delegation, `classify` error handling, `Send+Sync` assertion. Building on it avoids rework.

**Current state** (137 lines):
- `AzureStreamFn { base: AdapterBase }` — API key only
- URL: `{base_url}/chat/completions` — correct pattern
- Auth: `api-key` header — correct
- SSE: delegates to `parse_oai_sse_stream` — correct
- Errors: delegates to `error_event_from_status` — needs extension for content filter
- Missing: Azure AD auth, content filter detection, `ContentFiltered` error variant

## 3. Azure AD / Entra ID OAuth2

**Decision**: Full client credentials flow built into the adapter. Reuse same `reqwest::Client`.

**Rationale**: User requirement — don't defer to spec 035. The adapter owns its auth lifecycle. Client credentials flow is the standard for service-to-service Azure auth.

**Token endpoint**: `POST https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token`

**Parameters** (form-urlencoded):
| Param | Value |
|---|---|
| `grant_type` | `client_credentials` |
| `client_id` | `{app-client-id}` |
| `client_secret` | `{client-secret}` |
| `scope` | `https://cognitiveservices.azure.com/.default` |

**Response**: `{ "access_token": "...", "token_type": "Bearer", "expires_in": 3600 }`

**Token caching strategy**: `Arc<RwLock<Option<CachedToken>>>` with proactive refresh (e.g., refresh when < 5 min to expiry). Non-blocking for concurrent reads — only the refresh writer blocks briefly.

**Alternatives considered**: Separate reqwest::Client for auth (rejected — unnecessary allocation), delegate to spec 035 (rejected — user wants adapter-owned auth).

## 4. Authentication Enum Design

**Decision**: `AzureAuth` enum with two variants.

**Rationale**: Clean discrimination between API key and Azure AD at the type level. The adapter's `stream` method checks which variant and sets headers accordingly.

```rust
pub enum AzureAuth {
    ApiKey(String),
    EntraId {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
}
```

For Entra ID, tokens are cached internally. The `AzureStreamFn` struct holds `Arc<TokenCache>` when Entra ID is configured.

## 5. ContentFiltered Error — Cross-Cutting Change

**Decision**: Add `ContentFiltered` to both `StreamErrorKind` (stream.rs) and `AgentError` (error.rs) in the core crate.

**Rationale**: Content filtering is not Azure-specific — Anthropic, OpenAI, and Gemini all have safety filters. A core variant enables uniform handling across all adapters and lets policies/retry logic branch on it.

**Changes required**:
1. `StreamErrorKind::ContentFiltered` variant in `src/stream.rs`
2. `AssistantMessageEvent::error_content_filtered(message)` constructor
3. `AgentError::ContentFiltered` variant in `src/error.rs` (non-retryable)
4. Loop handling in `src/loop_.rs` — map `StreamErrorKind::ContentFiltered` → `AgentError::ContentFiltered`

**Azure-specific detection**:
- `finish_reason: "content_filter"` in SSE chunks
- `content_filter_results` object with `filtered: true` in any category
- HTTP error with `error.code: "ContentFilterBlocked"`

## 6. openai_compat Reuse Strategy

**Decision**: Maximum reuse — only customize URL, auth, and content filter post-processing.

**Rationale**: Azure v1 GA SSE format is identical to OpenAI. The only differences are URL routing, auth headers, and the presence of `content_filter_results` in chunks. The existing `parse_oai_sse_stream` already handles the SSE parsing, tool call accumulation, and finalization.

**Content filter hook**: The `openai_compat` module's `process_oai_chunk` currently ignores `content_filter_results`. Two options:
1. Add a post-processing step in the Azure stream that checks for `finish_reason: "content_filter"` after `parse_oai_sse_stream` emits Done
2. Add an optional callback/flag to `parse_oai_sse_stream` for content filter awareness

Option 1 is simpler and avoids changing shared code. The Azure adapter wraps the parsed stream with a filter that converts `Done` events preceded by content filter signals into `ContentFiltered` errors.

## 7. Feature Gate

**Decision**: `azure` feature flag already exists in `adapters/Cargo.toml` as a stub. No new dependencies needed beyond what's already in the workspace.

**Rationale**: Azure AD token acquisition uses `reqwest` (already a dependency) for the POST to the token endpoint. No additional crates needed.

## 8. Error Response Format

**Decision**: Extend error classification to detect Azure-specific error codes in response bodies.

**Azure error shape**:
```json
{
  "error": {
    "code": "ContentFilterBlocked",
    "message": "...",
    "inner_error": {
      "code": "ResponsibleAIPolicyViolation",
      "content_filter_results": { ... }
    }
  }
}
```

**Detection**: Parse JSON body on non-2xx responses. If `error.code == "ContentFilterBlocked"`, emit `ContentFiltered` error instead of the default HTTP status classification.
