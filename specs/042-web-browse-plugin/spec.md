# Feature Specification: Web Browse Plugin

**Feature Branch**: `042-web-browse-plugin`  
**Created**: 2026-04-04  
**Status**: Draft  
**Input**: User description: "Web browse plugin with Playwright, search providers (DuckDuckGo default, Brave, Tavily), domain filtering, rate limiting, and content sanitization"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Fetch and Read a Web Page (Priority: P1)

A developer configures an agent with the web browse plugin. The agent receives a user message asking about content on a specific URL. The agent calls the `web.fetch` tool with the URL, receives clean readable text extracted from the HTML (no navigation, ads, or scripts), and uses that content to answer the user's question.

**Why this priority**: Fetching and reading web pages is the foundational capability. Every other feature (search, screenshots, extraction) builds on top of reliable HTTP fetching and content cleaning. Without this, the plugin delivers no value.

**Independent Test**: Can be fully tested by fetching a known public URL and verifying that the returned content is clean, readable text that preserves the page's meaningful content while stripping boilerplate.

**Acceptance Scenarios**:

1. **Given** an agent with the web plugin registered, **When** the agent calls `web.fetch` with a valid public URL, **Then** the plugin returns the page's main content as clean text/markdown with navigation, ads, scripts, and boilerplate removed.
2. **Given** an agent calls `web.fetch`, **When** the target URL returns a non-200 status code, **Then** the plugin returns a descriptive error including the status code and reason.
3. **Given** an agent calls `web.fetch`, **When** the fetched content exceeds the configured maximum length, **Then** the content is truncated intelligently (preserving the beginning and end) with a note indicating truncation occurred.

---

### User Story 2 - Search the Web (Priority: P1)

A developer configures an agent with the web browse plugin using no API keys. The agent receives a question it cannot answer from its training data. The agent calls the `web.search` tool with a query string and receives a ranked list of results (title, URL, snippet) from DuckDuckGo. The agent can then decide to fetch specific results for deeper reading.

**Why this priority**: Web search is equally critical to fetch — it lets the agent discover URLs rather than requiring the user to provide them. DuckDuckGo as the default means zero configuration, making this immediately usable.

**Independent Test**: Can be fully tested by running a search query and verifying that results include titles, URLs, and snippets, and that results are relevant to the query.

**Acceptance Scenarios**:

1. **Given** an agent with the web plugin using the default search provider (DuckDuckGo), **When** the agent calls `web.search` with a query, **Then** the plugin returns a ranked list of results with title, URL, and snippet for each.
2. **Given** a developer configures the plugin with a Brave API key, **When** the agent calls `web.search`, **Then** results come from the Brave Search API instead of DuckDuckGo.
3. **Given** a developer configures the plugin with a Tavily API key, **When** the agent calls `web.search`, **Then** results come from the Tavily API instead of DuckDuckGo.
4. **Given** a search query returns no results, **When** the agent receives the response, **Then** the plugin returns an empty result set with a descriptive message, not an error.

---

### User Story 3 - Screenshot a Web Page (Priority: P2)

A developer needs an agent that can visually inspect web pages — for example, checking a deployment, reviewing a UI, or capturing visual evidence. The agent calls `web.screenshot` with a URL. The plugin launches a headless browser via the Playwright CLI, renders the page, captures a screenshot, and returns the image data.

**Why this priority**: Screenshots unlock visual understanding, which is valuable for UI review, debugging, and verification tasks. However, it requires an external dependency (Playwright CLI) and is a more specialized use case than text-based fetch/search.

**Independent Test**: Can be fully tested by taking a screenshot of a known public URL and verifying that the returned image is a valid image file that visually represents the page.

**Acceptance Scenarios**:

1. **Given** an agent with the web plugin and Playwright CLI installed on the host, **When** the agent calls `web.screenshot` with a URL, **Then** the plugin returns the screenshot as image data (PNG).
2. **Given** the Playwright CLI is not installed on the host, **When** the agent calls `web.screenshot`, **Then** the plugin returns a clear error message indicating that Playwright needs to be installed, with guidance on how to install it.
3. **Given** the agent calls `web.screenshot` with optional viewport size parameters, **When** the screenshot is taken, **Then** the browser renders at the specified dimensions.

---

### User Story 4 - Extract Structured Content from a Page (Priority: P2)

A developer needs the agent to extract specific structured data from a web page — for example, all links, all headings, all table data, or content matching a CSS selector. The agent calls `web.extract` with a URL and extraction parameters, and receives structured results.

**Why this priority**: Structured extraction is more efficient than dumping the entire page into context. It lets the agent surgically retrieve what it needs, saving tokens and improving answer quality.

**Independent Test**: Can be fully tested by extracting headings or links from a known page and verifying the structured output matches the page content.

**Acceptance Scenarios**:

