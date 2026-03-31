# Implementation Plan: Adapter Shared Infrastructure

**Branch**: `011-adapter-shared-infra` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/011-adapter-shared-infra/spec.md`

## Summary

Extract and consolidate the shared infrastructure that all LLM provider adapters depend on: a `MessageConverter` trait for converting agent messages to provider-specific formats, an `HttpErrorClassifier` for mapping HTTP status codes to agent error types, an `SseStreamParser` for consuming SSE byte streams, a catalog-driven `RemotePresets` system for constructing configured remote connections from preset keys, a `CacheStrategy` enum for provider-agnostic prompt caching configuration, a `ProxyStreamFn` for raw SSE byte relay in gateway deployments, and an `OnRawPayload` callback for observing raw provider data before event parsing.

All concerns live in the `swink-agent-adapters` crate (or core for types that flow through `StreamOptions`) with the conversion trait defined in core (`swink-agent`) and re-exported through adapters.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde_json`, `thiserror`, `tokio`
**Storage**: N/A
**Testing**: `cargo test --workspace`; unit tests per module, integration live tests (`#[ignore]`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Single-allocation buffering in SSE parser (one `String` buffer, no per-line heap allocation beyond the output `Vec`); minimal allocations on the streaming hot path
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types in core
**Scale/Scope**: 7 adapters use SSE, 9 adapters use `MessageConverter`, all remote adapters use `RemotePresets`

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All shared infra lives in `swink-agent-adapters` (or core for `MessageConverter`). No service, no daemon. Independently compilable and testable. |
| II | Test-Driven Development | PASS | Each module (`classify`, `sse`, `remote_presets`) already has unit tests. `convert` re-exports core's tested trait. New tests for CacheStrategy translation, ProxyStreamFn raw byte delivery, and OnRawPayload callback invocation/panic isolation. |
| III | Efficiency & Performance | PASS | `SseStreamParser` buffers bytes and drains lines incrementally; `classify_http_status` is `const fn`. No unnecessary allocations. |
| IV | Leverage the Ecosystem | PASS | Uses `reqwest` for HTTP, `futures` for streams, `bytes` for zero-copy chunks, `thiserror` for error derive. No reinvented wheels. |
| V | Provider Agnosticism | PASS | `MessageConverter` defines a generic contract — each adapter implements it. Core holds no provider types. `HttpErrorClassifier` is provider-neutral with an override mechanism. `CacheStrategy` is defined in core as a provider-agnostic enum; adapters translate to provider-specific formats. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. Errors produce proper error types, never panics. Poisoned-mutex recovery via `into_inner()`. `OnRawPayload` panics caught via `catch_unwind`; stream continues uninterrupted. |

## Project Structure

### Documentation (this feature)

```text
specs/011-adapter-shared-infra/
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
├── lib.rs               # Crate root — #[forbid(unsafe_code)], module declarations, re-exports
├── convert.rs           # Re-exports MessageConverter trait + convert_messages from core
├── classify.rs          # HttpErrorKind enum, classify_http_status, classify_with_overrides
├── sse.rs               # SseLine enum, SseStreamParser, sse_data_lines stream combinator
├── remote_presets.rs    # RemotePresetKey, remote_preset_keys module, build_remote_connection
├── base.rs              # Shared base HTTP client utilities
├── finalize.rs          # Stream finalization helpers
├── anthropic.rs         # (downstream adapter — consumes shared infra)
├── openai.rs            # (downstream adapter — consumes shared infra)
├── ollama.rs            # (downstream adapter — consumes shared infra)
├── google.rs            # (downstream adapter — consumes shared infra)
├── azure.rs             # (downstream adapter — consumes shared infra)
├── xai.rs               # (downstream adapter — consumes shared infra)
├── mistral.rs           # (downstream adapter — consumes shared infra)
├── bedrock.rs           # (downstream adapter — consumes shared infra)
├── openai_compat.rs     # OpenAI-compatible shared logic
└── proxy.rs             # ProxyStreamFn
```

**Structure Decision**: All shared infrastructure modules (`convert`, `classify`, `sse`, `remote_presets`) are top-level modules within the existing `adapters/src/` directory. The `MessageConverter` trait is defined in `swink-agent` core (`src/convert.rs`) and re-exported by `adapters/src/convert.rs`. No new crates are needed.

## Complexity Tracking

| Item | Principle | Decision | Justification |
|------|-----------|----------|---------------|
| SSE Parser | IV (Leverage Ecosystem) | Custom ~100-line `SseStreamParser` instead of wrapping `eventsource-stream` | `eventsource-stream` does not expose event-type labels needed by Anthropic's streaming state machine (event types like `content_block_start`, `content_block_delta`, etc. drive adapter state transitions). Wrapping it would require re-parsing the raw bytes anyway to extract event types, defeating the purpose. This falls below the 80% threshold for the "wrap it" escape hatch — the crate handles less than 80% of what adapters need. |

No other constitution violations. All modules fit within existing crate boundaries.
