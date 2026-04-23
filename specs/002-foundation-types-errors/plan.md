# Implementation Plan: Foundation Types & Errors

**Branch**: `002-foundation-types-errors` | **Date**: 2026-03-20 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-foundation-types-errors/spec.md`

## Summary

Implement the foundational data types and error taxonomy that every other
module depends on. This includes content blocks (text, thinking, tool call,
image), message types (user, assistant, tool result), the AgentMessage
wrapper with custom message support, usage/cost tracking, stop reasons,
model specification, agent result, agent context, and the complete
AgentError enum. All types must be Send + Sync, serializable, and produce
no business logic — only data definitions, constructors, trait impls, and
aggregation methods.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: serde, serde_json, thiserror, uuid, schemars (all workspace deps)
**Storage**: N/A (in-memory types only)
**Testing**: `cargo test --workspace`
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library (core crate `swink-agent`)
**Performance Goals**: Zero-copy where possible; minimal allocations in type constructors
**Constraints**: All public types Send + Sync. Zero unsafe code. Zero clippy warnings.
**Scale/Scope**: ~8 source files in `src/`, ~500-800 lines total

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | Types are part of the core library crate. No service dependencies. |
| II. Test-Driven Development | ✅ Pass | Tests written first for each type: construction, serialization round-trip, aggregation arithmetic, error display. |
| III. Efficiency & Performance | ✅ Pass | Types use owned strings. No unnecessary allocations. |
| IV. Leverage the Ecosystem | ✅ Pass | Uses thiserror for error derive, serde for serialization, uuid for IDs — no hand-rolled alternatives. |
| V. Provider Agnosticism | ✅ Pass | Types carry provider identifiers as strings, not provider-specific types. No provider SDK dependency. |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]`. All types derive or implement Send + Sync. Error variants implement `std::error::Error`. |

No violations. Complexity Tracking not needed.

## Project Structure

### Documentation (this feature)

```text
specs/002-foundation-types-errors/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Public type API contracts
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
src/
├── lib.rs               # Public re-exports (updated)
├── types.rs             # LlmMessage, AgentMessage, ContentBlock, StopReason, Usage, Cost
├── error.rs             # AgentError enum, DowncastError
├── context.rs           # AgentContext
├── model_spec.rs        # ModelSpec, ThinkingLevel
└── result.rs            # AgentResult

tests/
├── common/
│   └── mod.rs           # Shared test helpers
├── types_test.rs        # Message construction, content blocks, serialization
├── error_test.rs        # Error variant construction, display, trait impl
├── usage_test.rs        # Usage/Cost aggregation arithmetic
└── custom_message_test.rs # Custom message wrap, store, downcast
```

**Structure Decision**: All types live in the core crate (`swink-agent`)
under `src/`. Each concern gets its own file per the one-concern-per-file
convention. Tests live in `tests/` as integration tests to verify the
public API surface.

## Complexity Tracking

> No Constitution Check violations. Table intentionally left empty.
