# Implementation Plan: Context Management

**Branch**: `006-context-management` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/006-context-management/spec.md`

## Summary

Context management handles conversation history pruning, transformation, and preparation for LLM providers. The sliding window algorithm preserves anchor (first N) and tail (most recent) messages while removing the middle to fit a token budget, keeping tool-result pairs together even if this exceeds the budget. Pluggable synchronous and asynchronous transformation hooks run before each LLM call, receiving an overflow signal on retry. A message conversion pipeline filters custom messages and maps agent messages to provider format. Versioned context snapshots capture dropped messages for debugging and RAG-style recall. This feature is already fully implemented in `src/context.rs`, `src/context_transformer.rs`, `src/async_context_transformer.rs`, `src/context_version.rs`, and `src/convert.rs`.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: tokio (async runtime), serde_json (Value for tool arguments/extensions), tracing
**Storage**: N/A (in-memory `InMemoryVersionStore`; pluggable `ContextVersionStore` trait for persistence)
**Testing**: `cargo test --workspace` -- unit tests inline in each source module
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Minimize allocations on hot paths; chars/4 heuristic avoids tokenizer dependency
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific dependencies in core; synchronous transform by default
**Scale/Scope**: Context pipeline for single-agent conversations -- called once per turn

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | All context management is in the core `swink-agent` crate with no service/daemon coupling. No new crates needed. |
| II. Test-Driven Development | PASS | Comprehensive unit tests in each module: sliding window edge cases, transformer trait/blanket-impl, async transformer, versioning capture, message conversion filtering. |
| III. Efficiency & Performance | PASS | Token estimation uses chars/4 heuristic (simple, no tokenizer dependency). Sliding window operates in O(n) with in-place mutation. Custom messages use 100-token flat cost. |
| IV. Leverage the Ecosystem | PASS | Uses serde_json for content block serialization. No custom reimplementations of existing crate functionality. |
| V. Provider Agnosticism | PASS | `MessageConverter` trait is provider-generic. `ConvertToLlmFn` / `convert_messages` work with any adapter. No provider-specific types in context pipeline. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Poisoned mutex recovery via `into_inner()`. Tool-result pairs preserved together (correctness > token count). |

## Project Structure

### Documentation (this feature)

```text
specs/006-context-management/
├── plan.md              # This file
├── research.md          # Design decisions and rationale
├── data-model.md        # Entity definitions
├── quickstart.md        # Build/test instructions and usage examples
├── contracts/
│   └── public-api.md    # Public API contract
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
src/
├── context.rs                  # Sliding window algorithm, token estimation, CompactionResult
├── context_transformer.rs      # ContextTransformer trait, CompactionReport, SlidingWindowTransformer
├── async_context_transformer.rs # AsyncContextTransformer trait (async variant)
├── context_version.rs          # ContextVersion, ContextVersionStore, InMemoryVersionStore, VersioningTransformer
├── convert.rs                  # MessageConverter trait, convert_messages(), ToolSchema, extract_tool_schemas()
└── lib.rs                      # Re-exports all public types
```

**Structure Decision**: All context management lives in the core `swink-agent` crate across five source files, each owning a single concern. No new crates or modules needed. Public API re-exported through `lib.rs`.

## Complexity Tracking

No constitution violations. No complexity justifications needed.
