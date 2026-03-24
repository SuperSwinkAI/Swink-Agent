# Implementation Plan: Configurable Policy Slots for the Agent Loop

**Branch**: `031-policy-slots` | **Date**: 2026-03-24 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/031-policy-slots/spec.md`

## Summary

Replace five scattered single-purpose hook fields on `AgentLoopConfig` (budget_guard, loop_policy, post_turn_hook, tool_validator, tool_call_transformer) with a unified system of four configurable policy slots at natural seam points in the agent loop. Each slot accepts a `Vec<Arc<dyn Trait>>` of policy implementations, evaluated in order. Default is empty vecs — no policies, anything goes. The loop runs wide open unless the consumer opts in. Two verdict enums enforce Skip-only-in-PreDispatch at compile time. Six built-in policy implementations ship as opt-in convenience structs. Panic isolation via `catch_unwind` keeps the loop resilient to buggy policies.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `tokio` (async runtime), `tokio-util` (CancellationToken), `serde_json` (Value for arguments), `tracing` (debug/warn logging), `std::panic::catch_unwind` (panic isolation)
**Storage**: N/A (in-memory policy evaluation; CheckpointPolicy delegates to existing `CheckpointStore` trait)
**Testing**: `cargo test --workspace` — unit tests in each source module, integration tests in `tests/`
**Target Platform**: Cross-platform library (any target supporting tokio)
**Project Type**: Library crate (`swink-agent`)
**Performance Goals**: Zero-cost when no policies configured (empty vec iteration = no allocation). Sync evaluation only — no async overhead in policy hot path.
**Constraints**: `#[forbid(unsafe_code)]` (note: `catch_unwind` requires `UnwindSafe` bounds, not `unsafe`); no provider-specific dependencies; all shared state via `Arc<Mutex<>>` or atomics
**Scale/Scope**: Core crate refactor — 4 new source files (traits + runner), 6 built-in policy structs, modifications to `loop_/mod.rs`, `loop_/turn.rs`, `loop_/tool_dispatch.rs`, `agent.rs`, `agent_options.rs`, `lib.rs`

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | All types are library structs/traits in the core crate. No new crates — policy system slots into `swink-agent`. Built-in policies are convenience structs in the same crate, not a separate dependency. |
| II. Test-Driven Development | PASS | Each policy trait and built-in gets unit tests. Slot runner gets composition tests. Integration tests verify loop behavior with policies. All tests written before implementation. |
| III. Efficiency & Performance | PASS | Empty vec evaluation = no allocation, no overhead. Sync traits avoid async boxing. `catch_unwind` is called only when policies are present. Interior mutability for stateful policies uses `Mutex` (standard pattern). |
| IV. Leverage the Ecosystem | PASS | No new external dependencies. Uses `std::panic::catch_unwind` (stdlib), `tracing` (already in workspace), `serde_json::Value` (already in workspace). |
| V. Provider Agnosticism | PASS | Policies are provider-agnostic — they see `PolicyContext` (usage, cost, turn index), not provider-specific data. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]` maintained. Panic isolation via `catch_unwind`. Poisoned mutex recovery via `into_inner()`. Two verdict enums enforce Skip-only-in-PreDispatch at compile time. |

No violations. Complexity Tracking table not needed.

## Project Structure

### Documentation (this feature)

```text
specs/031-policy-slots/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output (minimal — no unknowns)
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Phase 1 output
└── checklists/
    └── requirements.md  # Spec quality checklist
