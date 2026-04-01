# Feature Specification: Session Key-Value State Store

**Feature Branch**: `034-session-state-store`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "Key-Value State Store — session-attached structured key-value storage with delta tracking for non-message data"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Tool Stores Structured Data Across Turns (Priority: P1)

A library consumer builds an agent with a research tool that accumulates findings across multiple turns. The tool stores discovered URLs, extracted facts, and intermediate results in the session state rather than injecting them into the conversation as messages. On subsequent turns, the tool reads the accumulated state to avoid re-fetching and to build on prior work.

**Why this priority**: Tools writing structured non-message data back to agent state is the primary use case that has no current solution. Today tools can only return text/content blocks — they cannot persist structured key-value data that outlives a single tool call.

**Independent Test**: Can be fully tested by creating a mock tool that calls `state.set("urls", [...])` during execution, running a multi-turn conversation, and verifying that a subsequent tool call can read the accumulated URLs via `state.get("urls")`.

**Acceptance Scenarios**:

1. **Given** an agent with state access enabled, **When** a tool calls `state.set("count", 5)` during execution, **Then** `state.get::<i64>("count")` returns `Some(5)` in the same and subsequent turns.
2. **Given** an agent with state containing `{"key": "old"}`, **When** a tool calls `state.set("key", "new")`, **Then** the delta records `{"key": "new"}` and the materialized state reflects the update.
3. **Given** an agent with state containing `{"a": 1, "b": 2}`, **When** a tool calls `state.remove("a")`, **Then** `state.get::<i64>("a")` returns `None` and the delta records `{"a": null}`.

---

### User Story 2 - State Survives Session Save and Restore (Priority: P1)

A library consumer builds an agent that tracks user preferences and accumulated context across sessions. After a session ends, the state is persisted alongside conversation messages. When the session is restored later, the full materialized state is available immediately without replaying the conversation.

**Why this priority**: State that disappears on session restore defeats the purpose of structured persistence. This is equally critical as the core API — without it, state is just an in-memory cache.

**Independent Test**: Can be fully tested by setting state values, saving the session via `SessionStore`, loading the session in a new agent, and verifying all state key-value pairs are restored.

**Acceptance Scenarios**:

1. **Given** an agent with state `{"user_lang": "en", "search_count": 7}`, **When** the session is saved and then loaded into a new agent, **Then** `state.get::<String>("user_lang")` returns `Some("en")` and `state.get::<i64>("search_count")` returns `Some(7)`.
2. **Given** a session store with no existing session state (pre-034 sessions), **When** the session is loaded, **Then** state is empty (backward compatible) and the agent operates normally.
3. **Given** state with nested JSON values (`{"config": {"theme": "dark", "font_size": 14}}`), **When** the session is saved and restored, **Then** the nested structure is preserved exactly.

---

### User Story 3 - Concurrent Tool Executions Access State Safely (Priority: P1)

A library consumer runs an agent with concurrent tool execution enabled. Multiple tools read and write state simultaneously without data races. Reads never block each other. Writes are serialized so that the final state is deterministic.

**Why this priority**: The agent loop already supports concurrent tool execution via `tokio::spawn`. State must be thread-safe or it becomes a footgun for the most common deployment pattern.

**Independent Test**: Can be fully tested by configuring an agent with concurrent tool execution and two tools that both read and write to state, verifying no panics and that all writes are reflected.

**Acceptance Scenarios**:

1. **Given** two tools executing concurrently, **When** both read the same key, **Then** both receive the current value without blocking each other.
2. **Given** two tools executing concurrently, **When** both write to different keys, **Then** both writes are reflected in the final state.
3. **Given** two tools executing concurrently, **When** both write to the same key, **Then** one write wins (last-writer-wins) and the final state is consistent.

---

### User Story 4 - Delta Tracking for Efficient Persistence (Priority: P2)

A library consumer wants efficient incremental persistence. Rather than re-serializing the entire state on every turn, only the changes (delta) since the last flush are persisted. The delta is emitted as part of the turn-end event so subscribers and persistence layers can react to state changes.

**Why this priority**: Full snapshot on every turn is correct but wasteful for append-only stores like JSONL. Delta tracking enables efficient persistence without changing the storage format fundamentally.

**Independent Test**: Can be fully tested by setting multiple keys, flushing the delta, setting more keys, and verifying the second delta contains only the new changes.

**Acceptance Scenarios**:

