# Research: Configurable Policy Slots for the Agent Loop

**Feature**: 031-policy-slots | **Date**: 2026-03-24

## Status

No NEEDS CLARIFICATION items in Technical Context. All design decisions were resolved during the specification and clarification phases.

## Decisions

### 1. Panic isolation mechanism

- **Decision**: `std::panic::catch_unwind` with `AssertUnwindSafe` wrapper
- **Rationale**: Matches existing pattern used for event subscribers in `agent.rs` (`dispatch_event`). `AssertUnwindSafe` is safe because policy `evaluate` takes `&self` and context structs contain only references and Copy types — no mutable shared state that could be left in an inconsistent state by a panic.
- **Alternatives considered**: (a) Let panics propagate — rejected because third-party policies should not crash the host loop. (b) `std::thread::spawn` + join — rejected because excessive overhead for a sync function call.

### 2. Verdict type design

- **Decision**: Two separate enums — `PolicyVerdict` (Continue, Stop, Inject) and `PreDispatchVerdict` (Continue, Stop, Inject, Skip)
- **Rationale**: Compile-time enforcement that Skip is only valid in the PreDispatch slot. A single enum with runtime checks was rejected to avoid "wrong slot" bugs that would only surface at runtime.
- **Alternatives considered**: (a) Single `PolicyVerdict` with Skip, runtime ignore in non-PreDispatch slots — rejected for safety reasons. (b) Trait-level associated types — rejected for complexity; two simple enums are clearer.

### 3. Stateful policy pattern

- **Decision**: `&self` on trait methods; interior mutability (`Mutex`, atomics) for state
- **Rationale**: Standard Rust pattern for shared trait objects behind `Arc`. Allows policies to be cloned/shared across threads. `&mut self` would require exclusive access, breaking `Arc<dyn Policy>` sharing.
- **Alternatives considered**: `&mut self` — rejected because it prevents `Arc` sharing and would require the slot runner to hold mutable references.

### 4. PreDispatch Stop mid-batch behavior

- **Decision**: Entire batch aborted — no tools execute, loop stops immediately
- **Rationale**: Stop means "something is fundamentally wrong." Allowing partial execution after a Stop signal would be confusing and could leave the system in an inconsistent state. PreDispatch is a pre-processing pass, not dispatch-as-you-go.
- **Alternatives considered**: (a) Execute already-approved tools, skip remaining — rejected for complexity and inconsistency. (b) Skip the triggering tool, continue others — rejected because that's what Skip is for, not Stop.

### 5. Slot runner implementation approach

- **Decision**: Generic `run_policies` function for PolicyVerdict slots; separate `run_pre_dispatch_policies` for PreDispatchVerdict slot
- **Rationale**: The two verdict types require slightly different handling (Skip short-circuit vs no Skip). A single generic function would need conditional logic or trait bounds that obscure the simplicity. Two small functions (~30 lines each) are clearer than one generic one.
- **Alternatives considered**: Single generic runner with associated type — rejected for readability.