1. **Given** an agent calls `web.extract` with a URL and a CSS selector, **When** the page is fetched, **Then** the plugin returns all elements matching the selector as structured text.
2. **Given** an agent calls `web.extract` with a URL and a preset extraction type ("links", "headings", "tables"), **When** the page is fetched, **Then** the plugin returns the requested elements in a structured format.
3. **Given** a CSS selector matches no elements on the page, **When** the agent receives the response, **Then** the plugin returns an empty result set with a descriptive message.

---

### User Story 5 - Domain Filtering Blocks Unsafe Requests (Priority: P1)

A developer configures the web plugin with a domain denylist (e.g., blocking internal network ranges, localhost, or specific domains). When the agent attempts to fetch or search a blocked domain, the request is rejected before any HTTP call is made, and the agent receives an explanatory error.

**Why this priority**: Safety is critical from day one. Without domain filtering, the web plugin is an SSRF vector. This must ship alongside the fetch/search tools, not as a follow-up.

**Independent Test**: Can be fully tested by configuring a denylist and verifying that requests to blocked domains are rejected without making any network calls.

**Acceptance Scenarios**:

1. **Given** a domain denylist includes "internal.corp.com", **When** the agent calls `web.fetch` with a URL on that domain, **Then** the request is rejected with an error explaining the domain is blocked, and no HTTP request is made.
2. **Given** no explicit denylist is configured, **When** the agent calls `web.fetch` with a localhost or private IP address (127.0.0.1, 10.x.x.x, 192.168.x.x), **Then** the request is rejected by the built-in SSRF protection.
3. **Given** a domain allowlist is configured, **When** the agent calls `web.fetch` with a URL not on the allowlist, **Then** the request is rejected.

---

### User Story 6 - Rate Limiting Prevents Abuse (Priority: P2)

The agent enters a loop where it rapidly fetches dozens of pages. The rate limiter policy detects that requests exceed the configured threshold and starts rejecting additional requests, preventing the agent from overwhelming target servers or burning through search API quotas.

**Why this priority**: Rate limiting protects both external servers from abuse and the user from unexpected API costs. It's important but secondary to the core fetch/search/safety functionality.

**Independent Test**: Can be fully tested by issuing rapid sequential requests and verifying that requests beyond the configured limit are rejected with an appropriate error.

**Acceptance Scenarios**:

1. **Given** a rate limit of 10 requests per minute, **When** the agent makes an 11th request within the same minute, **Then** the request is rejected with a "rate limit exceeded" error.
2. **Given** the rate limit window has elapsed, **When** the agent makes a new request, **Then** the request succeeds normally.
3. **Given** the developer has not configured a rate limit, **When** the plugin is initialized, **Then** a sensible default rate limit is applied (not unlimited).

---

### User Story 7 - Content Sanitization Protects Context (Priority: P2)

A fetched web page contains prompt injection attempts embedded in its content (e.g., "Ignore all previous instructions and..."). The content sanitizer policy runs after the turn and strips or neutralizes known injection patterns before the content persists in the agent's context.

**Why this priority**: Prompt injection via fetched web content is a real attack vector. While not blocking for basic functionality, it's essential for production safety.

**Independent Test**: Can be fully tested by fetching a page with known injection patterns and verifying they are stripped or neutralized in the returned content.

**Acceptance Scenarios**:

1. **Given** a fetched page contains text matching known prompt injection patterns, **When** the content passes through the sanitizer, **Then** the suspicious patterns are stripped or escaped.
2. **Given** a fetched page contains legitimate content that happens to include instruction-like language, **When** the content passes through the sanitizer, **Then** the legitimate content is preserved (low false-positive rate).
3. **Given** the sanitizer is applied, **When** the agent processes the cleaned content, **Then** the content remains coherent and useful for answering the user's question.

---

### Edge Cases

