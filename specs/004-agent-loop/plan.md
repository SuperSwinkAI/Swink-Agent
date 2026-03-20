# Implementation Plan: Agent Loop

**Branch**: `004-agent-loop` | **Date**: 2026-03-20 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-agent-loop/spec.md`

## Summary

Implement the core execution engine: the nested inner/outer loop that
orchestrates LLM calls, concurrent tool dispatch, steering interrupts,
follow-up continuation, retry integration, context overflow recovery,
and max tokens recovery. The loop is stateless — all state is passed
via `AgentLoopConfig` and `AgentContext`. It returns an async stream of
`AgentEvent` values.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: tokio (spawn, select!, CancellationToken), futures (Stream), serde_json (tool args)
**Storage**: N/A (stateless loop)
**Testing**: `cargo test --workspace` with mock StreamFn and mock tools
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library (core crate `swink-agent`)
**Performance Goals**: Tool calls execute concurrently via `tokio::spawn`. Minimal allocations in the hot loop path.
**Constraints**: Zero unsafe code. Zero clippy warnings. All async, cooperative cancellation only.
**Scale/Scope**: ~4 source files in `src/loop_/`, ~1200 lines total

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

**Structure Decision**: The loop module uses the trailing-underscore
convention (`loop_/`) since `loop` is a reserved word. Four files split
by concern: entry points, turn execution, streaming, and tool dispatch.

## Complexity Tracking

> No Constitution Check violations. Table intentionally left empty.
