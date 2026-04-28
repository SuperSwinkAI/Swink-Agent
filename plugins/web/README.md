# swink-agent-plugin-web

[![Crates.io](https://img.shields.io/crates/v/swink-agent-plugin-web.svg)](https://crates.io/crates/swink-agent-plugin-web)
[![Docs.rs](https://docs.rs/swink-agent-plugin-web/badge.svg)](https://docs.rs/swink-agent-plugin-web)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Web browsing plugin for [`swink-agent`](https://crates.io/crates/swink-agent) — fetch pages, search, screenshot, and extract structured content behind a domain filter and rate limiter.

## Features

- **Fetch** — HTTP GET with configurable timeouts and content-type sniffing (`FetchedContent`)
- **Search** — pluggable `SearchProvider` with feature-gated backends:
  - `duckduckgo` (default, no API key)
  - `brave` (requires `BRAVE_SEARCH_API_KEY`)
  - `tavily` (requires `TAVILY_API_KEY`)
- **Screenshot** — Playwright-driven full-page or viewport captures (`Viewport`, PNG/JPEG)
- **Extract** — CSS-selector-based structured extraction with `ExtractionPreset` shortcuts for common patterns (article body, product listing, etc.)
- **`DomainFilter`** — allow/deny lists with wildcard matching; rejected fetches never leave the process
- **Rate limiter** — per-domain requests-per-minute cap with shared state across tools
- **Content sanitizer** — strips scripts, iframes, and dangerous attributes before returning content to the model
- Integrates via `swink-agent`'s `plugins` feature — register once, expose all web tools to the agent

## Quick Start

```toml
[dependencies]
swink-agent = { version = "0.9.0", features = ["plugins"] }
swink-agent-plugin-web = { version = "0.9.0", features = ["duckduckgo"] }
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use std::sync::Arc;
use swink_agent::prelude::*;
use swink_agent_plugin_web::WebPlugin;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = WebPlugin::builder()
        .with_domain_allowlist(vec!["docs.rs".into(), "wikipedia.org".into()])
        .with_rate_limit_rpm(60)
        .build();
    let plugin: Arc<dyn swink_agent::Plugin> = Arc::new(WebPlugin::from_config(config)?);

    let options = AgentOptions::from_connections("You can browse the web.", connections)
        .with_plugin(plugin);

    let mut agent = Agent::new(options);
    let result = agent.prompt_text("Summarize the Rust 1.95 release notes.").await?;
    println!("{}", result.assistant_text());
    Ok(())
}
```

## Architecture

Each capability (fetch, search, screenshot, extract) is an independent `AgentTool` that shares a single `reqwest::Client` and rate-limiter state through the `WebPlugin`. Domain filtering runs before every request; the tools have no IO path that bypasses it. Search providers are trait objects (`Arc<dyn SearchProvider>`) selected at plugin build time — feature flags compile out the ones you don't use.

No `unsafe` code (`#![forbid(unsafe_code)]`). Screenshots launch an external Playwright process — disable the feature if your threat model disallows child processes.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
