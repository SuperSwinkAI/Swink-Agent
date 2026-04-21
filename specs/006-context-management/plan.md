# Implementation Plan: Context Management

**Branch**: `006-context-management` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/006-context-management/spec.md`

## Summary

Context management handles conversation history pruning, transformation, caching, and preparation for LLM providers. The sliding window algorithm preserves anchor (first N) and tail (most recent) messages while removing the middle to fit a token budget, keeping tool-result pairs together even if this exceeds the budget. Pluggable synchronous and asynchronous transformation hooks run before each LLM call, receiving an overflow signal on retry. A message conversion pipeline filters custom messages and maps agent messages to provider format. Versioned context snapshots capture dropped messages for debugging and RAG-style recall.

**New in this update (2026-03-31)**: Three additions to the context management system:
1. **Explicit Context Caching** — `CacheConfig` with TTL, min_tokens threshold, and cache_intervals controls provider-side caching. `CacheHint` annotations on messages let adapters translate to provider-specific format (Anthropic `cache_control`, Google `CachedContent`).
2. **Static/Dynamic Prompt Split** — `static_system_prompt` (cached across turns) and `dynamic_system_prompt` (closure, regenerated per turn) replace the single system prompt when caching is active.
3. **Context Overflow Predicate** — `is_context_overflow(messages, model, counter?)` estimates whether context exceeds the model's window before sending the request.

Existing implementation is in `src/context.rs`, `src/context_transformer.rs`, `src/async_context_transformer.rs`, `src/context_version.rs`, and `src/convert.rs`. New code will be added to `src/context.rs` (overflow predicate), `src/context_cache.rs` (caching types and logic), and `src/agent_options.rs` (new fields).

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: tokio (async runtime), serde_json (Value for tool arguments/extensions), tracing
**Storage**: N/A (in-memory `InMemoryVersionStore`; pluggable `ContextVersionStore` trait for persistence)
**Testing**: `cargo test --workspace` -- unit tests inline in each source module
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Minimize allocations on hot paths; chars/4 heuristic avoids tokenizer dependency; caching avoids re-tokenizing stable prefixes
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific dependencies in core; synchronous transform by default; caching abstraction must be provider-agnostic
**Scale/Scope**: Context pipeline for single-agent conversations -- called once per turn. Caching amortizes cost across multiple turns.

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
├── context.rs                  # Sliding window algorithm, token estimation, CompactionResult, is_context_overflow()
├── context_cache.rs            # CacheConfig, CacheHint, CacheState, cache lifecycle logic (NEW)
├── context_transformer.rs      # ContextTransformer trait, CompactionReport, SlidingWindowTransformer
├── async_context_transformer.rs # AsyncContextTransformer trait (async variant)
├── context_version.rs          # ContextVersion, ContextVersionStore, InMemoryVersionStore, VersioningTransformer
├── convert.rs                  # MessageConverter trait, convert_messages(), ToolSchema, extract_tool_schemas()
├── agent_options.rs            # AgentOptions with new static/dynamic prompt fields and CacheConfig
└── lib.rs                      # Re-exports all public types
```

**Structure Decision**: Context management lives in the core `swink-agent` crate. The new `context_cache.rs` file owns caching types and lifecycle logic (one concern per file). The overflow predicate goes in `context.rs` alongside token estimation since it's a direct consumer of `TokenCounter`. Static/dynamic prompt configuration goes in `agent_options.rs` as builder fields.

## Constitution Check (Re-checked 2026-03-31)

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | All context management in core `swink-agent` crate. New types (`CacheConfig`, `CacheHint`, `CacheState`) in core. No provider-specific code. |
| II. Test-Driven Development | PASS | Comprehensive unit tests in each module. New user stories include TDD phases (tests before impl). |
| III. Efficiency & Performance | PASS | chars/4 heuristic, O(n) sliding window. `CacheConfig` optional — zero overhead when absent. `is_context_overflow` is O(n) single pass. |
| IV. Leverage the Ecosystem | PASS | Uses serde_json, `std::time::Duration` for TTL. No new external dependencies. |
| V. Provider Agnosticism | PASS | `MessageConverter` + `CacheHint` are provider-agnostic. Adapters translate to Anthropic/Google/etc format. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Poisoned mutex recovery. `CacheState` uses `Mutex`. Dynamic prompt closure is `Send + Sync`. |

## Complexity Tracking

No constitution violations. No complexity justifications needed.
