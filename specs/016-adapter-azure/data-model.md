# Data Model: 016-adapter-azure

**Date**: 2026-04-02

## Entities

### AzureAuth (new)

Authentication credential for Azure OpenAI.

| Field | Type | Notes |
|---|---|---|
| (variant) | `ApiKey(String)` | API key passed via `api-key` header |
| (variant) | `EntraId { tenant_id, client_id, client_secret }` | All `String`. OAuth2 client credentials flow |

**Relationships**: Owned by `AzureStreamFn`. Determines auth header strategy.

### AzureStreamFn (extended)

Streaming function connecting to Azure OpenAI deployments.

| Field | Type | Notes |
|---|---|---|
| `base` | `AdapterBase` | Contains `base_url`, `api_key` (unused for EntraId), `client` |
| `auth` | `AzureAuth` | Authentication method |
| `token_cache` | `Arc<RwLock<Option<CachedToken>>>` | Only used for EntraId variant |

**Lifecycle**: Constructed once, used for many stream calls. Token cache is shared across concurrent streams.

### CachedToken (new, internal)

Cached OAuth2 bearer token.

| Field | Type | Notes |
|---|---|---|
| `access_token` | `String` | Bearer token value |
| `expires_at` | `Instant` | When the token expires |

**Validation rules**: Token is considered expired when `Instant::now() >= expires_at - REFRESH_MARGIN` (e.g., 5 min before actual expiry).

### StreamErrorKind (extended, core crate)

New variant added to existing enum.

| Variant | Description |
|---|---|
| `ContentFiltered` | Provider's content safety filter blocked the response |

### AgentError (extended, core crate)

New variant added to existing enum.

| Variant | Fields | Retryable |
|---|---|---|
| `ContentFiltered` | (none) | No |

## State Transitions

### Token Cache (EntraId only)

```
Empty → Acquiring → Cached → Refreshing → Cached
                              ↓
                          Expired → Acquiring → Cached
```

- **Empty**: No token yet (first request)
- **Acquiring**: POST to token endpoint in progress
- **Cached**: Valid token available
- **Refreshing**: Proactive refresh before expiry
- **Expired**: Token past expiry, must re-acquire

Concurrent requests during Acquiring/Refreshing wait on the same future (no duplicate token requests).
