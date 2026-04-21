# Implementation Plan: TransferToAgent Tool & Handoff Safety

**Branch**: `040-agent-transfer-handoff` | **Date**: 2026-04-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/040-agent-transfer-handoff/spec.md`

## Summary

Add agent-level handoff primitives to the core crate: a `TransferToAgentTool` that signals the agent loop to transfer conversation to another agent, a `TransferSignal` data type carrying handoff context, a `TransferChain` for circular detection and depth limiting, and the loop integration that recognizes transfer signals in tool results and terminates turns accordingly. Extends `StopReason` with a `Transfer` variant and `AgentResult`/`AgentToolResult` with transfer signal fields.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)  
**Primary Dependencies**: `swink-agent` core types (`AgentTool`, `AgentRegistry`, `StopReason`, `AgentToolResult`, `AgentResult`), `serde`/`serde_json` (serialization), `schemars` (tool schema), `tokio-util` (CancellationToken)  
**Storage**: N/A (in-memory only)  
**Testing**: `cargo test -p swink-agent`  
**Target Platform**: Cross-platform library  
**Project Type**: Library crate (core workspace member)  
**Performance Goals**: Zero overhead for agents without transfer tool. Transfer detection is O(n) scan of tool results per turn.  
**Constraints**: Must preserve `Copy` on `StopReason`. Must be backward-compatible with existing `AgentToolResult` serialization.  
**Scale/Scope**: ~2 source files modified (types, loop), 1 new file (transfer.rs), ~500 LOC estimated

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | Feature lives in the core crate, exposed as public API. No new crate needed — transfer is a core agent primitive. |
| II. Test-Driven Development | PASS | Tests written before implementation for all new types and loop integration. |
| III. Efficiency & Performance | PASS | Zero overhead when transfer tool not used (Option fields default to None, no scan needed when no tool results have signals). |
| IV. Leverage the Ecosystem | PASS | Uses existing workspace deps only (serde, schemars, tokio-util). No new external deps. |
| V. Provider Agnosticism | PASS | Transfer is provider-agnostic — purely a tool/loop mechanism. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. TransferChain prevents circular loops. Feature-gated under `transfer`. |

**Architectural Constraints**:
- **Crate count**: No new crates. Feature lives in core (`swink-agent`).
- **MSRV**: latest stable, edition 2024.
- **StopReason**: `Transfer` is a unit variant (no data) to preserve `Copy`. Transfer data lives in `AgentResult.transfer_signal`.

## Project Structure

### Documentation (this feature)

```text
specs/040-agent-transfer-handoff/
├── spec.md              # Feature specification
├── plan.md              # This file
├── research.md          # Phase 0: design decisions
├── data-model.md        # Phase 1: entity definitions
├── quickstart.md        # Phase 1: usage examples
├── contracts/
│   └── public-api.md    # Phase 1: API surface contract
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
src/
├── transfer.rs          # NEW: TransferToAgentTool, TransferSignal, TransferChain, TransferError
├── types/mod.rs         # MODIFIED: StopReason::Transfer variant, AgentResult.transfer_signal
├── tool.rs              # MODIFIED: AgentToolResult.transfer_signal field + transfer() constructor
├── loop_/turn.rs        # MODIFIED: Transfer signal detection after tool execution
└── lib.rs               # MODIFIED: Feature-gated re-export of transfer module
```

**Structure Decision**: Single new file `src/transfer.rs` for all transfer types. Core type modifications are minimal additions to existing files. The transfer module is feature-gated in `lib.rs`.

## Complexity Tracking

> No violations. All changes fit within existing crate boundaries.

## Key Design Decisions

### 1. StopReason::Transfer as unit variant

`StopReason` derives `Copy`. Adding `Transfer(TransferSignal)` would break `Copy` (TransferSignal contains `Vec`). Instead, `Transfer` is a unit variant and the signal data lives in `AgentResult.transfer_signal: Option<TransferSignal>` — mirroring the `Error` + `error` pattern. See [research.md](research.md) R1.

### 2. Tool returns partial signal, loop enriches

The tool's `execute()` returns a `TransferSignal` with target, reason, and summary. The loop detects this in tool results and enriches the signal with `conversation_history` from `LoopState.context_messages` before surfacing it in `AgentResult`. See [research.md](research.md) R3.

### 3. Feature gate scope

`StopReason::Transfer`, `AgentResult.transfer_signal`, and `AgentToolResult.transfer_signal` are always compiled (not gated) to avoid match-arm and field presence issues. Only `src/transfer.rs` types are behind `#[cfg(feature = "transfer")]`. See [research.md](research.md) R4.

### 4. Multiple transfers in one turn

If the LLM calls transfer_to_agent multiple times concurrently, the loop takes the first signal (by tool call order) and logs a warning for duplicates. The tool itself is stateless. See [research.md](research.md) R6.

### 5. TransferChain is orchestrator-owned

The chain is passed as an argument to the orchestrator's run method, not stored on the agent or tool. The orchestrator creates a new chain per user message and carries it forward through transfers. This keeps the chain external to the loop.
