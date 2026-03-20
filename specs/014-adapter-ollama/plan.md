# Implementation Plan: Adapter: Ollama

**Branch**: `014-adapter-ollama` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/014-adapter-ollama/spec.md`

## Summary

Implement `OllamaStreamFn`, a `StreamFn` adapter for Ollama's `/api/chat` endpoint. The adapter streams text and tool-call content blocks via newline-delimited JSON (NDJSON) -- not SSE -- with `done: true` as the stream termination sentinel. It converts agent messages to Ollama's message format using the shared `MessageConverter` trait, classifies HTTP and connection errors for retry, and handles missing/unexpected fields leniently via `#[serde(default)]`. The adapter defaults to `localhost:11434` when no base URL is provided. All code lives in `adapters/src/ollama.rs` within the `swink-agent-adapters` crate.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid`
**Storage**: N/A
**Testing**: `cargo test -p swink-agent-adapters`; unit tests in module, live integration tests (`#[ignore]`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Incremental NDJSON event emission (no buffering); zero-copy UTF-8 conversion via `std::str::from_utf8`
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core; `OllamaStreamFn` must be `Send + Sync`
**Scale/Scope**: Single adapter module (~590 lines)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All code lives in the `swink-agent-adapters` crate. `OllamaStreamFn` is a public type re-exported from `lib.rs`. No service, no daemon. |
| II | Test-Driven Development | PASS | Unit tests for NDJSON parsing, tool call event emission, and done-reason mapping. Live integration tests (`#[ignore]`) validate end-to-end streaming against a running Ollama instance. |
| III | Efficiency & Performance | PASS | NDJSON parsing is incremental via `ndjson_lines()`. Events emitted as each line arrives, never buffered. Zero-copy UTF-8 conversion attempted before fallback. |
| IV | Leverage the Ecosystem | PASS | Uses `reqwest` for HTTP, `futures` for stream combinators, `serde_json` for JSON parsing, `uuid` for tool call IDs, `tokio-util` for `CancellationToken`. Reuses shared `MessageConverter`, `StreamFinalize`, and `extract_tool_schemas`. |
| V | Provider Agnosticism | PASS | Core crate has no knowledge of Ollama. `OllamaStreamFn` implements `StreamFn` -- the sole provider boundary. Adding the Ollama adapter required zero changes to core. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. HTTP errors and connection failures produce error events, never panics. `StreamState` tracks open blocks for clean finalization on cancellation or unexpected stream end. Missing fields handled with `#[serde(default)]`. Compile-time `Send + Sync` assertion. |

## Project Structure

### Documentation (this feature)

```text
specs/014-adapter-ollama/
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
├── lib.rs               # Crate root -- re-exports OllamaStreamFn
├── ollama.rs            # OllamaStreamFn, NDJSON stream parsing, tool call processing
├── convert.rs           # MessageConverter trait, convert_messages (shared infra)
├── finalize.rs          # StreamFinalize trait, finalize_blocks (shared infra)
└── classify.rs          # HttpErrorKind, classify_http_status (shared infra)

adapters/tests/
└── ollama_live.rs       # Live integration tests (#[ignore])
```

**Structure Decision**: Single file (`ollama.rs`) in the existing adapters crate. The Ollama adapter implements its own NDJSON line parser (`ndjson_lines()`) instead of using the shared SSE parser, since Ollama uses a fundamentally different streaming protocol. It uses the shared `MessageConverter` trait via `OllamaConverter`, the shared `StreamFinalize` trait for block cleanup, and `extract_tool_schemas` for tool definition conversion. No new crates or modules required.

## Complexity Tracking

No constitution violations. All code fits within the existing `swink-agent-adapters` crate boundary. The Ollama adapter's primary distinguishing feature is the custom NDJSON line parser, which is necessary because Ollama does not use SSE. This is inherent complexity, not accidental -- there is no simpler alternative that would correctly parse the protocol.