1. **Given** an empty state, **When** `set("a", 1)` and `set("b", 2)` are called, **Then** `delta()` contains `{"a": 1, "b": 2}`.
2. **Given** a state with pending delta, **When** `flush_delta()` is called, **Then** the returned delta contains all pending changes and subsequent `delta()` is empty.
3. **Given** `set("x", 1)` then `set("x", 2)` before a flush, **Then** the delta contains `{"x": 2}` (last value wins within a delta window).
4. **Given** `set("y", 1)` then `remove("y")` before a flush, **Then** the delta contains `{"y": null}` (removal supersedes prior set).

---

### User Story 5 - Policies Can Read State for Decisions (Priority: P2)

A library consumer implements a custom policy that checks session state to enforce business rules. For example, a policy reads a "verified" flag from state and stops the loop if the user has not been verified. The policy receives read-only access to state via the policy context.

**Why this priority**: State-aware policies are a natural extension of the policy system but depend on both the state store and the policy infrastructure being in place.

**Independent Test**: Can be fully tested by implementing a custom policy that reads a state key and returns Stop/Continue based on the value, then verifying the policy fires correctly.

**Acceptance Scenarios**:

1. **Given** a pre-turn policy that checks `state.get::<bool>("verified")`, **When** the key is `Some(true)`, **Then** the policy returns Continue.
2. **Given** a pre-turn policy that checks `state.get::<bool>("verified")`, **When** the key is `None` or `Some(false)`, **Then** the policy returns Stop with a descriptive reason.

---

### User Story 6 - Agent Owner Reads and Pre-Seeds State (Priority: P3)

A library consumer wants to pre-seed the agent with application-specific state before the first turn (e.g., user profile data, feature flags, environment context). They also want to read the final state after a conversation ends to extract structured outputs.

**Why this priority**: Pre-seeding and post-run extraction are convenience features that round out the API but are not required for the core loop to function.

**Independent Test**: Can be fully tested by pre-seeding state before calling `prompt_async`, then reading state after the call returns.

**Acceptance Scenarios**:

1. **Given** an agent with pre-seeded state `{"user_id": "abc123"}`, **When** a tool executes, **Then** `state.get::<String>("user_id")` returns `Some("abc123")`.
2. **Given** a completed conversation where tools set `{"result": "success", "items_found": 42}`, **When** the consumer reads agent state, **Then** both keys are accessible.

---

### Edge Cases

- What happens when `get` is called with a type that does not match the stored JSON? Deserialization fails silently, returning `None`. The raw `Value` is not corrupted — only the typed extraction fails.
- What happens when state is accessed after the agent loop ends? State remains accessible on the Agent struct for post-run extraction. It is not cleared automatically.
- What happens when `flush_delta` is called with no pending changes? Returns an empty `StateDelta` (no changes map entries). This is a no-op, not an error.
- What happens when a tool panics while holding a write lock on state? The `RwLock` becomes poisoned. The state accessors recover via `PoisonError::into_inner()`, matching the existing pattern used for steering/follow-up queues in the Agent struct.
- What happens when the same key is set by a tool and then by a policy injection in the same turn? Both writes go through the delta. The policy write (which happens after tool execution) overwrites the tool write. Last-writer-wins semantics.
- What happens when state contains very large values (e.g., multi-MB JSON arrays)? No size limit is enforced by the state store itself. Consumers are responsible for managing value sizes. Serialization to the session store is bounded by the same constraints as message persistence.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a `SessionState` type that stores key-value pairs where keys are strings and values are JSON-serializable (stored internally as `serde_json::Value`).
- **FR-002**: System MUST support typed get operations that deserialize the stored value into the requested type, returning `None` if the key does not exist or deserialization fails.
- **FR-003**: System MUST support typed set operations that serialize the value to JSON and store it under the given key.
- **FR-004**: System MUST support `remove(key)` to delete a key-value pair from state.
- **FR-005**: System MUST support `contains(key)` to check key existence and `keys()` to iterate over all keys.
- **FR-006**: System MUST support raw value access returning the stored JSON value directly without deserialization overhead.
- **FR-007**: System MUST support `len()` and `is_empty()` for state introspection.
- **FR-008**: System MUST track all mutations (set and remove) as a delta — a map of key names to an optional value where presence indicates a set/update and absence of value indicates a removal.
- **FR-009**: System MUST provide read access to the current pending delta and a flush operation that returns the pending delta and resets the tracking.
- **FR-010**: Within a single delta window, multiple writes to the same key MUST collapse to the final value. A set followed by a remove MUST record the key as removed. A remove followed by a set MUST record the key as set to the new value.
- **FR-011**: System MUST provide `clear()` to remove all key-value pairs and record all existing keys as removed in the delta.
- **FR-012**: The state store MUST be thread-safe for concurrent access from multiple tool executions. Multiple concurrent reads MUST NOT block each other. Writes MUST be serialized. Poisoned locks MUST be recovered gracefully matching existing crate patterns.
- **FR-013**: The Agent struct MUST expose state via a method that avoids collision with the existing `state()` method returning `&AgentState`.
- **FR-014**: Tools MUST receive access to the session state via a shared reference passed alongside existing execution parameters.
- **FR-015**: Policy context MUST be extended with read-only access to the session state so that policies can make state-aware decisions.
- **FR-016**: The agent loop MUST flush the state delta at the end of each turn (after PostTurn policies evaluate) and include it in the turn-end event data.
- **FR-017**: A state-changed event MUST be emitted when the state delta is flushed and the delta is non-empty, immediately before the turn-end event. If no state mutations occurred during the turn (empty delta), the event MUST be suppressed.
- **FR-018**: The session store trait MUST be extended with default-implemented methods for state persistence. Default implementations MUST be no-ops for backward compatibility with existing store implementations.
- **FR-019**: The JSONL session store MUST implement state persistence by storing the full materialized state as a dedicated line type distinguishable from message and metadata lines.
- **FR-020**: On session load, if a state snapshot exists, the session state MUST be reconstructed from it. If no state exists (pre-034 sessions), the state MUST be empty.
- **FR-021**: Checkpoint types MUST include an optional state field so that state survives checkpoint save/restore cycles.
- **FR-022**: The builder pattern MUST support providing initial state (pre-seeding) before the first turn. Pre-seeded state MUST be treated as baseline — it MUST NOT appear in the delta. Only runtime mutations during the conversation are tracked as delta entries.
- **FR-023**: The delta type MUST be serializable and deserializable for persistence and event transmission.
- **FR-024**: The session state MUST implement default construction (empty state, empty delta) and cloning (for snapshot creation).

