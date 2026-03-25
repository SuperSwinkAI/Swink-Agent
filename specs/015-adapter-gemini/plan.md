# Implementation Plan: Adapter: Google Gemini

**Branch**: `015-adapter-gemini` | **Date**: 2026-03-24 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/015-adapter-gemini/spec.md`

## Summary

Implement `GeminiStreamFn`, a `StreamFn` adapter for the Google Generative Language API (`streamGenerateContent` via SSE). The adapter streams text, tool-call, and thinking content blocks, converts agent messages to Gemini's parts-based content format with function declarations, classifies HTTP errors via the shared error classifier, and surfaces safety filter blocks as errors. All code lives in `adapters/src/google.rs` within the `swink-agent-adapters` crate.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`
**Storage**: N/A
**Testing**: `cargo test -p swink-agent-adapters`; wiremock unit tests in `adapters/tests/google.rs`, live integration tests in `adapters/tests/google_live.rs` (`#[ignore]`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Incremental SSE event emission (no buffering entire response); zero-copy where possible
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core; `GeminiStreamFn` must be `Send + Sync`
**Scale/Scope**: Single adapter module (~800–900 lines)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All code lives in the `swink-agent-adapters` crate. `GeminiStreamFn` is a public type re-exported from `lib.rs`. No service, no daemon. |
| II | Test-Driven Development | PASS | Wiremock unit tests for text streaming, tool calls, thinking blocks, safety filters, and error classification. Live integration tests (`#[ignore]`) validate end-to-end streaming. |
| III | Efficiency & Performance | PASS | SSE parsing is incremental (line-by-line via `stream::unfold`). Events emitted as they arrive, never buffered. First-candidate selection is O(1). |
| IV | Leverage the Ecosystem | PASS | Uses `reqwest` for HTTP, `futures` for stream combinators, `serde_json` for JSON parsing, `tokio-util` for `CancellationToken`, `wiremock` for tests. No reinvented wheels. |
| V | Provider Agnosticism | PASS | Core crate has no knowledge of Google/Gemini. `GeminiStreamFn` implements `StreamFn` — the sole provider boundary. Zero changes to core required. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. HTTP errors produce error events via shared classifier, never panics. Stream state implements `StreamFinalize` for clean finalization on cancellation or unexpected stream end. Compile-time `Send + Sync` assertion. |

## Project Structure

### Documentation (this feature)

```text
specs/015-adapter-gemini/
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
├── lib.rs               # Crate root — re-exports GeminiStreamFn
├── google.rs            # GeminiStreamFn, message conversion, SSE parsing, thinking support
├── classify.rs          # HttpErrorKind, classify_http_status, error_event_from_status (shared infra)
├── convert.rs           # extract_tool_schemas (shared infra)
├── finalize.rs          # StreamFinalize trait, finalize_blocks (shared infra)
└── sse.rs               # SseLine, sse_data_lines (shared infra)

adapters/tests/
├── google.rs            # Wiremock unit tests (text, tool calls, thinking, safety, errors)
└── google_live.rs       # Live integration tests (#[ignore])
```

**Structure Decision**: Single file (`google.rs`) in the existing adapters crate. The Gemini adapter uses shared infrastructure from `classify.rs`, `convert.rs`, `finalize.rs`, and `sse.rs`. Custom `convert_messages` function required due to Gemini's parts-based format, function declarations, inline data for images, and thinking blocks with signatures. No new crates or modules needed.

## Complexity Tracking

No constitution violations. All code fits within the existing `swink-agent-adapters` crate boundary.
