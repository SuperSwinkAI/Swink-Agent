# Quickstart: Web Browse Plugin

**Feature**: 042-web-browse-plugin | **Date**: 2026-04-04

## Prerequisites

- Rust 1.88+ (edition 2024)
- Node.js 18+ and Playwright CLI (only for screenshot/extract features):
  ```bash
  npm install -g playwright
  npx playwright install chromium
  ```

## Add Dependency

```toml
# In your Cargo.toml
[dependencies]
swink-agent = { path = "../" }           # or version from crates.io
swink-agent-plugin-web = { path = "../plugins/web" }
```

Feature flags for search providers:
```toml
# Default includes DuckDuckGo only
swink-agent-plugin-web = { path = "../plugins/web" }

# Add Brave Search support
swink-agent-plugin-web = { path = "../plugins/web", features = ["brave"] }

# Add all providers
swink-agent-plugin-web = { path = "../plugins/web", features = ["all"] }
```

## Minimal Usage (Zero Config)

```rust
use std::sync::Arc;
use swink_agent::Agent;
use swink_agent_plugin_web::WebPlugin;

let agent = Agent::builder()
    .plugin(Arc::new(WebPlugin::new()))
    // ... stream_fn, model, etc.
    .build();

// Agent now has tools: web.fetch, web.search, web.screenshot, web.extract
// - DuckDuckGo search (no API key needed)
// - SSRF protection enabled by default
// - Rate limit: 30 req/min
// - Content truncation at 50,000 chars
```

## Configured Usage

```rust
use std::sync::Arc;
use std::time::Duration;
use swink_agent::Agent;
use swink_agent_plugin_web::{WebPlugin, SearchProviderKind};

let plugin = WebPlugin::builder()
    .search_provider(SearchProviderKind::Brave)
    .brave_api_key("BSA-...")
    .rate_limit_rpm(60)
    .max_content_length(100_000)
    .domain_denylist(vec!["internal.corp.com".into()])
    .viewport(1920, 1080)
    .build();

let agent = Agent::builder()
    .plugin(Arc::new(plugin))
    .build();
```

## Build & Test

```bash
# Build the plugin crate
cargo build -p swink-agent-plugin-web

# Run unit tests (no external dependencies needed)
cargo test -p swink-agent-plugin-web

# Run with all search providers
cargo test -p swink-agent-plugin-web --features all

# Run integration tests (requires Playwright)
cargo test -p swink-agent-plugin-web --features integration
```

## What the Agent Can Do

Once the plugin is registered, the agent has access to:

| Tool | Description | Requires Playwright? |
|------|-------------|---------------------|
| `web.fetch` | Fetch a URL, return clean readable text | No |
| `web.search` | Search the web, return ranked results | No |
| `web.screenshot` | Capture a PNG screenshot of a page | Yes |
| `web.extract` | Extract structured content via CSS selectors | Yes |

Policies are automatically active:
- **Domain filter** (PreDispatch): Blocks private IPs and denied domains
- **Rate limiter** (PreDispatch): Limits requests per minute
- **Content sanitizer** (PostTurn): Strips prompt injection patterns
