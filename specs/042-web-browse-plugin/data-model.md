# Data Model: Web Browse Plugin

**Feature**: 042-web-browse-plugin | **Date**: 2026-04-04

## Entities

### WebPlugin

The top-level struct implementing `Plugin`. Owns all configuration and shared state.

| Field | Type | Description |
|-------|------|-------------|
| config | `WebPluginConfig` | Immutable configuration |
| http_client | `reqwest::Client` | Shared HTTP client (connection pooling, redirect policy) |
| playwright | `Arc<Mutex<Option<PlaywrightBridge>>>` | Lazily-initialized subprocess handle |
| rate_state | `Arc<Mutex<VecDeque<Instant>>>` | Shared rate limiter timestamps |
| search_provider | `Arc<dyn SearchProvider>` | Selected search backend |

### WebPluginConfig

Builder-pattern configuration. All fields have sensible defaults.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| search_provider_kind | `SearchProviderKind` | `DuckDuckGo` | Which search backend to use |
| brave_api_key | `Option<String>` | `None` | API key for Brave Search |
| tavily_api_key | `Option<String>` | `None` | API key for Tavily |
| domain_allowlist | `Vec<String>` | `[]` (all allowed) | If non-empty, only these domains permitted |
| domain_denylist | `Vec<String>` | `[]` | Additional domains to block |
| block_private_ips | `bool` | `true` | SSRF protection for RFC1918/loopback |
| rate_limit_rpm | `u32` | `30` | Max requests per minute across all tools |
| max_content_length | `usize` | `50_000` | Max chars before truncation |
| max_redirects | `u32` | `10` | HTTP redirect follow limit |
| max_search_results | `usize` | `10` | Max results from web.search |
| user_agent | `String` | `"SwinkAgent/0.5"` | User-Agent header for HTTP requests |
| playwright_path | `Option<PathBuf>` | `None` (auto-detect) | Path to `npx playwright` or `playwright` binary |
| screenshot_timeout | `Duration` | `15s` | Playwright operation timeout |
| request_timeout | `Duration` | `30s` | HTTP request timeout |
| viewport_width | `u32` | `1280` | Default screenshot viewport width |
| viewport_height | `u32` | `720` | Default screenshot viewport height |
| sanitizer_enabled | `bool` | `true` | Enable prompt injection sanitization |

### SearchProviderKind (enum)

```rust
pub enum SearchProviderKind {
    DuckDuckGo,
    Brave,
    Tavily,
}
```

### SearchProvider (trait)

```rust
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, query: &str, max_results: usize) -> impl Future<Output = Result<Vec<SearchResult>, SearchError>> + Send;
}
```

### SearchResult

| Field | Type | Description |
|-------|------|-------------|
| title | `String` | Page title |
| url | `String` | Result URL |
| snippet | `String` | Text snippet / description |

### SearchError (enum)

```rust
pub enum SearchError {
    NetworkError(String),
    ParseError(String),
    RateLimited,
    ApiKeyMissing,
    ProviderUnavailable(String),
}
```

### FetchedContent

| Field | Type | Description |
|-------|------|-------------|
| url | `String` | Final URL (after redirects) |
| title | `Option<String>` | Extracted page title |
| text | `String` | Cleaned readable text |
| content_type | `String` | Original Content-Type header |
| content_length | `usize` | Original content size in bytes |
| truncated | `bool` | Whether content was truncated |
| status_code | `u16` | HTTP status code |

### DomainFilter

| Field | Type | Description |
|-------|------|-------------|
| allowlist | `Vec<String>` | If non-empty, only these domains allowed |
| denylist | `Vec<String>` | Explicitly blocked domains |
| block_private_ips | `bool` | Block RFC1918, loopback, link-local |

**Validation rules**:
- URL must be `http://` or `https://` scheme (reject `file://`, `ftp://`, etc.)
- If allowlist is non-empty, domain must appear in allowlist
- Domain must not appear in denylist
- If `block_private_ips`, resolved IP must not be in private ranges
- DNS resolution checked against private ranges (prevents DNS rebinding)

### PlaywrightBridge

| Field | Type | Description |
|-------|------|-------------|
| child | `tokio::process::Child` | Node.js subprocess handle |
| stdin | `ChildStdin` | JSON request writer |
| stdout | `BufReader<ChildStdout>` | JSON response reader |

**Lifecycle**: Created lazily on first `web.screenshot` or `web.extract` call. Shut down (SIGTERM → SIGKILL after timeout) when `WebPlugin` is dropped.

### PlaywrightRequest (enum, serialized as JSON)

```rust
pub enum PlaywrightRequest {
    Screenshot { url: String, viewport: Option<Viewport> },
    Extract { url: String, selector: Option<String>, preset: Option<ExtractionPreset> },
    Ping,
    Shutdown,
}
```

### PlaywrightResponse

```rust
pub struct PlaywrightResponse {
    pub ok: bool,
    pub data: Option<serde_json::Value>,  // base64 string for screenshots, array for extract
    pub error: Option<String>,
}
```

### ExtractionPreset (enum)

```rust
pub enum ExtractionPreset {
    Links,
    Headings,
    Tables,
}
```

### ExtractedElement

| Field | Type | Description |
|-------|------|-------------|
| tag | `String` | HTML tag name |
| text | `String` | Text content |
| attributes | `HashMap<String, String>` | Relevant attributes (href, src, etc.) |

### Viewport

| Field | Type | Description |
|-------|------|-------------|
| width | `u32` | Viewport width in pixels |
| height | `u32` | Viewport height in pixels |

## Relationships

```
WebPlugin
├── owns WebPluginConfig (immutable after construction)
├── owns reqwest::Client (shared, connection-pooled)
├── owns Arc<dyn SearchProvider> (one of DuckDuckGo/Brave/Tavily)
├── owns Arc<Mutex<Option<PlaywrightBridge>>> (lazy singleton)
├── owns Arc<Mutex<VecDeque<Instant>>> (rate limiter state)
├── contributes tools:
│   ├── FetchTool → uses reqwest::Client, readability, DomainFilter
│   ├── SearchTool → uses SearchProvider
│   ├── ScreenshotTool → uses PlaywrightBridge, DomainFilter
│   └── ExtractTool → uses PlaywrightBridge, DomainFilter
├── contributes PreDispatch policies:
│   ├── DomainFilterPolicy → reads DomainFilter config, resolves DNS
│   └── RateLimitPolicy → reads/writes rate_state
└── contributes PostTurn policy:
    └── ContentSanitizerPolicy → regex scan of tool results
```

## Feature Gates

| Feature | Gates | Dependencies Added |
|---------|-------|--------------------|
| `default` | `["duckduckgo"]` | (base deps always compiled) |
| `duckduckgo` | `DuckDuckGoProvider` | `scraper` (HTML parsing) |
| `brave` | `BraveProvider` | (uses `reqwest` already in base) |
| `tavily` | `TavilyProvider` | (uses `reqwest` already in base) |
| `all` | All providers | `scraper` |
