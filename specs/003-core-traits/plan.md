# Implementation Plan: Core Traits

**Branch**: `003-core-traits` | **Date**: 2026-03-20 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-core-traits/spec.md`

## Summary

Define the three trait boundaries of the agent harness: `AgentTool` (tool
execution), `StreamFn` (LLM streaming), and `RetryStrategy` (retry logic).
Includes tool argument validation via JSON Schema, delta accumulation for
streaming, the default exponential backoff retry strategy, and all
supporting types (AgentToolResult, StreamOptions, AssistantMessageEvent,
AssistantMessageDelta).

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: serde_json (tool args), jsonschema (validation), tokio (async), tokio-util (CancellationToken), futures (Stream), rand (jitter)
**Storage**: N/A (traits and types only)
**Testing**: `cargo test --workspace` with mock implementations, proptest for retry delay properties
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library (core crate `swink-agent`)
**Performance Goals**: Minimal allocations in delta accumulation hot path
**Constraints**: All traits must be object-safe (dyn-compatible). Zero unsafe code. Zero clippy warnings.
**Scale/Scope**: ~4 source files in `src/`, ~800-1000 lines total

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | Traits are part of the core library. No service dependencies. |
| II. Test-Driven Development | ✅ Pass | Mock implementations test each trait. Property tests for retry delay correctness. |
| III. Efficiency & Performance | ✅ Pass | Delta accumulation avoids unnecessary copies. Partial JSON parsed once on ToolCallEnd. |
| IV. Leverage the Ecosystem | ✅ Pass | jsonschema for validation, rand for jitter — no hand-rolled alternatives. |
| V. Provider Agnosticism | ✅ Pass | StreamFn is the sole provider boundary. Trait-based, no provider types in core. |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]`. Strict event ordering enforcement. Out-of-order events produce errors. |

No violations.

## Project Structure

### Documentation (this feature)

```text
specs/003-core-traits/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Trait API contracts
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
src/
├── lib.rs               # Public re-exports (updated)
├── tool.rs              # AgentTool trait, AgentToolResult, validate_tool_arguments
├── stream.rs            # StreamFn trait, StreamOptions, AssistantMessageEvent, AssistantMessageDelta, accumulate_message
├── retry.rs             # RetryStrategy trait, DefaultRetryStrategy

tests/
├── tool.rs              # Tool validation: valid args, invalid args, missing fields, empty schema, empty args
├── stream.rs            # Accumulation: text, thinking, tool calls, interleaved, out-of-order errors, empty stream
├── retry.rs             # Default strategy: retryable vs non-retryable, delay growth, jitter range, max cap
```

**Structure Decision**: One file per trait concern. Tests as integration
tests verifying the public API surface.

## Complexity Tracking

> No Constitution Check violations. Table intentionally left empty.
