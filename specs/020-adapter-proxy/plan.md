# Implementation Plan: Adapter: Proxy

**Branch**: `020-adapter-proxy` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/020-adapter-proxy/spec.md`

## Summary

Implement `ProxyStreamFn`, a `StreamFn` adapter that forwards LLM calls to an HTTP proxy server over SSE. The proxy relays requests to an upstream LLM provider and streams back typed SSE events (TextDelta, ToolCallDelta, ThinkingDelta, etc.). The adapter authenticates via bearer token, parses the SSE stream using `eventsource-stream`, maps each typed event directly to `AssistantMessageEvent`, and classifies errors (connection, auth, rate-limit, malformed) using the shared `classify_http_status` from spec 011.

No delta reconstruction or partial_message diffing is needed: the proxy protocol uses discrete typed delta events, so each SSE event maps 1:1 to an `AssistantMessageEvent`.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `serde`/`serde_json`, `eventsource-stream`, `tokio`, `tokio-util`
**Storage**: N/A
**Testing**: `cargo test -p swink-agent-adapters`; unit tests for SSE event parsing, error classification, and stream lifecycle
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Streaming — events emitted as they arrive from the proxy; no buffering beyond SSE line boundaries
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core; proxy is transparent for content

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | `ProxyStreamFn` is a public type in `swink-agent-adapters`. No service, no daemon. Independently compilable and testable. Re-exported from `lib.rs`. |
| II | Test-Driven Development | PASS | 12 unit tests cover SSE event parsing (all variants), error classification, debug redaction, and terminal-event detection. Live tests are `#[ignore]`. |
| III | Efficiency & Performance | PASS | SSE events are parsed and converted inline — no intermediate buffering or state accumulation. `eventsource-stream` operates on the `bytes_stream()` directly. |
| IV | Leverage the Ecosystem | PASS | Uses `eventsource-stream` for SSE parsing (well-maintained, used by many Rust SSE clients), `reqwest` for HTTP, shared `classify_http_status` from spec 011. |
| V | Provider Agnosticism | PASS | `ProxyStreamFn` implements `StreamFn` — the same trait as all other adapters. The proxy protocol is its own provider; core has no knowledge of it. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. Malformed JSON yields error events, never panics. Bearer token redacted in `Debug`. Cancellation respected via `tokio::select!`. |

## Project Structure

### Documentation (this feature)

```text
specs/020-adapter-proxy/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── spec.md
```

### Source Code (repository root)

```text
adapters/src/
├── lib.rs               # Crate root — re-exports ProxyStreamFn
├── proxy.rs             # ProxyStreamFn, SseEventData, ProxyRequest, ProxyRequestOptions
├── classify.rs          # HttpErrorKind, classify_http_status (shared infra from spec 011)
└── ...                  # Other adapter modules (unchanged)
```

**Structure Decision**: `ProxyStreamFn` and all its supporting types (`SseEventData`, `ProxyRequest`, `ProxyRequestOptions`) live in a single file `adapters/src/proxy.rs`. The proxy module is private; only `ProxyStreamFn` is re-exported from `lib.rs`. This follows the same pattern as all other adapters in the crate.

## Complexity Tracking

No constitution violations. All types fit within the existing `adapters` crate boundary.
