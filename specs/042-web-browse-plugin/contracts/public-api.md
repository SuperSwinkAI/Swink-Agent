# Public API Contract: swink-agent-plugin-web

**Feature**: 042-web-browse-plugin | **Date**: 2026-04-04

## Crate Re-exports (`lib.rs`)

```rust
// Top-level public API — consumers never reach into submodules
pub use config::{WebPluginConfig, WebPluginConfigBuilder, SearchProviderKind};
pub use plugin::WebPlugin;
pub use search::{SearchProvider, SearchResult, SearchError};
pub use content::FetchedContent;
pub use domain::DomainFilter;
pub use playwright::{ExtractionPreset, ExtractedElement, Viewport};
```

## Plugin Construction (Builder Pattern)

```rust
use swink_agent_plugin_web::{WebPlugin, WebPluginConfig, SearchProviderKind};

// Minimal (zero-config, DuckDuckGo search, all defaults)
let plugin = WebPlugin::new();

// Full configuration
let plugin = WebPlugin::builder()
    .search_provider(SearchProviderKind::Brave)
    .brave_api_key("sk-...")
    .domain_denylist(vec!["evil.com".into()])
    .domain_allowlist(vec!["docs.rs".into(), "crates.io".into()])
    .block_private_ips(true)          // default: true
    .rate_limit_rpm(60)               // default: 30
    .max_content_length(100_000)      // default: 50_000
    .max_search_results(5)            // default: 10
    .max_redirects(5)                 // default: 10
    .playwright_path("/usr/local/bin/npx")
    .screenshot_timeout(Duration::from_secs(20))  // default: 15s
    .request_timeout(Duration::from_secs(10))     // default: 30s
    .viewport(1920, 1080)             // default: 1280x720
    .sanitizer_enabled(true)          // default: true
    .build();
```

## Plugin Registration

```rust
use swink_agent::Agent;

let agent = Agent::builder()
    .plugin(Arc::new(plugin))
    // ... other config
    .build();

// Tools are auto-namespaced as: web.fetch, web.search, web.screenshot, web.extract
// Policies are auto-contributed: DomainFilterPolicy, RateLimitPolicy, ContentSanitizerPolicy
```

## Tool Schemas

### web.fetch

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "The URL to fetch. Must be http:// or https://."
    }
  },
  "required": ["url"]
}
```

**Returns**: `AgentToolResult` with `ContentBlock::Text` containing the readable content. Error result if domain blocked, non-200, non-HTML content type, or network failure.

### web.search

```json
{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "The search query string."
    },
    "max_results": {
      "type": "integer",
      "description": "Maximum number of results to return. Defaults to 10.",
      "minimum": 1,
      "maximum": 50
    }
  },
  "required": ["query"]
}
```

**Returns**: `AgentToolResult` with `ContentBlock::Text` containing formatted search results (title, URL, snippet per result). Error result if provider unavailable or API key missing.

### web.screenshot

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "The URL to screenshot. Must be http:// or https://."
    },
    "width": {
      "type": "integer",
      "description": "Viewport width in pixels. Defaults to 1280.",
      "minimum": 320,
      "maximum": 3840
    },
    "height": {
      "type": "integer",
      "description": "Viewport height in pixels. Defaults to 720.",
      "minimum": 240,
      "maximum": 2160
    }
  },
  "required": ["url"]
}
```

**Returns**: `AgentToolResult` with `ContentBlock::Image { source: ImageSource::Base64 { media_type: "image/png", data } }`. Error result if Playwright not installed, timeout, or domain blocked.

### web.extract

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "The URL to extract content from. Must be http:// or https://."
    },
    "selector": {
      "type": "string",
      "description": "CSS selector to match elements. Mutually exclusive with 'preset'."
    },
    "preset": {
      "type": "string",
      "enum": ["links", "headings", "tables"],
      "description": "Preset extraction type. Mutually exclusive with 'selector'."
    }
  },
  "required": ["url"]
}
```

**Returns**: `AgentToolResult` with `ContentBlock::Text` containing structured extraction results (JSON array of elements). Error result if Playwright not installed, no matches, timeout, or domain blocked. If neither `selector` nor `preset` provided, defaults to extracting all text content.

## SearchProvider Trait (Extension Point)

```rust
#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError>;
}
```

Consumers can implement custom search providers and pass them via `WebPlugin::builder().custom_search_provider(Arc::new(my_provider))`.

## Playwright Bridge Protocol

JSON-newline protocol over stdin/stdout of a long-lived Node.js subprocess.

**Request format** (one JSON object per line, Rust → Node.js):
```json
{"id":1,"action":"screenshot","url":"https://example.com","viewport":{"width":1280,"height":720}}
{"id":2,"action":"extract","url":"https://example.com","selector":"h1","preset":null}
{"id":3,"action":"ping"}
{"id":4,"action":"shutdown"}
```

**Response format** (one JSON object per line, Node.js → Rust):
```json
{"id":1,"ok":true,"data":"iVBORw0KGgo..."}
{"id":2,"ok":true,"data":[{"tag":"h1","text":"Example","attributes":{}}]}
{"id":3,"ok":true}
{"id":4,"ok":true}
```

Request `id` field enables concurrent requests over the single subprocess. Responses are matched by `id`.
