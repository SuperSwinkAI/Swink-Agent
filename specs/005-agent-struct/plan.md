# Implementation Plan: Agent Struct & Public API

**Branch**: `005-agent-struct` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/005-agent-struct/spec.md`

## Summary

The Agent struct is the stateful public API wrapper over the agent loop. It owns conversation history, manages steering/follow-up queues, enforces single-invocation concurrency, provides three invocation modes (streaming, async, sync), implements structured output with schema validation, and fans events to subscribers with panic isolation. This feature is already fully implemented in `src/agent.rs`, `src/agent_options.rs`, and `src/agent_subscriptions.rs`.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: tokio, tokio-util (CancellationToken), futures (Stream), serde_json (Value), tracing
**Storage**: N/A (in-memory state; optional CheckpointStore trait for persistence)
**Testing**: `cargo test --workspace` — unit tests in `tests/agent.rs`, `tests/agent_structured.rs`, `tests/agent_steering.rs`, `tests/agent_continuation.rs`, `tests/handle.rs`
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Minimize allocations on hot paths; concurrent tool execution via `tokio::spawn`
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific dependencies in core; single-invocation concurrency enforced at runtime
**Scale/Scope**: Single-agent API surface — the primary way applications interact with the agent loop

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | Agent is a library struct with no service/daemon coupling |
| II. Test-Driven Development | PASS | Comprehensive test coverage in `tests/agent*.rs`, including model swap (T079–T082) and wait_for_idle (T086–T089) integration tests |
| III. Efficiency & Performance | PASS | `Arc<Mutex<>>` for shared queues; concurrent tool dispatch via tokio::spawn |
| IV. Leverage the Ecosystem | PASS | Uses tokio, futures, serde_json — no custom reimplementations |
| V. Provider Agnosticism | PASS | All LLM interaction via `StreamFn` trait; Agent holds no provider types |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`; panic-isolated subscribers; poisoned-mutex recovery |

## Project Structure

### Documentation (this feature)

```text
specs/005-agent-struct/
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
├── agent.rs               # Agent struct, AgentState, SteeringMode, FollowUpMode, invocation methods
├── agent_options.rs       # AgentOptions builder with with_*() chain
├── agent_subscriptions.rs # ListenerRegistry, SubscriptionId, panic-isolated dispatch
├── handle.rs              # AgentHandle for spawned background tasks
├── error.rs               # AgentError variants (AlreadyRunning, NoMessages, InvalidContinue, etc.)
├── loop_/                 # Inner agent loop (owned by spec 003)
├── stream.rs              # StreamFn trait and accumulation
├── tool.rs                # AgentTool trait, AgentToolResult
├── types.rs               # AgentMessage, AgentResult, ModelSpec, Usage, Cost, StopReason
└── lib.rs                 # Re-exports all public types

tests/
├── agent.rs               # Core agent lifecycle tests
├── agent_continuation.rs  # Continue invocation tests
├── agent_steering.rs      # Steering/follow-up queue tests
├── agent_structured.rs    # Structured output tests
├── agent_models.rs        # Model cycling tests
├── handle.rs              # AgentHandle spawn/cancel tests
├── public_api.rs          # Public API surface tests
└── common/
    └── mod.rs             # MockStreamFn, MockTool, test helpers
```

**Structure Decision**: Single library crate (`swink-agent`) with the Agent struct as the primary API surface. All public types re-exported through `lib.rs`. Test files mirror the source module structure.

## Complexity Tracking

No constitution violations. No complexity justifications needed.
