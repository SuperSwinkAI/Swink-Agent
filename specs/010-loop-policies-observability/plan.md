# Implementation Plan: Loop Policies & Observability

**Branch**: `010-loop-policies-observability` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/010-loop-policies-observability/spec.md`

## Summary

Cross-cutting infrastructure for agent loop governance, observability, and resumability. Provides composable loop policies (`LoopPolicy` trait with `MaxTurnsPolicy`, `CostCapPolicy`, `ComposedPolicy`, and closure blanket impl) for post-turn termination decisions. Stream middleware (`StreamMiddleware`) wraps `StreamFn` using the decorator pattern for event interception, transformation, and filtering. Structured emission (`Emission`) carries named payloads for enriched events. An async `MetricsCollector` trait receives `TurnMetrics` snapshots (LLM call duration, per-tool timing, token usage, cost). Async `PostTurnHook` callbacks return `PostTurnAction` (Continue/Stop/InjectMessages) to influence loop control flow. `BudgetGuard` provides pre-call cost/token gating for real-time budget enforcement. `Checkpoint` and `LoopCheckpoint` capture serializable snapshots of agent state, with `CheckpointStore` as an async trait for persistence.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: tokio (async runtime), tokio-util (CancellationToken), futures (Stream, StreamExt), serde / serde_json (serialization), tracing (diagnostics)
**Storage**: N/A (in-memory by default; `CheckpointStore` trait abstracts persistence)
**Testing**: `cargo test --workspace` — unit tests in each source module, integration tests in `tests/`
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Non-blocking policy checks (synchronous predicate); non-blocking budget guard (const fn check); zero-overhead when checkpointing is not configured
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific dependencies; no global mutable state (all shared state via `Arc<Mutex<>>`)
**Scale/Scope**: Governance and observability layer — composable primitives for loop control, metrics, and state persistence

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | All types are library structs/traits in the core crate. LoopPolicy, StreamMiddleware, MetricsCollector, PostTurnHook, BudgetGuard, Checkpoint, and CheckpointStore are independently usable API surfaces with no service/daemon coupling. |
| II. Test-Driven Development | PASS | Each module has comprehensive unit tests: loop_policy (7 tests), stream_middleware (4 tests), metrics (4 tests), post_turn_hook (5 tests), budget_guard (12 tests), checkpoint (10 tests). All test names are descriptive snake_case without `test_` prefix. |
| III. Efficiency & Performance | PASS | LoopPolicy::should_continue is synchronous (no async overhead at turn boundary). BudgetGuard::check is `const fn`. StreamMiddleware adds zero cost when not used. Checkpoint overhead is zero when not configured (opt-in). |
| IV. Leverage the Ecosystem | PASS | Uses futures Stream/StreamExt for middleware composition, serde for checkpoint serialization, tokio CancellationToken for budget guard cancellation. No custom reimplementations. |
| V. Provider Agnosticism | PASS | No provider-specific types. StreamMiddleware wraps the generic `StreamFn` trait. Policies operate on `PolicyContext` (turn index, usage, cost) — provider-independent data. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. BudgetExceeded is a proper error enum with Display. PostTurnHook panics are caught and the hook is skipped (per spec). Poisoned mutex recovery via `into_inner()` in CheckpointStore implementations. Compile-time Send+Sync assertions on all public types. |

## Project Structure

### Documentation (this feature)

```text
specs/010-loop-policies-observability/
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
├── loop_policy.rs       # LoopPolicy trait, PolicyContext, MaxTurnsPolicy, CostCapPolicy, ComposedPolicy
├── stream_middleware.rs  # StreamMiddleware (decorator wrapping StreamFn), MapStreamFn type alias
├── emit.rs              # Emission struct (structured event payloads)
├── metrics.rs           # MetricsCollector trait, TurnMetrics, ToolExecMetrics
├── post_turn_hook.rs    # PostTurnHook trait, PostTurnContext, PostTurnAction enum
├── budget_guard.rs      # BudgetGuard (pre-call check), BudgetExceeded error enum
├── checkpoint.rs        # Checkpoint, LoopCheckpoint, CheckpointStore trait
└── lib.rs               # Re-exports all public types
```

**Structure Decision**: All loop policy and observability types live in the core `swink-agent` crate, each in its own file (one concern per file). No new crate needed — these are core infrastructure primitives that depend on `Usage`, `Cost`, `AgentMessage`, `StreamFn`, and `AssistantMessageEvent` from the same crate.

## Complexity Tracking

No constitution violations. No complexity justifications needed.
