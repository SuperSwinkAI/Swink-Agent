# Tasks: Web Browse Plugin

**Input**: Design documents from `/specs/042-web-browse-plugin/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — Constitution II mandates test-driven development.

**Organization**: Tasks grouped by user story. P1 stories (US1, US2, US5) form the MVP. P2 stories (US3, US4, US6, US7) add polish and safety.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Exact file paths included in descriptions

## Path Conventions

All source code under `plugins/web/src/`. Tests under `plugins/web/tests/`.

---

## Phase 1: Setup

**Purpose**: Create the crate, configure workspace, establish basic structure

- [x] T001 Create `plugins/web/` directory structure per plan.md: `src/`, `src/tools/`, `src/search/`, `src/policy/`, `tests/`, `tests/common/`
- [x] T002 Create `plugins/web/Cargo.toml` with crate name `swink-agent-plugin-web`, edition 2024, `#[forbid(unsafe_code)]`, workspace deps (`swink-agent`, `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`, `url`, `regex`, `base64`, `thiserror`), optional deps (`scraper` for `duckduckgo` feature, `readability`), dev-deps (`wiremock`, `tokio` with `test-util`), feature gates (`default = ["duckduckgo"]`, `duckduckgo`, `brave`, `tavily`, `all`, `integration`)
- [x] T003 Add `"plugins/web"` to workspace members in root `Cargo.toml`
- [x] T004 Create `plugins/web/src/lib.rs` with `#![forbid(unsafe_code)]`, module declarations, and public re-exports (initially empty stubs)
- [x] T005 [P] Create `plugins/web/tests/common/mod.rs` with shared test helpers (mock HTTP server setup using `wiremock` or inline `tokio::net::TcpListener`)
- [x] T006 Verify `cargo build -p swink-agent-plugin-web` compiles successfully

**Checkpoint**: Empty crate compiles in workspace

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared types, config builder, and domain utilities that all user stories depend on

**WARNING**: No user story work can begin until this phase is complete