### Key Entities

- **SessionState**: The core key-value store with change tracking. Holds materialized state and pending delta.
- **StateDelta**: A record of mutations since the last flush. Maps key names to optional values — present value for set/update, absent value for removal.
- **StateSnapshot**: The serialized form of the full materialized state, used for persistence in session stores and checkpoints.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Tools can store and retrieve typed key-value data across turns without injecting data into the conversation message stream.
- **SC-002**: State survives session save/load cycles with no data loss for all JSON-representable value types (strings, numbers, booleans, arrays, objects, null).
- **SC-003**: Concurrent tool executions can read and write state without data races or panics.
- **SC-004**: Delta tracking correctly captures all mutations within a turn, and flushing resets the delta to empty.
- **SC-005**: Existing agents without state usage (empty state, no state calls) experience no behavioral change or measurable overhead.
- **SC-006**: Pre-034 sessions load successfully with empty state (full backward compatibility).
- **SC-007**: Policies can read state to make enforcement decisions without needing direct access to tool internals.
- **SC-008**: A library consumer can add state support to an existing agent by pre-seeding values and passing state to tools with no changes to loop configuration or stream function.

## Clarifications

### Session 2026-03-31

- Q: Should `StateChanged` event be emitted when the delta is empty (no state mutations in a turn)? → A: Skip — do not emit when delta is empty. Subscribers who need turn-completion signals can use `TurnEnd`.
- Q: In multi-agent scenarios (spec 009), should child agents inherit parent state? → A: Independent — child agents start with empty state. Parents can explicitly pre-seed child state via the builder if needed.
- Q: Should pre-seeded state (via builder) appear in the initial delta? → A: Baseline — pre-seeded state produces no delta. Delta tracks only runtime mutations during the conversation.

## Assumptions

- Values are JSON-serializable. Non-JSON types (binary data, custom structs) must be serialized to JSON by the consumer before storing.
- The state store is per-session, not global. Each agent instance has its own independent state. In multi-agent scenarios (spec 009), child agents start with empty state — there is no implicit inheritance from the parent. Parents can pre-seed child state explicitly via the builder.
- There is no key-level access control or namespacing. All tools and policies see the same flat key-value namespace. Consumers can adopt naming conventions (e.g., `tool_name.key`) but the system does not enforce them.
- State size is not bounded by this specification. Consumers are responsible for managing state growth. Future specifications may add eviction or size-limit policies.
- The persistence strategy is full-snapshot-on-flush (not delta replay). This is simpler to implement, faster to load, and avoids the complexity of ordered delta application. The trade-off is that each flush writes the full state, which is acceptable given that state is expected to be small relative to conversation messages.
