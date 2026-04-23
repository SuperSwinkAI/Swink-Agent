# Implementation Plan: Adapter: Anthropic

**Branch**: `012-adapter-anthropic` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/012-adapter-anthropic/spec.md`

## Summary

Implement `AnthropicStreamFn`, a `StreamFn` adapter for the Anthropic Messages API (`/v1/messages`). The adapter streams text, tool-call, and thinking content blocks via Server-Sent Events, converts agent messages to Anthropic's format (system prompt as a top-level field, thinking blocks filtered from outgoing requests), classifies HTTP errors with 529 overloaded handling, and supports extended thinking with configurable budget control. All code lives in `adapters/src/anthropic.rs` within the `swink-agent-adapters` crate.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`
**Storage**: N/A
**Testing**: `cargo test -p swink-agent-adapters`; unit tests in module, live integration tests (`#[ignore]`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Zero-copy SSE line parsing where possible; incremental event emission (no buffering entire response)
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core; `AnthropicStreamFn` must be `Send + Sync`
**Scale/Scope**: Single adapter module (~800 lines)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All code lives in the `swink-agent-adapters` crate. `AnthropicStreamFn` is a public type re-exported from `lib.rs`. No service, no daemon. |
| II | Test-Driven Development | PASS | Unit tests for message conversion, thinking resolution, SSE event processing. Live integration tests (`#[ignore]`) validate end-to-end streaming against the real API. |
| III | Efficiency & Performance | PASS | SSE parsing is incremental (line-by-line via `stream::unfold`). Events emitted as they arrive, never buffered. `HashMap` for block index remapping is O(1) lookup. |
| IV | Leverage the Ecosystem | PASS | Uses `reqwest` for HTTP, `futures` for stream combinators, `serde_json` for JSON parsing, `tokio-util` for `CancellationToken`. No reinvented wheels. |
| V | Provider Agnosticism | PASS | Core crate has no knowledge of Anthropic. `AnthropicStreamFn` implements `StreamFn` — the sole provider boundary. Adding Anthropic required zero changes to core. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. HTTP errors produce error events, never panics. `SseStreamState` tracks open blocks for clean finalization on cancellation or unexpected stream end. Compile-time `Send + Sync` assertion. |

## Project Structure

### Documentation (this feature)

```text
specs/012-adapter-anthropic/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
├── checklists/
│   └── requirements.md
└── spec.md
```

### Source Code (repository root)

```text
adapters/src/
├── lib.rs               # Crate root — re-exports AnthropicStreamFn
├── anthropic.rs         # AnthropicStreamFn, message conversion, SSE parsing, thinking support
├── base.rs              # AdapterBase (shared HTTP client)
├── classify.rs          # HttpErrorKind, classify_http_status (shared infra)
├── convert.rs           # MessageConverter trait re-exports (shared infra)
├── finalize.rs          # StreamFinalize trait, finalize_blocks (shared infra)
└── sse.rs               # SseStreamParser, sse_data_lines (shared infra)

adapters/tests/
└── anthropic_live.rs    # Live integration tests (#[ignore])
```

**Structure Decision**: Single file (`anthropic.rs`) in the existing adapters crate. The Anthropic adapter uses shared infrastructure from `base.rs`, `classify.rs`, `convert.rs`, `finalize.rs`, and `sse.rs` but has its own bespoke `convert_messages` function (system prompt is top-level, thinking blocks filtered). No new crates or modules needed.

## Complexity Tracking

No constitution violations. All code fits within the existing `swink-agent-adapters` crate boundary.
