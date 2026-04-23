# Implementation Plan: Memory Crate

**Branch**: `021-memory-crate` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/021-memory-crate/spec.md`

## Summary

Implement the `swink-agent-memory` workspace crate providing session persistence, summarization-aware context compaction, rich session entry types, session versioning with migration, interrupt state persistence, and filtered session retrieval. The crate defines `SessionStore` (sync) and `AsyncSessionStore` traits with save/load/list/delete operations, a `JsonlSessionStore` concrete backend using JSONL (one message per line, metadata on line 1), and a `SummarizingCompactor` that produces a closure compatible with `Agent::with_transform_context()`. Summaries are pre-computed asynchronously after each turn and injected synchronously during context transformation.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `serde`, `serde_json`, `tokio` (fs), `chrono` (timestamps), `tracing` (warning on corrupted lines)
**Storage**: Local filesystem via JSONL files (one file per session)
**Testing**: `cargo test -p swink-agent-memory`; integration tests with real filesystem I/O preferred over mocks
**Target Platform**: Cross-platform library crate (Linux, macOS, Windows)
**Project Type**: Library crate (`swink-agent-memory`) within the `swink-agent` workspace
**Performance Goals**: Append-only writes for incremental saves; no full-file rewrite on message append
**Constraints**: No unsafe code; no dependency on core agent internals; `TransformContextFn` is synchronous
**Scale/Scope**: Single-writer assumption; no file locking; designed for local development workflows

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | Memory is its own workspace crate with a clean public API. No reverse dependency on core. |
| II | Test-Driven Development | PASS | Tests cover save/load round-trip, compaction budget enforcement, partial corruption recovery, async operations, edge cases (empty file, single message, unsafe IDs), rich entry type serialization (T058–T060), backward compatibility for old-format sessions (T059, T064), interrupt state save/load/clear/delete roundtrips (T072–T075), version migration (T067), optimistic concurrency conflict detection (T066), and filtered loading by count/timestamp/type (T081–T084). |
| III | Efficiency & Performance | PASS | Append-only writes avoid full-file rewrites. JSONL enables streaming reads. No unnecessary allocations on the compaction hot path. |
| IV | Leverage the Ecosystem | PASS | Uses `serde`/`serde_json` for serialization, `tokio::fs` for async I/O, `chrono` for timestamps. No hand-rolled JSON or async filesystem code. |
| V | Provider Agnosticism | PASS | Compactor accepts a summarization function (`async Fn(Vec<LlmMessage>) -> String`) — no embedded provider. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Corrupted lines produce warnings via `tracing`, not panics. Errors are `io::Result` — no panics on I/O failure. |

## Project Structure

### Documentation (this feature)

```text
specs/021-memory-crate/
├── plan.md              # This file
├── research.md          # Design decisions and trade-offs
├── data-model.md        # Entity definitions and relationships
├── quickstart.md        # Getting started guide
├── contracts/
│   └── public-api.md    # Public API surface contract
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
memory/
├── Cargo.toml
├── AGENTS.md
└── src/
    ├── lib.rs            # Re-exports public API
    ├── store.rs          # SessionStore trait (sync)
    ├── store_async.rs    # AsyncSessionStore trait
    ├── jsonl.rs          # JsonlSessionStore implementation
    ├── compaction.rs     # SummarizingCompactor
    ├── meta.rs           # SessionMeta struct (with version + sequence)
    ├── entry.rs          # SessionEntry enum (rich entry types)
    ├── interrupt.rs      # InterruptState, PendingToolCall
    ├── migrate.rs        # SessionMigrator trait
    ├── load_options.rs   # LoadOptions struct
    └── time.rs           # Timestamp utilities

memory/tests/
├── common/
│   └── mod.rs            # Shared test helpers
├── round_trip.rs         # Save/load round-trip tests
├── compaction.rs         # Compaction budget and summary tests
├── corruption.rs         # Partial recovery tests
└── async_store.rs        # Async operation tests
```

**Structure Decision**: The memory crate already exists at `memory/` as a workspace member. Source files follow the one-concern-per-file convention. The `lib.rs` re-exports all public types so consumers never reach into submodules.

## Complexity Tracking

No constitution violations. No complexity justifications required.
