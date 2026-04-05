# Implementation Plan: Web Browse Plugin

**Branch**: `042-web-browse-plugin` | **Date**: 2026-04-04 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/042-web-browse-plugin/spec.md`

## Summary

A new workspace crate (`swink-agent-plugin-web`) at `plugins/web/` that implements the `Plugin` trait to provide web browsing capabilities: page fetching with readability extraction, multi-provider search (DuckDuckGo/Brave/Tavily), Playwright-based screenshots and structured extraction, domain filtering (SSRF protection), rate limiting, and content sanitization. The plugin contributes 4 tools (`web.fetch`, `web.search`, `web.screenshot`, `web.extract`), 2 PreDispatch policies (domain filter, rate limiter), and 1 PostTurn policy (content sanitizer).

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core — Plugin, AgentTool, policy traits, ContentBlock, AgentEvent), `reqwest` 0.13 (HTTP + redirects), `readability` 0.3 (content extraction), `scraper` 0.23 (HTML parsing / CSS selectors for DuckDuckGo Lite endpoint), `serde`/`serde_json` (serialization), `tokio` (async runtime, subprocess management), `base64` (screenshot encoding), `url` (URL parsing/validation), `regex` (injection pattern matching), `tracing` (diagnostics)
**Storage**: N/A (in-memory state only — rate limiter counter, Playwright subprocess handle)
**Testing**: `cargo test -p swink-agent-plugin-web` + integration tests with mock HTTP server (`wiremock` or similar)
**Target Platform**: Any platform supporting Rust + tokio; Playwright features require Node.js on host
**Project Type**: Library (workspace crate, plugin for swink-agent)
**Performance Goals**: `web.fetch` < 5s for typical pages; `web.screenshot` < 15s; search < 3s
**Constraints**: No unsafe code; Playwright is external (Node.js subprocess); DuckDuckGo scraping is best-effort
**Scale/Scope**: Single plugin crate, ~10-15 source files, 4 tools + 3 policies + 1 event observer

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | **PASS** | New workspace crate depends only on `swink-agent` public API. Self-contained, independently compilable and testable. |
| II. Test-Driven | **PASS** | Tests before implementation. Mock HTTP server for fetch/search tests. Playwright tests gated behind integration feature. |
| III. Efficiency & Performance | **PASS** | `reqwest` for HTTP (connection pooling). Long-lived Playwright subprocess (lazy start). Shared rate limiter via `Arc<Mutex<>>`. |
| IV. Leverage the Ecosystem | **PASS** | Uses `readability` (Mozilla port), `scraper` (Servo-backed), `reqwest`, `base64` — all well-maintained, high-download crates. |
| V. Provider Agnosticism | **PASS** | `SearchProvider` trait abstracts search backends. Plugin system already namespaces tools. Not an LLM concern. |
| VI. Safety & Correctness | **PASS** | `#[forbid(unsafe_code)]`. SSRF protection by default. Domain filtering before any HTTP. Content sanitization post-turn. |
| Crate count (11 → 12) | **JUSTIFIED** | Web browsing has unique deps (readability, scraper, base64, Playwright subprocess mgmt) that don't belong in any existing crate. Policies crate handles generic policies, not domain-specific tools. |

## Project Structure

### Documentation (this feature)

```text
specs/042-web-browse-plugin/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
plugins/
└── web/
    ├── Cargo.toml                # swink-agent-plugin-web
    ├── src/
    │   ├── lib.rs                # Public API re-exports, #[forbid(unsafe_code)]
    │   ├── plugin.rs             # WebPlugin struct implementing Plugin trait
    │   ├── config.rs             # WebPluginConfig builder
    │   ├── tools/
    │   │   ├── mod.rs            # Tool module re-exports
    │   │   ├── fetch.rs          # web.fetch tool (reqwest + readability)
    │   │   ├── search.rs         # web.search tool (delegates to SearchProvider)
    │   │   ├── screenshot.rs     # web.screenshot tool (Playwright subprocess)
    │   │   └── extract.rs        # web.extract tool (Playwright subprocess)
    │   ├── search/
    │   │   ├── mod.rs            # SearchProvider trait + re-exports
    │   │   ├── duckduckgo.rs     # DuckDuckGo Lite HTML scraping
    │   │   ├── brave.rs          # Brave Search API (feature-gated)
    │   │   └── tavily.rs         # Tavily API (feature-gated)
    │   ├── policy/
    │   │   ├── mod.rs            # Policy module re-exports
    │   │   ├── domain_filter.rs  # PreDispatch: SSRF + allow/deny lists
    │   │   ├── rate_limiter.rs   # PreDispatch: shared rate limiting
    │   │   └── sanitizer.rs      # PostTurn: prompt injection stripping
    │   ├── playwright.rs         # Long-lived Playwright subprocess manager
    │   ├── content.rs            # HTML readability extraction + truncation
    │   └── domain.rs             # URL parsing, IP classification, domain matching
    └── tests/
        ├── common/
        │   └── mod.rs            # Shared test helpers
        ├── fetch_test.rs
        ├── search_test.rs
        ├── domain_filter_test.rs
        ├── rate_limiter_test.rs
        ├── sanitizer_test.rs
        └── playwright_test.rs    # Integration tests (require Playwright)
```

**Structure Decision**: New `plugins/web/` directory at workspace root. This is the first plugin crate, establishing the `plugins/` convention for future domain-specific plugin crates. Added as workspace member `"plugins/web"` in root `Cargo.toml`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 12th workspace crate | Web browsing has unique deps (readability, scraper, Playwright subprocess) and is a domain-specific plugin, not a core concern | Adding to `policies` crate would pollute it with HTTP/HTML deps; adding to core would violate Library-First |