- What happens when a URL redirects multiple times? The plugin should follow redirects up to a configurable limit (default: 10) and return the final page's content.
- What happens when a page requires JavaScript rendering to display content? The `web.fetch` tool returns the static HTML content only. Users needing JS-rendered content should use `web.screenshot` or `web.extract` (future: headless fetch via Playwright).
- What happens when the search provider (DuckDuckGo) is temporarily unavailable? The plugin returns a clear error indicating the search backend is unreachable, allowing the agent to inform the user rather than silently failing.
- What happens when a fetched page is extremely large (e.g., a log dump or data file)? Content is truncated at the configured maximum length with a truncation notice appended.
- What happens when the agent calls `web.fetch` on a non-HTML resource (PDF, image, binary)? The plugin detects the content type and returns an appropriate message (e.g., "This URL points to a PDF document. Use web.screenshot to view it visually.").
- What happens when the Playwright process hangs or crashes during a screenshot? A timeout terminates the process and returns an error to the agent.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The plugin MUST implement the `Plugin` trait from `swink-agent` core, registering all tools, policies, and event observers as a single composable unit.
- **FR-002**: The plugin MUST provide a `web.fetch` tool that retrieves a URL and returns the page's main content as clean, readable text with boilerplate removed.
- **FR-003**: The plugin MUST provide a `web.search` tool that accepts a query string and returns a ranked list of results (title, URL, snippet).
- **FR-004**: The plugin MUST provide a `web.screenshot` tool that renders a URL in a headless browser and returns a PNG screenshot.
- **FR-005**: The plugin MUST provide a `web.extract` tool that retrieves structured content from a URL using CSS selectors or preset extraction types (links, headings, tables).
- **FR-006**: The plugin MUST use DuckDuckGo as the default search provider, requiring no API keys for basic operation.
- **FR-007**: The plugin MUST support Brave Search as an alternative search provider when an API key is configured.
- **FR-008**: The plugin MUST support Tavily as an alternative search provider when an API key is configured.
- **FR-009**: The plugin MUST include a PreDispatch domain filtering policy that blocks requests to private/internal IP ranges (SSRF protection) by default, with configurable allowlist and denylist.
- **FR-010**: The plugin MUST include a PreDispatch rate limiting policy with a configurable requests-per-minute threshold and a sensible default.
- **FR-011**: The plugin MUST include a PostTurn content sanitization policy that strips known prompt injection patterns from fetched web content.
- **FR-012**: The plugin MUST use the Playwright CLI as an external process for headless browser operations (screenshot), not link a browser engine into the binary.
- **FR-013**: The plugin MUST provide a builder-style configuration API for setting search provider, domain filters, rate limits, content length limits, and Playwright path.
- **FR-014**: The plugin MUST extract readable content from HTML using a readability-style algorithm that strips navigation, ads, scripts, and boilerplate while preserving meaningful content structure.
- **FR-015**: The plugin MUST truncate fetched content that exceeds the configured maximum length, preserving content from the beginning and end with a truncation notice.
- **FR-016**: The plugin MUST log every web request (URL, status, content size, latency) via the event observer for debugging and auditing.
- **FR-017**: The plugin MUST detect non-HTML content types and return an appropriate message rather than dumping binary data into the agent's context.
- **FR-018**: The plugin MUST gracefully handle Playwright not being installed, returning a clear error with installation guidance rather than an opaque failure.
- **FR-019**: The search provider MUST be abstracted behind a trait so that additional providers can be added without modifying existing code.
- **FR-020**: Each search provider (Brave, Tavily) MUST be independently feature-gated so that unused providers add zero compile-time cost.

### Key Entities

- **SearchResult**: A single search result containing a title, URL, and text snippet.
- **SearchProvider**: An abstraction over different search backends (DuckDuckGo, Brave, Tavily) that accepts a query and returns ranked results.
- **WebPluginConfig**: The configuration for the plugin, including search provider selection, domain filters, rate limits, content limits, and Playwright settings.
- **DomainFilter**: A set of allowlist/denylist rules and built-in SSRF protections that determine whether a URL is safe to request.
- **FetchedContent**: The result of fetching a URL, containing the cleaned text, original URL, status code, and content metadata (type, length, truncation status).

## Assumptions

- Playwright CLI is installed separately by the user; the plugin does not install or manage it.
- DuckDuckGo's HTML lite search endpoint remains publicly accessible for scraping (this is the same approach used by LangChain, AutoGen, and other major frameworks).
- The plugin operates in a workspace crate (`plugins/web/`) following the same conventions as `swink-agent-policies` — depends only on `swink-agent` public API.
- The `web.screenshot` tool returns image data that the LLM can interpret (multi-modal models) or stores it as an artifact for the user.
- Content sanitization targets known prompt injection patterns; it is a defense-in-depth measure, not a guarantee against all injection techniques.
- Default rate limit is conservative (e.g., 30 requests per minute) to prevent accidental abuse while still being useful for normal agent operation.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An agent with the web plugin can fetch any public web page and return readable content within 5 seconds for typical pages.
- **SC-002**: An agent with the web plugin can perform a web search and receive relevant results with zero configuration (no API keys required).
- **SC-003**: An agent with the web plugin can capture a screenshot of any public web page within 15 seconds.
- **SC-004**: All requests to private/internal IP ranges are blocked by default, with zero false negatives on standard SSRF vectors (localhost, 10.x, 172.16-31.x, 192.168.x).
- **SC-005**: Rate limiting prevents more than the configured number of requests per minute, with no requests slipping through during burst scenarios.
- **SC-006**: Fetched content from typical web pages is at least 80% shorter than the raw HTML while retaining all meaningful content (readability extraction effectiveness).
- **SC-007**: The plugin compiles and passes all tests with no external dependencies beyond Playwright CLI (which is only required for screenshot functionality).
- **SC-008**: Switching search providers requires changing a single builder method call — no other code changes needed.
