# Public API Contract: Session Key-Value State Store

**Feature**: 034-session-state-store
**Date**: 2026-03-31

## Core Types (swink-agent crate)

### SessionState

```rust
/// Key-value store with change tracking for session-attached structured data.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionState {
    data: HashMap<String, Value>,
    #[serde(skip)]
    delta: StateDelta,
}

impl SessionState {
    /// Create a new empty session state.
    pub fn new() -> Self;

    /// Create session state pre-populated with the given data.
    /// Pre-seeded data does NOT appear in the delta.
    pub fn with_data(data: HashMap<String, Value>) -> Self;

    /// Get a typed value by key. Returns None if key missing or deserialization fails.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T>;

    /// Get the raw JSON value by key without deserialization.
    pub fn get_raw(&self, key: &str) -> Option<&Value>;

    /// Set a typed value. Serializes to Value and records in delta.
    pub fn set<T: Serialize>(&mut self, key: &str, value: T);

    /// Remove a key. Records removal in delta. No-op if key absent.
    pub fn remove(&mut self, key: &str);

    /// Check if a key exists.
    pub fn contains(&self, key: &str) -> bool;

    /// Iterate over all keys.
    pub fn keys(&self) -> impl Iterator<Item = &str>;

    /// Number of key-value pairs.
    pub fn len(&self) -> usize;

    /// True if no key-value pairs.
    pub fn is_empty(&self) -> bool;

    /// Remove all key-value pairs. Records all existing keys as removed in delta.
    pub fn clear(&mut self);

    /// Read-only reference to pending delta.
    pub fn delta(&self) -> &StateDelta;

    /// Take the pending delta and reset tracking. Returns the delta.
    pub fn flush_delta(&mut self) -> StateDelta;

    /// Snapshot the materialized data as a JSON Value (for persistence).
    pub fn snapshot(&self) -> Value;

    /// Restore from a JSON Value snapshot. Clears existing data and delta.
    pub fn restore_from_snapshot(snapshot: Value) -> Self;
}
```

### StateDelta

```rust
/// Record of mutations since the last flush.
/// Some(value) = set/update, None = removed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub changes: HashMap<String, Option<Value>>,
}

impl StateDelta {
    /// True if no changes recorded.
    pub fn is_empty(&self) -> bool;

    /// Number of changed keys.
    pub fn len(&self) -> usize;
}
```

## Agent Integration (swink-agent crate)

### Agent Methods

```rust
impl Agent {
    /// Access the session state (thread-safe, shared reference).
    /// Name avoids collision with existing `state() -> &AgentState`.
    pub fn session_state(&self) -> &Arc<RwLock<SessionState>>;
}
```

### AgentOptions Builder

```rust
impl AgentOptions {
    /// Pre-seed session state with initial key-value pairs.
    pub fn with_initial_state(self, state: SessionState) -> Self;

    /// Add a single key-value pair to initial state.
    pub fn with_state_entry(self, key: impl Into<String>, value: impl Serialize) -> Self;
}
```

## Tool Execution (swink-agent crate)

### AgentTool::execute Signature Change

```rust
pub trait AgentTool: Send + Sync {
    // ... existing methods unchanged ...

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<RwLock<SessionState>>,  // NEW parameter
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}
```

## Policy Context (swink-agent crate)

### PolicyContext Extension

```rust
pub struct PolicyContext<'a> {
    pub turn_index: usize,
    pub accumulated_usage: &'a Usage,
    pub accumulated_cost: &'a Cost,
    pub message_count: usize,
    pub overflow_signal: bool,
    pub new_messages: &'a [AgentMessage],
    pub state: &'a SessionState,  // NEW field — read-only access
}
```

## Events (swink-agent crate)

### New AgentEvent Variant

```rust
pub enum AgentEvent {
    // ... existing variants ...

    /// Emitted when state delta is flushed (non-empty only).
    /// Fired immediately before TurnEnd.
    StateChanged {
        delta: StateDelta,
    },
}
```

### TurnSnapshot Extension

```rust
pub struct TurnSnapshot {
    pub turn_index: usize,
    pub messages: Arc<Vec<LlmMessage>>,
    pub usage: Usage,
    pub cost: Cost,
    pub stop_reason: StopReason,
    pub state_delta: Option<StateDelta>,  // NEW field — None if no changes
}
```

## Checkpoint (swink-agent crate)

### Checkpoint Extension

```rust
pub struct Checkpoint {
    // ... existing fields ...
    #[serde(default)]
    pub state: Option<Value>,  // NEW — serialized SessionState.data
}

pub struct LoopCheckpoint {
    // ... existing fields ...
    #[serde(default)]
    pub state: Option<Value>,  // NEW — serialized SessionState.data
}
```

## Session Store (swink-agent-memory crate)

### SessionStore Trait Extension

```rust
pub trait SessionStore: Send + Sync {
    // ... existing methods unchanged ...

    /// Save session state snapshot. Default: no-op.
    fn save_state(&self, id: &str, state: &Value) -> io::Result<()> {
        let _ = (id, state);
        Ok(())
    }

    /// Load session state snapshot. Default: None (empty state).
    fn load_state(&self, id: &str) -> io::Result<Option<Value>> {
        let _ = id;
        Ok(None)
    }
}
```

### JsonlSessionStore JSONL Line Format

```json
{"_state": true, "data": {"key1": "value", "key2": 42}}
```

- Discriminated by `_state: true` field (parallel to `_custom: true` for custom messages)
- At most one state line per session file
- `save_state` replaces existing state line (full rewrite) or appends if none exists

## Re-exports (swink-agent crate lib.rs)

```rust
pub use state::{SessionState, StateDelta};
```
