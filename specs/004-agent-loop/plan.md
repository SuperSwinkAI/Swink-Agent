# Implementation Plan: Agent Loop

**Branch**: `004-agent-loop` | **Date**: 2026-03-20 | **Updated**: 2026-03-31 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-agent-loop/spec.md`

## Summary

Implement the core execution engine: the nested inner/outer loop that
orchestrates LLM calls, concurrent tool dispatch, steering interrupts,
follow-up continuation, retry integration, emergency context overflow
recovery (in-place compaction + retry), and max tokens recovery. The loop is stateless — all state is passed
via `AgentLoopConfig` and `AgentContext`. It returns an async stream of
`AgentEvent` values.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: tokio (spawn, select!), tokio-util (CancellationToken), futures (Stream), serde_json (tool args)
**Storage**: N/A (stateless loop)
**Testing**: `cargo test --workspace` with mock StreamFn and mock tools
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library (core crate `swink-agent`)
**Performance Goals**: Tool calls execute concurrently via `tokio::spawn`. Minimal allocations in the hot loop path.
**Constraints**: Zero unsafe code. Zero clippy warnings. All async, cooperative cancellation only.
**Scale/Scope**: ~4 source files in `src/loop_/`, ~1200 lines total

**Layout update [2026-07-06]**: The implementation has since grown to 12 files across `src/loop_/` and `src/loop_/tool_dispatch/` (~6,500 lines total, verified via `find src/loop_ -name '*.rs' | xargs wc -l`). Tool dispatch was split out of a single file into a three-phase `tool_dispatch/` submodule (`preprocess.rs` → `execute.rs` → `collect.rs`, plus `shared.rs`). See the updated Source Code tree below.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | Loop is internal to the core library crate. No service dependencies. |
| II. Test-Driven Development | ✅ Pass | Tests use mock StreamFn and mock tools to verify event sequences, concurrency, steering, retry, and error recovery. |
| III. Efficiency & Performance | ✅ Pass | Concurrent tool execution via `tokio::spawn`. Sequential turns minimize coordination overhead. |
| IV. Leverage the Ecosystem | ✅ Pass | Uses tokio for async runtime, futures for Stream trait, tokio-util for CancellationToken. |
| V. Provider Agnosticism | ✅ Pass | Loop calls StreamFn trait — no provider-specific code. |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]`. Cooperative cancellation via CancellationToken. Errors stay in message log. |

No violations.

## Project Structure

### Documentation (this feature)

```text
specs/004-agent-loop/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Loop entry points and config API
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
src/
├── lib.rs               # Public re-exports (updated)
├── loop_/
│   ├── mod.rs           # agent_loop, agent_loop_continue entry points, AgentLoopConfig
│   ├── turn.rs          # Single turn execution: transform → convert → stream → tool check
│   ├── stream.rs        # StreamFn invocation with retry, cancellation, error handling
│   └── tool_dispatch.rs # Concurrent tool execution, steering detection, cancellation
├── emit.rs              # Event emission helper
└── event_forwarder.rs   # Event forwarding utilities

tests/
├── loop_single_turn.rs      # US1: single-turn event sequence
├── loop_tool_execution.rs   # US2: multi-turn tool cycles
├── loop_steering.rs         # US3: steering interrupts
├── loop_follow_up.rs        # US4: follow-up continuation
├── loop_retry.rs            # US5: retry integration
├── loop_overflow.rs         # US6: context overflow recovery
├── loop_max_tokens.rs       # US7: max tokens recovery
└── loop_cancellation.rs     # Cancellation via token
```

**Layout update [2026-07-06]**: The tree above reflects the original plan. Actual current layout: `src/loop_/` holds `mod.rs`, `config.rs`, `event.rs`, `overflow.rs`, `stream.rs`, `turn.rs`, `types.rs`, plus a `tool_dispatch/` submodule (`mod.rs`, `preprocess.rs`, `execute.rs`, `collect.rs`, `shared.rs`) — 12 files, ~6,500 lines total, verified via `ls src/loop_/ src/loop_/tool_dispatch/` and `find src/loop_ -name '*.rs' | xargs wc -l`. The per-user-story test files above were never created as separate files; they were consolidated into `tests/agent_loop.rs` (single-turn, tool execution, steering, follow-up, cancellation), `tests/retry.rs` (retry integration), `tests/fallback.rs` (model fallback), and `tests/loop_overflow.rs` (context overflow recovery, matches the plan as-is).

**Structure Decision**: The loop module uses the trailing-underscore
convention (`loop_/`) since `loop` is a reserved word. Four files split
by concern: entry points, turn execution, streaming, and tool dispatch.

## Complexity Tracking

> No Constitution Check violations. Table intentionally left empty.
