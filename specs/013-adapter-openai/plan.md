# Implementation Plan: Adapter: OpenAI

**Branch**: `013-adapter-openai` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/013-adapter-openai/spec.md`

## Summary

Implement `OpenAiStreamFn`, a `StreamFn` adapter for any OpenAI-compatible chat completions API (`/v1/chat/completions`). The adapter streams text and tool-call content blocks via Server-Sent Events with `[DONE]` sentinel termination, converts agent messages to the OpenAI chat completions format using the shared `OaiConverter` (via the `MessageConverter` trait), classifies HTTP errors for retry, and handles provider-specific quirks leniently (missing fields, absent tool call IDs, unrecognized finish reasons). Multi-provider compatibility (vLLM, LM Studio, Groq, Together, etc.) is achieved through configurable base URLs with no provider-specific code paths. All code lives in `adapters/src/openai.rs` within the `swink-agent-adapters` crate.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid`
**Storage**: N/A
**Testing**: `cargo test -p swink-agent-adapters`; unit tests in module, live integration tests (`#[ignore]`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Incremental SSE event emission (no buffering); zero-copy line parsing via shared `SseStreamParser`
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core; `OpenAiStreamFn` must be `Send + Sync`
**Scale/Scope**: Single adapter module (~450 lines)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All code lives in the `swink-agent-adapters` crate. `OpenAiStreamFn` is a public type re-exported from `lib.rs`. No service, no daemon. |
| II | Test-Driven Development | PASS | Unit tests for SSE parsing, tool call delta processing, and finish reason mapping. Live integration tests (`#[ignore]`) validate end-to-end streaming against the real API. |
| III | Efficiency & Performance | PASS | SSE parsing is incremental via shared `sse_data_lines()`. Events emitted as they arrive, never buffered. `HashMap` for tool call state tracking is O(1) lookup. |
| IV | Leverage the Ecosystem | PASS | Uses `reqwest` for HTTP, `futures` for stream combinators, `serde_json` for JSON parsing, `uuid` for fallback tool call IDs, `tokio-util` for `CancellationToken`. Reuses shared `SseStreamParser`, `MessageConverter`, `StreamFinalize`, and `AdapterBase`. |
| V | Provider Agnosticism | PASS | Core crate has no knowledge of OpenAI. `OpenAiStreamFn` implements `StreamFn` -- the sole provider boundary. Adding the OpenAI adapter required zero changes to core. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. HTTP errors produce error events, never panics. `SseStreamState` tracks open blocks for clean finalization on cancellation or unexpected stream end. Missing fields handled with `#[serde(default)]`. Compile-time `Send + Sync` assertion. |

## Project Structure

### Documentation (this feature)

```text
specs/013-adapter-openai/
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
├── lib.rs               # Crate root -- re-exports OpenAiStreamFn
├── openai.rs            # OpenAiStreamFn, SSE stream parsing, tool call delta processing
├── openai_compat.rs     # OaiChatRequest, OaiChunk, OaiConverter, shared request/response types
├── base.rs              # AdapterBase (shared HTTP client)
├── classify.rs          # HttpErrorKind, classify_http_status (shared infra)
├── convert.rs           # MessageConverter trait, convert_messages (shared infra)
├── finalize.rs          # StreamFinalize trait, finalize_blocks (shared infra)
└── sse.rs               # SseStreamParser, sse_data_lines (shared infra)

adapters/tests/
└── openai_live.rs       # Live integration tests (#[ignore])
```

**Structure Decision**: Single file (`openai.rs`) in the existing adapters crate. The OpenAI adapter uses the shared `MessageConverter` trait via `OaiConverter` (defined in `openai_compat.rs`), plus shared infrastructure from `base.rs`, `convert.rs`, `finalize.rs`, and `sse.rs`. Unlike the Anthropic adapter, OpenAI uses the standard message conversion path -- no bespoke conversion needed. No new crates or modules required.

## Complexity Tracking

No constitution violations. All code fits within the existing `swink-agent-adapters` crate boundary. The OpenAI adapter reuses more shared infrastructure than Anthropic (shared `MessageConverter`, shared `sse_data_lines()`, shared `OaiChunk` types), resulting in a simpler implementation.
