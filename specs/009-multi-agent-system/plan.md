# Implementation Plan: Multi-Agent System

**Branch**: `009-multi-agent-system` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/009-multi-agent-system/spec.md`

## Summary

A multi-agent system for composing, coordinating, and supervising multiple agents within the swink-agent core crate. Provides four primitives: a thread-safe `AgentRegistry` for named agent lookup, an `AgentMailbox` and `send_to` function for asynchronous inter-agent messaging, a `SubAgent` tool wrapper that bridges the tool system with agent composition (including cancellation propagation), and an `AgentOrchestrator` for lifecycle management with supervisor policies (Restart/Escalate/Stop). All primitives are independent and composable — the orchestrator is optional convenience over registry + messaging.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: tokio (spawn, mpsc, oneshot, select!), tokio-util (CancellationToken), serde_json (Value), tracing (info, warn)
**Storage**: N/A (in-memory state only)
**Testing**: `cargo test --workspace` — unit tests in source modules (`orchestrator.rs`), integration tests in `tests/`
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Non-blocking mailbox send (mutex-guarded Vec push); concurrent agent execution via `tokio::spawn`
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific dependencies; no global mutable state (all shared state via `Arc<Mutex<>>` or `Arc<RwLock<>>`)
**Scale/Scope**: Multi-agent coordination layer — composable primitives for agent-to-agent interaction

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | All types are library structs in the core crate; no service/daemon coupling. Registry, mailbox, sub-agent, and orchestrator are independently usable API surfaces. |
| II. Test-Driven Development | PASS | Unit tests in `orchestrator.rs` (13 tests covering hierarchy, supervisor, spawn errors). Registry, mailbox, and sub-agent testable with existing `MockStreamFn`/`MockTool` helpers. |
| III. Efficiency & Performance | PASS | `RwLock` for registry (concurrent reads), `Mutex<Vec>` for mailbox (non-blocking push), `tokio::spawn` for orchestrated agents. No unnecessary allocations. |
| IV. Leverage the Ecosystem | PASS | Uses tokio channels (mpsc, oneshot), CancellationToken from tokio-util, tracing for diagnostics. No custom reimplementations. |
| V. Provider Agnosticism | PASS | No provider-specific types. SubAgent uses `AgentOptions` factory with `StreamFn` — works with any provider. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`; poisoned mutex recovery via `into_inner()`; panic-safe subscriber dispatch inherited from Agent. Supervisor policy prevents unhandled agent failures. |

## Project Structure

### Documentation (this feature)

```text
specs/009-multi-agent-system/
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
├── registry.rs          # AgentId, AgentRef, AgentRegistry (RwLock-based named lookup)
├── messaging.rs         # AgentMailbox (Mutex<Vec> inbox), send_to() free function
├── sub_agent.rs         # SubAgent tool wrapper (AgentTool impl, cancellation propagation)
├── orchestrator.rs      # AgentOrchestrator, OrchestratedHandle, SupervisorPolicy, AgentRequest
├── handle.rs            # AgentStatus enum (shared with orchestrator)
├── agent.rs             # Agent struct (owns AgentId, provides steer() for messaging)
├── error.rs             # AgentError variants (Plugin used for messaging/orchestrator errors)
└── lib.rs               # Re-exports all public types

tests/
├── common/
│   └── mod.rs           # MockStreamFn, MockTool, test helpers
└── ...                  # Integration tests for multi-agent scenarios
```

**Structure Decision**: All multi-agent types live in the core `swink-agent` crate, each in its own file (one concern per file). No new crate needed — these are core coordination primitives that depend on `Agent`, `AgentTool`, and `AgentMessage` from the same crate.

## Complexity Tracking

No constitution violations. No complexity justifications needed.
