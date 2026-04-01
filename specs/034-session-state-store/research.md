# Research: Session Key-Value State Store

**Feature**: 034-session-state-store
**Date**: 2026-03-31

## R1: Thread-Safety Primitive Selection

**Decision**: Use `std::sync::RwLock` (not `tokio::sync::RwLock`).

**Rationale**: Policy evaluation is synchronous (`&self`, not async). Using `tokio::sync::RwLock` would require `.await` on lock acquisition, which is incompatible with sync policy traits. `std::sync::RwLock` allows multiple concurrent readers and serialized writers without async. The lock hold time is minimal (HashMap get/insert + serde_json conversion), so blocking is negligible. This matches the existing pattern: `steering_queue` and `follow_up_queue` on Agent use `Arc<Mutex<>>` (std, not tokio).

**Alternatives considered**:
- `tokio::sync::RwLock`: Requires async context for all access. Incompatible with sync policy evaluation (FR-015). Would force policy traits to become async, which contradicts spec 031 (FR-014: "All four policy traits MUST be synchronous").
- `parking_lot::RwLock`: Better performance under contention, no poisoning. However, adds a new dependency for marginal gain. The crate already uses `std::sync` primitives exclusively. Poison recovery via `into_inner()` is an established pattern (constitution VI).
- `DashMap`: Lock-free concurrent HashMap. Overkill — no hot-path contention expected. Delta tracking requires coordinated read-modify-write that DashMap doesn't simplify.

## R2: Tool State Access Mechanism

**Decision**: Pass `Arc<RwLock<SessionState>>` as a new parameter to `AgentTool::execute`.

**Rationale**: The `execute` signature must change to include state access. Adding a new parameter is the most explicit approach. Since `AgentTool` is a trait that consumers implement, this is a breaking change. However, the trait already has 4 parameters and adding a 5th is consistent with the existing pattern. The alternative (wrapping params in a context struct) would be a larger breaking change.

**Alternatives considered**:
- **Context struct**: Bundle `tool_call_id`, `params`, `cancellation_token`, `on_update`, and `state` into a `ToolExecutionContext`. Cleaner long-term but a more disruptive migration — every existing tool implementation must change its signature to destructure the context. Deferred to a future "tool API v2" cleanup.
- **Thread-local / global state**: Violates constitution ("No global mutable state"). Rejected.
- **Callback-based access**: Tool receives a `get`/`set` closure pair. Adds indirection without benefit. Rejected.

## R3: JSONL State Line Format

**Decision**: Use a tagged JSON line with `"_state": true` discriminator, storing the full materialized state snapshot.

**Rationale**: The JSONL format already uses `"_custom": true` to distinguish custom message envelopes from LlmMessage lines. Following the same pattern, a state line uses `"_state": true` with the materialized state as the payload. On save, the state line replaces any previous state line (there is at most one). On load, if present, it reconstructs SessionState; if absent (pre-034), state is empty.

**Format**:
```json
{"_state": true, "data": {"key1": "value1", "key2": 42}}
```

**Alternatives considered**:
- **Delta log**: Append one delta line per turn. Requires ordered replay on load, more complex, and O(n) in turn count. Rejected per spec Assumptions (snapshot > delta for persistence).
- **Separate file**: Store state in `{session_id}.state.json` alongside `{session_id}.jsonl`. Simpler code but splits atomicity — a crash between writing the two files leaves inconsistent state. Rejected.
- **Metadata field**: Embed state in the SessionMeta line. SessionMeta is small and frequently rewritten on append (fast path). Embedding arbitrarily large state would break the fast-path optimization. Rejected.

## R4: PolicyContext State Access

**Decision**: Add `state: &SessionState` field to `PolicyContext`.

**Rationale**: Policies need read-only access. Since policies are sync and state is behind `RwLock`, the loop acquires a read lock before evaluating policies and passes the `&SessionState` reference. The lock is held for the duration of policy evaluation (a slot's entire policy vec). This is safe because policy evaluation is sequential within a slot and does not call tools (which would need write access).

**Alternatives considered**:
- **Clone state snapshot**: Clone the `HashMap<String, Value>` before policy evaluation. Avoids holding the lock but wastes allocation for potentially large state. Rejected — read lock contention is not a concern since policy evaluation doesn't overlap with tool execution (it happens after tools complete).
- **Separate `StateView` trait**: Abstract read-only access behind a trait. Over-engineering for a simple `&SessionState` borrow. Rejected.

## R5: Checkpoint State Storage

**Decision**: Add `state: Option<serde_json::Value>` field to both `Checkpoint` and `LoopCheckpoint`, with `#[serde(default)]` for backward compatibility.

**Rationale**: Both checkpoint types already use `serde_json::Value` for metadata. Adding an optional state field follows the same pattern as `custom_messages` (added in a prior spec with `#[serde(default)]` for backward compat). The state is serialized as the full materialized HashMap via `serde_json::to_value()`. On restore, `None` means empty state.

**Alternatives considered**:
- **Store in metadata**: Use `checkpoint.metadata["session_state"]`. Works but makes state a second-class citizen buried in a generic map. Harder to discover and type-check. Rejected.
- **Separate checkpoint type**: `StateCheckpoint` alongside `Checkpoint`. Unnecessary complexity for a single `Option<Value>` field. Rejected.

## R6: Delta Flush Timing in the Loop

**Decision**: Flush delta after PostTurn policy evaluation, before `TurnEnd` event emission.

**Rationale**: PostTurn policies may read state (via PolicyContext) but should see the state as it was when the turn ended, including all tool mutations. The delta flush happens after policies evaluate so that the flushed delta captures the complete turn's mutations. The `StateChanged` event (if delta is non-empty) is emitted immediately before `TurnEnd`, giving subscribers the delta before the turn snapshot.

**Sequence**:
1. Tool execution (tools mutate state via write lock)
2. PostTurn policy evaluation (policies read state via read lock on PolicyContext)
3. Flush delta (take pending delta, reset tracking)
4. If delta non-empty: emit `AgentEvent::StateChanged { delta }`
5. Emit `AgentEvent::TurnEnd { ... snapshot with state_delta ... }`

## R7: AgentEvent Variant Design

**Decision**: Add `AgentEvent::StateChanged { delta: StateDelta }` as a new variant.

**Rationale**: A dedicated event variant allows subscribers to react specifically to state changes without parsing TurnEnd. The delta is also included in `TurnSnapshot` for subscribers that prefer the aggregated turn view. Both channels serve different consumer patterns — event-driven (react to state changes) vs. snapshot-driven (persist full turn context).

**Alternatives considered**:
- **TurnEnd only**: Embed delta in TurnSnapshot, no separate event. Subscribers must process every TurnEnd even if they only care about state. Less ergonomic. Rejected.
- **Custom event via `AgentEvent::Custom(Emission)`**: Avoids adding a variant but loses type safety. Consumers must downcast. Rejected.
