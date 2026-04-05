# Research: Web Browse Plugin

**Feature**: 042-web-browse-plugin | **Date**: 2026-04-04

## R-001: HTML Readability Extraction

**Decision**: Use the `readability` crate (v0.3.0, ~477K downloads)

**Rationale**: Direct port of arc90's readability algorithm — the same family of algorithms powering Firefox Reader View and Safari Reader. Takes a `Read` impl + URL, returns `Product { title, content, text }`. The `text` field provides plain text; `content` provides cleaned HTML. We'll use `text` for the tool result and optionally `title` as metadata.

**Alternatives considered**:
- `readability-rust` (~26K downloads): Mozilla Readability.js port. Takes `&str` directly. Lower adoption, less battle-tested.
- Custom heuristics with `scraper`: Maximum flexibility but significant implementation effort for a solved problem. Violates Constitution IV (Leverage the Ecosystem).

**API surface**:
```rust
use readability::extractor;
let product = extractor::extract(&mut html_bytes.as_slice(), &url)?;
// product.title: String, product.content: String (cleaned HTML), product.text: String (plain text)
```

## R-002: HTML Parsing for DuckDuckGo Scraping

**Decision**: Use the `scraper` crate (v0.23.1)

**Rationale**: Built on Servo's `html5ever` parser and `selectors` engine. Industry-standard Rust HTML parsing with CSS selector support. Also useful as a fallback for `web.extract` if Playwright is unavailable for any reason.

**API surface**:
```rust
use scraper::{Html, Selector};
let doc = Html::parse_document(&html);
let sel = Selector::parse("a.result-link").unwrap();
for el in doc.select(&sel) {
    let href = el.value().attr("href");
    let text = el.text().collect::<String>();
}
```

## R-003: DuckDuckGo Lite Endpoint

**Decision**: Scrape `https://lite.duckduckgo.com/lite/` via POST with `q=<query>` form data

**Rationale**: Same approach used by LangChain (`duckduckgo-search`), AutoGen, and other major AI frameworks. The Lite endpoint returns simple HTML without JavaScript, making it reliable for scraping. No API key required.

**Alternatives considered**:
- DuckDuckGo Instant Answer API: Limited to instant answers, not full web search results. Deprecated/unreliable.
- DuckDuckGo main site: Requires JavaScript rendering.

**Risks**: DuckDuckGo could change the Lite HTML structure or add rate limiting. Mitigation: keep selectors isolated in `duckduckgo.rs` for easy updates.

## R-004: Brave Search API

**Decision**: REST API at `https://api.search.brave.com/res/v1/web/search` with API key in `X-Subscription-Token` header

**Rationale**: Well-documented JSON API. Returns structured results with title, URL, description. Free tier available (1 query/sec, 2000/month).

**Feature gate**: `brave` feature flag on the crate.

## R-005: Tavily Search API

**Decision**: REST API at `https://api.tavily.com/search` with API key in request body

**Rationale**: Purpose-built for AI agents. Returns structured results optimized for LLM consumption. Includes relevance scoring.

**Feature gate**: `tavily` feature flag on the crate.

## R-006: Playwright Subprocess Communication

**Decision**: Bundle a small Node.js script (`playwright-bridge.js`) with the crate. Launch as a long-lived subprocess via `tokio::process::Command`. Communicate via JSON-newline protocol over stdin/stdout.

**Rationale**: Playwright's full API is available via Node.js. A JSON protocol over stdio is simple, debuggable, and avoids the complexity of CDP or WebSocket. The subprocess is lazily started on first `web.screenshot` or `web.extract` call and shut down on plugin drop.

**Protocol**:
```json
// Request (Rust → Node.js)
{"action":"screenshot","url":"https://example.com","viewport":{"width":1280,"height":720}}
{"action":"extract","url":"https://example.com","selector":"h1","preset":null}

// Response (Node.js → Rust)
{"ok":true,"data":"<base64 png>"}
{"ok":true,"data":[{"tag":"h1","text":"Hello World","html":"<h1>Hello World</h1>"}]}
{"ok":false,"error":"Navigation timeout"}
```

**Alternatives considered**:
- CDP via `chromiumoxide`: Chrome-only, less mature Rust crate, would lose Playwright's multi-browser abstraction.
- Fresh subprocess per request: Simpler but ~1-2s startup overhead per call, unacceptable for multiple operations.

**Bridge script location**: Shipped as `include_str!` in the Rust binary or written to a temp file at first use. Avoids requiring users to manage an external JS file.

## R-007: SSRF Protection / Domain Filtering

**Decision**: Parse URL, resolve to IP, check against private ranges before any HTTP call

**Rationale**: Standard SSRF prevention. Must block: `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16` (link-local), `::1`, `fc00::/7` (ULA). Also block `0.0.0.0`, and DNS rebinding via checking resolved IP (not just hostname).

**Implementation**: Use `url::Url` for parsing, `std::net::ToSocketAddrs` or `tokio::net::lookup_host` for DNS resolution, then check the resolved IP against private ranges. This prevents DNS rebinding attacks where a hostname resolves to a private IP.

**Ordering**: Domain filter runs as PreDispatch policy → checks URL before tool execute() is called → zero HTTP if blocked.

## R-008: Rate Limiter Design

**Decision**: Sliding window counter using `Arc<Mutex<VecDeque<Instant>>>` shared across all tools

**Rationale**: Simple, accurate, and low overhead. Each request timestamp is pushed; timestamps older than the window are pruned. If count >= limit after pruning, reject. Interior mutability via `Arc<Mutex<>>` matches project conventions (see Constitution: "No global mutable state").

**Default**: 30 requests per minute (configurable via builder).

## R-009: Content Sanitization

**Decision**: Regex-based pattern matching for known prompt injection patterns, applied as PostTurn policy

**Rationale**: Defense-in-depth, not a guarantee. Targets common patterns: "ignore all previous instructions", "you are now", "system:", "IMPORTANT:", and variations. Uses compiled regex (same pattern as `ContentFilter` in policies crate). False-positive mitigation: patterns require contextual anchoring (e.g., start-of-line or after punctuation).

**Implementation**: PostTurnPolicy that scans tool results in the turn's messages for injection patterns. Verdict is `Inject` with sanitized content replacing the original, or `Continue` if clean.

## R-010: reqwest Redirect Configuration

**Decision**: Use `reqwest::redirect::Policy::limited(n)` with default of 10 redirects

**Rationale**: Confirmed in reqwest API — `Client::builder().redirect(Policy::limited(10))` is the default. Configurable via builder for users who need fewer/more.

```rust
let client = reqwest::Client::builder()
    .redirect(reqwest::redirect::Policy::limited(max_redirects))
    .timeout(std::time::Duration::from_secs(30))
    .build()?;
```
