# Research: TransferToAgent Tool & Handoff Safety

**Feature**: 040-agent-transfer-handoff  
**Date**: 2026-04-02

## Research Tasks

### R1: StopReason::Transfer and the Copy Constraint

**Question**: The spec calls for `StopReason::Transfer(TransferSignal)` but `StopReason` derives `Copy`. `TransferSignal` contains `String` and `Vec<AgentMessage>` which are not `Copy`. Adding data to the variant would break the `Copy` derive — a breaking change for all downstream code.

**Decision**: Add `StopReason::Transfer` as a unit variant (no data). Place the `TransferSignal` as a separate `Option<TransferSignal>` field on `AgentResult`, mirroring how `error: Option<String>` accompanies `StopReason::Error`. This preserves `Copy` on `StopReason` and keeps the pattern consistent.

**Rationale**: Removing `Copy` from `StopReason` would be a breaking change affecting the entire codebase. The `Error` + `error` field pattern already establishes the precedent for "stop reason variant + companion data" in `AgentResult`.

**Alternatives considered**:
- `Transfer(Box<TransferSignal>)` — still not `Copy` because `Box` isn't `Copy` (rejected)
- Remove `Copy` derive from `StopReason` — breaking change across entire codebase (rejected)
- `Transfer(Arc<TransferSignal>)` — `Arc` is not `Copy` either (rejected)
- Encode transfer as `StopReason::Stop` + a field — loses type-level distinction (rejected)

### R2: AgentToolResult Transfer Signal Field

**Question**: How does adding `Option<TransferSignal>` to `AgentToolResult` interact with serialization and existing consumers?

**Decision**: Add `transfer_signal: Option<TransferSignal>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Existing JSON deserialization succeeds (missing field defaults to None). Add a `transfer()` constructor alongside `text()` and `error()`.

**Rationale**: The `serde(default)` + `skip_serializing_if` pattern ensures wire-format backward compatibility. Consumers reading tool results from existing JSON or checkpoints will deserialize without issues.

### R3: Loop Integration Point

**Question**: Where in the loop should the transfer signal be detected?

**Finding**: After `execute_tools_concurrently()` returns tool results, they are added to `context_messages` as `ToolResultMessage`s. The loop then returns `TurnOutcome::ContinueInner`. The transfer check must happen between tool result processing and the continue decision.

**Decision**: After tool results are collected and added to context, scan results for any `transfer_signal`. If found, construct the full `TransferSignal` (enriching with conversation history from `LoopState.context_messages`), set the stop reason to `Transfer`, and return `TurnOutcome::BreakInner` (or a new `TurnOutcome::Transfer` variant) instead of `ContinueInner`.

**Rationale**: This is the minimal-intrusion integration point. Tool results are already processed and in context (so conversation history is complete). The check is a simple scan of result signals. The loop terminates cleanly via the existing break mechanism.

### R4: Feature Gate Integration

**Question**: How should the `transfer` feature gate interact with the `StopReason::Transfer` variant and `AgentToolResult.transfer_signal` field?

**Decision**: `StopReason::Transfer` and `AgentToolResult.transfer_signal` are always compiled (not gated). They are core type changes that affect type shapes and match exhaustiveness. The feature gate controls only `src/transfer.rs` (the `TransferToAgentTool`, `TransferSignal`, `TransferChain`, `TransferError` types and their re-export from `lib.rs`).

**Rationale**: Gating enum variants and struct fields behind features creates match-arm compilation issues and conditional field presence. The overhead of an unused enum variant and an always-None field is negligible. Only the tool implementation and supporting types are gated.

### R5: TransferSignal Serialization

**Question**: Should `TransferSignal` implement `Serialize`/`Deserialize`?

**Decision**: Yes. `TransferSignal` derives `Clone`, `Debug`, `Serialize`, `Deserialize`. The `conversation_history` field is `Vec<AgentMessage>` — `AgentMessage::Llm` variants serialize fine, but `AgentMessage::Custom` uses trait objects. Conversation history in the transfer signal will include only `AgentMessage::Llm` variants (custom messages are filtered out, consistent with how `in_flight_llm_messages` already works).

**Rationale**: Serializability enables checkpoint/persistence of transfer signals by downstream consumers. Filtering custom messages matches existing patterns.

### R6: Multiple Transfer Calls in One Turn

**Question**: FR-010 says only the first transfer is honored. How is the "already pending" state tracked?

**Decision**: The tool itself does not track state. The loop scans all tool results after concurrent execution. If multiple transfer signals exist, only the first (by tool call order) is used. Others are logged as warnings. Since tools execute concurrently but results are ordered by the original call list, this is deterministic.

**Rationale**: Stateless tool, deterministic ordering. No need for a mutable "pending" flag on the tool — the loop handles deduplication during its post-execution scan.
