# Data Model: Session Key-Value State Store

**Feature**: 034-session-state-store
**Date**: 2026-03-31

## Entities

### SessionState

The core key-value store with change tracking.

| Field | Type | Description |
|-------|------|-------------|
| data | `HashMap<String, Value>` | Materialized key-value pairs |
| delta | `StateDelta` | Pending mutations since last flush |

**Invariants**:
- `data` always reflects the current truth (all sets/removes applied immediately)
- `delta` tracks mutations since last `flush_delta()` call
- Pre-seeded values populate `data` but do NOT appear in `delta`
- After `flush_delta()`, `delta.changes` is empty
- Thread-safety: wrapped in `Arc<RwLock<SessionState>>` at the Agent level

**Traits**: `Default` (empty data + empty delta), `Clone` (snapshot creation), `Debug`, `Serialize`, `Deserialize`

### StateDelta

A record of mutations within a delta window (typically one turn).

| Field | Type | Description |
|-------|------|-------------|
| changes | `HashMap<String, Option<Value>>` | `Some(v)` = set/update, `None` = removed |

**Collapse rules** (within a single delta window):
- `set("k", 1)` then `set("k", 2)` → `{"k": Some(2)}`
- `set("k", 1)` then `remove("k")` → `{"k": None}`
- `remove("k")` then `set("k", 3)` → `{"k": Some(3)}`
- `clear()` → all existing keys mapped to `None`

**Traits**: `Default` (empty changes), `Clone`, `Debug`, `Serialize`, `Deserialize`

**Methods**:
- `is_empty() -> bool` — true when changes map is empty
- `len() -> usize` — number of changed keys

### StateSnapshot (serialized form)

Not a separate struct — it is the `serde_json::Value` serialization of `SessionState.data` (the `HashMap<String, Value>`). Used in:
- JSONL session files: `{"_state": true, "data": {...}}`
- Checkpoint fields: `state: Option<Value>`

## Relationships

```
Agent
├── state: Arc<RwLock<SessionState>>     (owned, 1:1)
│   ├── data: HashMap<String, Value>
│   └── delta: StateDelta
│       └── changes: HashMap<String, Option<Value>>
│
├── AgentLoopConfig (borrows Arc<RwLock<SessionState>>)
│   └── passed to loop, shared with tool execution & policy evaluation
│
└── Checkpoint / LoopCheckpoint
    └── state: Option<Value>             (serialized snapshot)
```

## State Transitions

```
                  ┌──────────────┐
                  │ Empty State   │ ← Default / new agent / child agent
                  │ data: {}      │
                  │ delta: {}     │
                  └──────┬───────┘
                         │
              pre-seed via builder
              (no delta recorded)
                         │
                         ▼
                  ┌──────────────┐
                  │ Baseline     │ ← Pre-seeded values in data, empty delta
                  │ data: {k:v}  │
                  │ delta: {}    │
                  └──────┬───────┘
                         │
              tool/policy mutations
              (delta records changes)
                         │
                         ▼
                  ┌──────────────┐
                  │ Dirty        │ ← Mutations pending in delta
                  │ data: {k:v'} │
                  │ delta: {k:v'}│
                  └──────┬───────┘
                         │
              flush_delta() at turn end
              (emits StateChanged if non-empty)
                         │
                         ▼
                  ┌──────────────┐
                  │ Clean        │ ← Delta reset, data retains current values
                  │ data: {k:v'} │
                  │ delta: {}    │
                  └──────┬───────┘
                         │
              next turn mutations ──→ back to Dirty
              session save ──→ snapshot of data persisted
              session load ──→ reconstructed to Clean state
```

## Persistence Format

### JSONL Session File

```
Line 1:  {"id":"...","title":"...","created_at":"...","updated_at":"..."}    ← SessionMeta
Line 2:  {"role":"user","content":[...]}                                      ← LlmMessage
Line 3:  {"role":"assistant","content":[...],...}                              ← LlmMessage
...
Line N:  {"_state":true,"data":{"key1":"val","key2":42,"key3":[1,2,3]}}      ← State snapshot
```

- At most one `_state` line per file
- Written/updated on `save_full` or dedicated `save_state` call
- Position in file: appended after messages (or replaces previous state line on rewrite)
- Absent in pre-034 sessions → empty state on load

### Checkpoint

```json
{
  "id": "...",
  "system_prompt": "...",
  "messages": [...],
  "custom_messages": [...],
  "state": {"key1": "val", "key2": 42},   // ← new optional field
  "turn_count": 5,
  "usage": {...},
  "cost": {...},
  ...
}
```

- `state: null` or absent → empty SessionState on restore
- `#[serde(default)]` ensures backward compatibility