```

### Source Code (repository root)

```text
src/
├── policy.rs                    # NEW: PolicyVerdict, PreDispatchVerdict, PolicyContext,
│                                #      ToolPolicyContext, TurnPolicyContext, slot traits,
│                                #      slot runner (run_policies / run_pre_dispatch_policies)
├── policies/
│   ├── mod.rs                   # NEW: re-exports all built-in policies
│   ├── budget.rs                # NEW: BudgetPolicy (PreTurnPolicy)
│   ├── max_turns.rs             # NEW: MaxTurnsPolicy (PreTurnPolicy + PostTurnPolicy)
│   ├── sandbox.rs               # NEW: SandboxPolicy (PreDispatchPolicy)
│   ├── deny_list.rs             # NEW: ToolDenyListPolicy (PreDispatchPolicy)
│   ├── checkpoint.rs            # NEW: CheckpointPolicy (PostTurnPolicy)
│   └── loop_detection.rs        # NEW: LoopDetectionPolicy (PostTurnPolicy)
├── agent.rs                     # MODIFY: replace old fields with policy slot vecs
├── agent_options.rs             # MODIFY: replace old builder methods with policy slot methods
├── loop_/
│   ├── mod.rs                   # MODIFY: AgentLoopConfig — replace 5 fields with 4 vecs,
│   │                            #          add PreTurn slot call, add PostLoop slot call
│   ├── turn.rs                  # MODIFY: add PostTurn slot call, replace tool_validator/
│   │                            #          tool_call_transformer with PreDispatch slot call
│   └── tool_dispatch.rs         # MODIFY: replace approval→transformer→validator pipeline
│                                #          with PreDispatch→approval pipeline
├── lib.rs                       # MODIFY: re-export new types, remove old type re-exports
│
├── budget_guard.rs              # DELETE (replaced by BudgetPolicy)
├── loop_policy.rs               # DELETE (replaced by PreTurnPolicy + PostTurnPolicy)
├── post_turn_hook.rs            # DELETE (replaced by PostTurnPolicy)
├── tool_validator.rs            # DELETE (replaced by PreDispatchPolicy)
└── tool_call_transformer.rs     # DELETE (replaced by PreDispatchPolicy)

tests/
├── common/mod.rs                # MODIFY: add policy test helpers (MockPolicy, etc.)
├── policy_slots.rs              # NEW: integration tests for slot composition
└── (existing test files)        # MODIFY: update any tests using old fields
```

**Structure Decision**: All policy infrastructure lives in the core `swink-agent` crate. Traits and runner in `src/policy.rs` (single file — estimated ~200 lines). Built-in implementations in `src/policies/` directory (one file per policy). No new crates needed.

## Notes

- **Deletion scope**: 5 source files removed (`budget_guard.rs`, `loop_policy.rs`, `post_turn_hook.rs`, `tool_validator.rs`, `tool_call_transformer.rs`). Their functionality is absorbed by the policy slot system and built-in policy implementations.
- **Migration of existing tests**: Tests in `tests/` that reference old types (e.g., `BudgetGuard`, `LoopPolicy`) must be rewritten to use the new policy API. The test semantics should be preserved — same behavior, new API.
- **MaxTurnsPolicy dual-slot**: MaxTurnsPolicy can implement both `PreTurnPolicy` (check before LLM call) and `PostTurnPolicy` (check after turn). The consumer chooses which slot to place it in based on their preferred semantics (pre-call guard vs post-turn check).
- **Dispatch pipeline change**: Old order was approval → transformer → validator → schema → execute. New order is PreDispatch policies → approval → schema validation → execute. This is a semantic change — policies now run before the user sees the tool call in the approval UI.
- **catch_unwind + AssertUnwindSafe**: The slot runner wraps each `evaluate` call with `AssertUnwindSafe` before `catch_unwind`. Policy traits only need `Send + Sync` — implementors do not need to satisfy `UnwindSafe` bounds. This is safe because `catch_unwind` is purely for isolation (the panicking policy is skipped), not for recovery of shared state.
- **PreDispatch two-pass evaluation**: PreDispatch uses a two-pass approach. Pass 1: evaluate all PreDispatch policies for all tool calls in the batch. If any returns Stop, the entire batch is aborted before any tool executes. Pass 2: proceed to approval → schema → execute for all passing tool calls. This ensures Stop means "halt everything" and Skip means "skip just this tool."
- **CheckpointPolicy async bridge**: CheckpointPolicy is sync (PostTurnPolicy) but delegates to async CheckpointStore via `tokio::spawn` (fire-and-forget). Captures `Handle::current()` at construction time. Returns Continue immediately — persistence does not block the loop.
- **SandboxPolicy path detection**: Configured with specific argument field names to check (default: `["path", "file_path", "file"]`). Only inspects string values in those fields. Skips with error on violation — no silent path rewriting.