- [x] T007 Implement `SearchResult` struct (title, url, snippet) with `Serialize`/`Deserialize` in `plugins/web/src/search/mod.rs`
- [x] T008 [P] Implement `SearchError` enum with `thiserror` derives in `plugins/web/src/search/mod.rs`
- [x] T009 [P] Implement `SearchProvider` async trait (name, search method) in `plugins/web/src/search/mod.rs`
- [x] T010 [P] Implement `FetchedContent` struct (url, title, text, content_type, content_length, truncated, status_code) in `plugins/web/src/content.rs`
- [x] T011 [P] Implement `SearchProviderKind` enum (DuckDuckGo, Brave, Tavily) in `plugins/web/src/config.rs`
- [x] T012 [P] Implement `ExtractionPreset` enum (Links, Headings, Tables), `ExtractedElement` struct, `Viewport` struct in `plugins/web/src/playwright.rs`
- [x] T013 Implement `WebPluginConfig` struct with all fields from data-model.md (plus `user_agent: String` defaulting to `"SwinkAgent/0.5"`) and `WebPluginConfigBuilder` with `with_*()` builder methods in `plugins/web/src/config.rs`
- [x] T014 Write tests for `WebPluginConfigBuilder` defaults and overrides in `plugins/web/tests/config_test.rs`
- [x] T015 Implement `DomainFilter` struct (allowlist, denylist, block_private_ips) with `is_allowed(&self, url: &Url) -> Result<(), DomainFilterError>` method in `plugins/web/src/domain.rs` — parse URL, validate scheme (http/https only), check allowlist/denylist, resolve DNS and check against private IP ranges (127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, ::1, fc00::/7, 0.0.0.0)
- [x] T016 Write tests for `DomainFilter` covering: allowed domain, denied domain, private IP blocking (127.0.0.1, 10.x, 172.16.x, 192.168.x), scheme rejection (file://, ftp://), allowlist-only mode, empty config in `plugins/web/tests/domain_filter_test.rs`
- [x] T017 Implement `WebPlugin` struct skeleton in `plugins/web/src/plugin.rs` — holds `WebPluginConfig`, `reqwest::Client` (built with `user_agent` from config), `Arc<Mutex<Option<PlaywrightBridge>>>`, `Arc<Mutex<VecDeque<Instant>>>`, `Arc<dyn SearchProvider>`. Constructor `new()` for defaults, `builder()` returning `WebPluginConfigBuilder`
- [x] T018 Implement `Plugin` trait for `WebPlugin` in `plugins/web/src/plugin.rs` — `name()` returns `"web"`, stub `tools()`, `pre_dispatch_policies()`, `post_turn_policies()`, `on_event()` (populated in later phases)
- [x] T019 Update `plugins/web/src/lib.rs` re-exports for all foundational types
- [x] T020 Verify `cargo test -p swink-agent-plugin-web` passes with foundational tests

**Checkpoint**: Foundation ready — shared types, config builder, domain filter, plugin skeleton all working

---

## Phase 3: User Story 5 — Domain Filtering Blocks Unsafe Requests (Priority: P1)

**Goal**: PreDispatch policy that blocks requests to private/internal IPs and denied domains before any HTTP call

**Independent Test**: Configure a denylist, call web tools with blocked domains, verify rejection without network calls

### Tests for User Story 5

- [x] T021 [P] [US5] Write tests for `DomainFilterPolicy` as `PreDispatchPolicy`: blocked domain returns `Skip`, allowed domain returns `Continue`, private IP returns `Skip`, no denylist still blocks private IPs, allowlist mode rejects unlisted domains, non-web tool (no `url` arg or tool name not prefixed `web.`) returns `Continue` (passthrough) — in `plugins/web/tests/domain_filter_test.rs`

### Implementation for User Story 5

- [x] T022 [US5] Implement `DomainFilterPolicy` struct wrapping `DomainFilter` in `plugins/web/src/policy/domain_filter.rs` — implement `PreDispatchPolicy` trait: `name()` returns `"web.domain_filter"`, `evaluate()` first checks tool name starts with `"web."` (return `Continue` immediately for non-web tools), then extracts URL from tool arguments (`url` field — return `Continue` if absent), calls `DomainFilter::is_allowed()`, returns `PreDispatchVerdict::Skip` with error message if blocked, `Continue` if allowed
- [x] T023 [US5] Create `plugins/web/src/policy/mod.rs` re-exporting `DomainFilterPolicy`
- [x] T024 [US5] Wire `DomainFilterPolicy` into `WebPlugin::pre_dispatch_policies()` in `plugins/web/src/plugin.rs`
- [x] T025 [US5] Verify domain filter policy tests pass in `plugins/web/tests/domain_filter_test.rs`

**Checkpoint**: Domain filtering policy active — all web tools are protected against SSRF

---

## Phase 4: User Story 1 — Fetch and Read a Web Page (Priority: P1) MVP

**Goal**: `web.fetch` tool retrieves a URL via reqwest, extracts readable content via `readability` crate, returns clean text

**Independent Test**: Fetch a known URL (mock server), verify returned content is clean text with boilerplate removed

### Tests for User Story 1

- [x] T026 [P] [US1] Write tests for HTML content extraction: clean article, page with nav/ads/scripts stripped, truncation at max_content_length (beginning + end preserved), non-HTML content type detection, readability extraction effectiveness (output < 20% of raw HTML length for representative page fixture, per SC-006) — in `plugins/web/tests/fetch_test.rs`
- [x] T027 [P] [US1] Write tests for `FetchTool::execute()`: successful fetch returns clean text, non-200 status returns error, redirect following, content truncation notice, non-HTML content type returns guidance message — in `plugins/web/tests/fetch_test.rs` (uses mock HTTP server from `tests/common/mod.rs`)

### Implementation for User Story 1

- [x] T028 [US1] Implement `extract_readable_content(html: &[u8], url: &Url) -> Result<FetchedContent, ContentError>` in `plugins/web/src/content.rs` — use `readability::extractor::extract()`, populate `FetchedContent` fields, handle extraction failures gracefully
- [x] T029 [US1] Implement `truncate_content(text: &str, max_len: usize) -> (String, bool)` in `plugins/web/src/content.rs` — preserve beginning (80%) and end (20%) with truncation notice in the middle
- [x] T030 [US1] Implement `FetchTool` struct in `plugins/web/src/tools/fetch.rs` — holds `reqwest::Client`, `max_content_length`, `request_timeout`. Implement `AgentTool` trait: `name()` = `"fetch"`, `description()` explains fetching web pages, `parameters_schema()` per contracts (url required), `execute()` performs reqwest GET, checks Content-Type header, extracts readable content, truncates if needed, returns `AgentToolResult::text()` or `AgentToolResult::error()`. Domain filtering is handled by the PreDispatch policy — tool trusts the policy pipeline.
- [x] T031 [US1] Create `plugins/web/src/tools/mod.rs` re-exporting `FetchTool`
- [x] T032 [US1] Wire `FetchTool` into `WebPlugin::tools()` in `plugins/web/src/plugin.rs`
- [x] T033 [US1] Verify fetch tool tests pass in `plugins/web/tests/fetch_test.rs`

**Checkpoint**: Agent can fetch any public web page and get clean readable text. MVP core complete.

---

## Phase 5: User Story 2 — Search the Web (Priority: P1)

**Goal**: `web.search` tool with pluggable providers. DuckDuckGo default (zero config), Brave and Tavily behind feature gates.

**Independent Test**: Search with default provider, verify results have title/URL/snippet. Switch to Brave/Tavily via builder, verify provider change.

### Tests for User Story 2

- [x] T034 [P] [US2] Write tests for `DuckDuckGoProvider`: parse known HTML response fixture, extract title/URL/snippet, handle empty results, handle malformed HTML — in `plugins/web/tests/search_test.rs`
- [x] T035 [P] [US2] Write tests for `SearchTool::execute()`: successful search returns formatted results, empty results return message (not error), max_results parameter respected — in `plugins/web/tests/search_test.rs`

### Implementation for User Story 2

- [x] T036 [US2] Implement `DuckDuckGoProvider` in `plugins/web/src/search/duckduckgo.rs` — `SearchProvider` impl: POST to `https://lite.duckduckgo.com/lite/` with `q=<query>` form data via `reqwest`, parse HTML response with `scraper` crate (CSS selectors for result links, titles, snippets), return `Vec<SearchResult>`. Feature-gated behind `duckduckgo`
- [x] T037 [P] [US2] Implement `BraveProvider` in `plugins/web/src/search/brave.rs` — `SearchProvider` impl: GET to Brave Search API with `X-Subscription-Token` header, parse JSON response, return `Vec<SearchResult>`. Feature-gated behind `brave`
- [x] T038 [P] [US2] Implement `TavilyProvider` in `plugins/web/src/search/tavily.rs` — `SearchProvider` impl: POST to Tavily API with API key in body, parse JSON response, return `Vec<SearchResult>`. Feature-gated behind `tavily`
- [x] T039 [US2] Implement `SearchTool` struct in `plugins/web/src/tools/search.rs` — holds `Arc<dyn SearchProvider>`, `max_search_results`. Implement `AgentTool` trait: `name()` = `"search"`, `parameters_schema()` per contracts (query required, max_results optional), `execute()` delegates to provider, formats results as numbered list text
- [x] T040 [US2] Update `plugins/web/src/search/mod.rs` with feature-gated re-exports for all providers
- [x] T041 [US2] Update `plugins/web/src/tools/mod.rs` re-exporting `SearchTool`
- [x] T042 [US2] Wire `SearchTool` into `WebPlugin::tools()` and wire provider selection in `WebPlugin` constructor based on `SearchProviderKind` config — in `plugins/web/src/plugin.rs`
- [x] T043 [US2] Verify search tool tests pass in `plugins/web/tests/search_test.rs`

**Checkpoint**: Agent can search the web with zero config (DuckDuckGo). Brave/Tavily available via builder.

---

## Phase 6: User Story 3 — Screenshot a Web Page (Priority: P2)

**Goal**: `web.screenshot` tool renders a URL via Playwright subprocess, returns base64 PNG as `ContentBlock::Image`

**Independent Test**: Screenshot a known URL (or local test page), verify result is valid PNG image data in a ContentBlock::Image

### Tests for User Story 3

- [x] T044 [P] [US3] Write tests for `PlaywrightBridge`: startup/shutdown lifecycle, send screenshot request and receive response, handle subprocess not found (Playwright not installed), handle timeout — in `plugins/web/tests/playwright_test.rs` (gated behind `integration` feature)
- [x] T045 [P] [US3] Write unit tests for `ScreenshotTool::execute()`: missing Playwright returns installation guidance error, valid params construct correct request — in `plugins/web/tests/screenshot_test.rs`

### Implementation for User Story 3

- [x] T046 [US3] Create the Playwright bridge Node.js script as an embedded string constant in `plugins/web/src/playwright.rs` — `const BRIDGE_SCRIPT: &str = include_str!("playwright_bridge.js")` or inline string. Script accepts JSON-newline commands on stdin, responds on stdout. Supports `screenshot` (navigate, capture, base64 encode) and `ping`/`shutdown` actions. Include error handling and timeout per request.
- [x] T047 [US3] Create `plugins/web/src/playwright_bridge.js` — Node.js script: reads JSON lines from stdin, launches Playwright chromium browser on first request, handles `screenshot` action (navigate to URL, set viewport, take screenshot, respond with base64 PNG), handles `extract` action (navigate, querySelectorAll, respond with elements), handles `ping`/`shutdown`, timeout per navigation, process cleanup on shutdown
- [x] T048 [US3] Implement `PlaywrightBridge` struct in `plugins/web/src/playwright.rs` — `start()` writes bridge script to temp file, spawns `node <tempfile>` via `tokio::process::Command`, stores `Child`/`ChildStdin`/`BufReader<ChildStdout>`. `send_request(request: PlaywrightRequest) -> Result<PlaywrightResponse>` writes JSON line to stdin, reads JSON line from stdout (with timeout). `shutdown()` sends shutdown command, waits, kills if needed. `Drop` impl calls shutdown.
- [x] T049 [US3] Add request ID tracking to `PlaywrightBridge` for concurrent request support — atomic counter for IDs, response matching by ID in `plugins/web/src/playwright.rs`
- [x] T050 [US3] Implement `ScreenshotTool` struct in `plugins/web/src/tools/screenshot.rs` — holds `Arc<Mutex<Option<PlaywrightBridge>>>`, `Viewport` defaults, `screenshot_timeout`. Implement `AgentTool` trait: `name()` = `"screenshot"`, `parameters_schema()` per contracts (url required, width/height optional), `execute()` lazily starts bridge, sends screenshot request, returns `AgentToolResult` with `ContentBlock::Image { source: ImageSource::Base64 { media_type: "image/png".into(), data } }`. Handle bridge errors with clear messages (Playwright not installed, timeout, navigation failure).
- [x] T051 [US3] Update `plugins/web/src/tools/mod.rs` re-exporting `ScreenshotTool`
- [x] T052 [US3] Wire `ScreenshotTool` into `WebPlugin::tools()` with shared `PlaywrightBridge` handle in `plugins/web/src/plugin.rs`
- [x] T053 [US3] Verify screenshot tests pass — unit tests always, integration tests with `--features integration`

**Checkpoint**: Agent can capture PNG screenshots of web pages, returned as ContentBlock::Image for multi-modal LLMs

---

## Phase 7: User Story 4 — Extract Structured Content from a Page (Priority: P2)

**Goal**: `web.extract` tool uses Playwright to render page and extract elements by CSS selector or preset

**Independent Test**: Extract headings from a known page, verify structured output matches page content

### Tests for User Story 4

- [x] T054 [P] [US4] Write tests for `ExtractTool::execute()`: CSS selector extraction, preset extraction (links, headings, tables), empty result returns message not error, no selector and no preset defaults to all text — in `plugins/web/tests/extract_test.rs`

### Implementation for User Story 4

- [x] T055 [US4] Implement `ExtractTool` struct in `plugins/web/src/tools/extract.rs` — holds `Arc<Mutex<Option<PlaywrightBridge>>>`, timeout config. Implement `AgentTool` trait: `name()` = `"extract"`, `parameters_schema()` per contracts (url required, selector/preset mutually exclusive optional), `execute()` lazily starts bridge, sends extract request with selector or preset, formats `Vec<ExtractedElement>` as structured JSON text in `AgentToolResult::text()`. Handle: no matches (descriptive message), Playwright not installed (guidance error), timeout
- [x] T056 [US4] Update `plugins/web/src/tools/mod.rs` re-exporting `ExtractTool`
- [x] T057 [US4] Wire `ExtractTool` into `WebPlugin::tools()` with shared `PlaywrightBridge` handle in `plugins/web/src/plugin.rs`
- [x] T058 [US4] Verify extract tests pass in `plugins/web/tests/extract_test.rs`

**Checkpoint**: Agent can extract structured content (links, headings, tables, CSS selectors) from JS-rendered pages

---

## Phase 8: User Story 6 — Rate Limiting Prevents Abuse (Priority: P2)

**Goal**: PreDispatch policy enforcing shared rate limit across all web tools

**Independent Test**: Issue rapid requests exceeding limit, verify rejection. Wait for window, verify resume.

### Tests for User Story 6

- [x] T059 [P] [US6] Write tests for `RateLimitPolicy`: requests within limit pass, exceeding limit returns Skip, window expiry resets counter, default 30 RPM applied when unconfigured, concurrent access safety, non-web tool (name not prefixed `web.`) returns `Continue` (passthrough) — in `plugins/web/tests/rate_limiter_test.rs`

### Implementation for User Story 6

- [x] T060 [US6] Implement `RateLimitPolicy` struct in `plugins/web/src/policy/rate_limiter.rs` — holds `Arc<Mutex<VecDeque<Instant>>>`, `rate_limit_rpm: u32`. Implement `PreDispatchPolicy` trait: `name()` = `"web.rate_limiter"`, `evaluate()` first checks tool name starts with `"web."` (return `Continue` immediately for non-web tools), then prunes timestamps older than 60s, checks count vs limit, pushes current timestamp if allowed, returns `Continue` or `Skip("Rate limit exceeded: {limit} requests per minute")`
- [x] T061 [US6] Update `plugins/web/src/policy/mod.rs` re-exporting `RateLimitPolicy`
- [x] T062 [US6] Wire `RateLimitPolicy` into `WebPlugin::pre_dispatch_policies()` with shared rate state in `plugins/web/src/plugin.rs`
- [x] T063 [US6] Verify rate limiter tests pass in `plugins/web/tests/rate_limiter_test.rs`

**Checkpoint**: All web tools share a rate limit. Burst requests are rejected.

---

## Phase 9: User Story 7 — Content Sanitization Protects Context (Priority: P2)

**Goal**: PostTurn policy that strips known prompt injection patterns from web content in tool results

**Independent Test**: Fetch page with injection patterns, verify they are stripped. Verify legitimate content preserved.

### Tests for User Story 7

- [x] T064 [P] [US7] Write tests for `ContentSanitizerPolicy`: strips "ignore all previous instructions", strips "you are now", strips "system:" prefix patterns, preserves legitimate instruction-like content, handles empty content, multiple patterns in one response, only scans tool results from `web.*` tools (non-web tool results passed through unchanged) — in `plugins/web/tests/sanitizer_test.rs`

### Implementation for User Story 7

- [x] T065 [US7] Implement `ContentSanitizerPolicy` struct in `plugins/web/src/policy/sanitizer.rs` — compile regex patterns at construction (common injection patterns: `(?i)ignore\s+(all\s+)?previous\s+instructions`, `(?i)you\s+are\s+now\s+`, `(?i)^system:\s*`, `(?i)IMPORTANT:\s*ignore`, `(?i)disregard\s+(all\s+)?(previous|above)`, etc.). Implement `PostTurnPolicy` trait: `name()` = `"web.sanitizer"`, `evaluate()` only scans tool result `ContentBlock::Text` values from `web.*` tool results in turn messages (skip non-web tool results), strips matching patterns, returns `Inject` with sanitized messages if modified, `Continue` if clean
- [x] T066 [US7] Update `plugins/web/src/policy/mod.rs` re-exporting `ContentSanitizerPolicy`
- [x] T067 [US7] Wire `ContentSanitizerPolicy` into `WebPlugin::post_turn_policies()` (conditionally on `sanitizer_enabled` config) in `plugins/web/src/plugin.rs`
- [x] T068 [US7] Verify sanitizer tests pass in `plugins/web/tests/sanitizer_test.rs`

**Checkpoint**: Web content is scanned for prompt injection patterns before persisting in agent context

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Event observer, full Plugin wiring validation, integration testing, documentation

- [x] T069 [P] Implement `on_event()` observer in `WebPlugin` in `plugins/web/src/plugin.rs` — log `ToolExecutionStart`/`ToolExecutionEnd` events for web tools with URL, status, content size, latency via `tracing::info!`
- [x] T070 [P] Write integration test: register `WebPlugin` with a mock `Agent`, verify all 4 tools are discoverable with `web.` namespace prefix, verify all 3 policies are contributed — in `plugins/web/tests/integration_test.rs`
- [x] T071 [P] Write integration test: full fetch → search → screenshot flow with mock HTTP server (fetch and search only, screenshot gated behind `integration` feature) — in `plugins/web/tests/integration_test.rs`
- [x] T072 Update `plugins/web/src/lib.rs` with complete public re-exports and crate-level doc comment
- [x] T073 Run `cargo clippy -p swink-agent-plugin-web -- -D warnings` and fix all warnings
- [x] T074 Run `cargo test -p swink-agent-plugin-web --features all` and verify all tests pass
- [x] T075 Validate quickstart.md code examples compile correctly

**Checkpoint**: Plugin fully wired, all tests pass, clippy clean, ready for integration with swink-agent consumers

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US5 Domain Filtering (Phase 3)**: Depends on Foundational — prerequisite for US1 fetch safety
- **US1 Fetch (Phase 4)**: Depends on Foundational + US5 (domain filter needed for safe fetching)
- **US2 Search (Phase 5)**: Depends on Foundational only — independent of US1 and US5 (search doesn't fetch URLs)
- **US3 Screenshot (Phase 6)**: Depends on Foundational — introduces Playwright bridge
- **US4 Extract (Phase 7)**: Depends on US3 (shares Playwright bridge)
- **US6 Rate Limiting (Phase 8)**: Depends on Foundational only — independent policy
- **US7 Sanitization (Phase 9)**: Depends on Foundational only — independent policy
- **Polish (Phase 10)**: Depends on all user stories complete

### User Story Dependencies

- **US5 (Domain Filter)**: Foundational only — no story dependencies
- **US1 (Fetch)**: Foundational + US5 (domain filter policy must exist for safe requests)
- **US2 (Search)**: Foundational only — fully independent
- **US3 (Screenshot)**: Foundational only — introduces Playwright bridge
- **US4 (Extract)**: US3 (reuses Playwright bridge and bridge script)
- **US6 (Rate Limit)**: Foundational only — fully independent
- **US7 (Sanitization)**: Foundational only — fully independent

### Parallel Opportunities

After Foundational (Phase 2) completes, these can run in parallel:
- **Stream A**: US5 → US1 (domain filter then fetch)
- **Stream B**: US2 (search — fully independent)
- **Stream C**: US3 → US4 (Playwright bridge then extract)
- **Stream D**: US6 (rate limiting — fully independent)
- **Stream E**: US7 (sanitization — fully independent)

### Within Each User Story

- Tests written first and verified to fail
- Core types/logic before tool implementation
- Tool wired into Plugin last
- Story verified at checkpoint before moving on

---

## Parallel Example: After Foundational

```bash
# Stream A: Domain Filter + Fetch (sequential)
Task: "T021-T025: Domain filter policy"
Task: "T026-T033: Fetch tool" (after T025)

# Stream B: Search (independent, can run with Stream A)
Task: "T034-T043: Search tool + providers"

# Stream C: Screenshot + Extract (sequential)
Task: "T044-T053: Playwright bridge + screenshot"
Task: "T054-T058: Extract tool" (after T053)

# Stream D: Rate Limiter (independent)
Task: "T059-T063: Rate limit policy"

# Stream E: Sanitizer (independent)
Task: "T064-T068: Content sanitizer policy"
```

---

## Implementation Strategy

### MVP First (P1 Stories Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: US5 — Domain Filtering
4. Complete Phase 4: US1 — Fetch and Read
5. Complete Phase 5: US2 — Search
6. **STOP and VALIDATE**: Agent can fetch pages and search the web safely
7. Deploy/demo if ready

### Incremental Delivery

1. Setup + Foundational → Crate compiles
2. Add US5 (Domain Filter) → Safety baseline
3. Add US1 (Fetch) → Core value delivered (MVP!)
4. Add US2 (Search) → Discovery capability
5. Add US3 (Screenshot) → Visual inspection
6. Add US4 (Extract) → Structured data extraction
7. Add US6 (Rate Limiting) → Abuse prevention
8. Add US7 (Sanitization) → Injection defense
9. Polish → Production ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story
- Each user story independently testable at its checkpoint
- Constitution II: tests written before implementation, verified to fail
- `integration` feature flag gates tests requiring Playwright CLI
- Commit after each task or logical group
