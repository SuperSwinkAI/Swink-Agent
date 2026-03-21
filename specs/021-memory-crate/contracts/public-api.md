# Public API Contract: Memory Crate

**Branch**: `021-memory-crate` | **Date**: 2026-03-20

## Crate: `swink-agent-memory`

All public types are re-exported from `lib.rs`. Consumers use `use swink_agent_memory::*`.

---

### `SessionMeta`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

---

### `SessionStore` (sync trait)

```rust
pub trait SessionStore: Send + Sync {
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>;
    fn append(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>;
    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>;
    fn list(&self) -> io::Result<Vec<SessionMeta>>;
    fn delete(&self, id: &str) -> io::Result<()>;
}
```

**Invariants**:
- `id` is validated: rejects `/`, `\`, `..`, null bytes. Returns `io::ErrorKind::InvalidInput` on violation.
- `save` overwrites any existing session with the same ID.
- `append` updates the `updated_at` timestamp on the stored metadata.
- `load` returns `io::ErrorKind::NotFound` for missing sessions, `io::ErrorKind::InvalidData` for empty files.
- `CustomMessage` values in `messages` are silently filtered out (not persisted).

---

### `AsyncSessionStore` (async trait)

```rust
#[async_trait]
pub trait AsyncSessionStore: Send + Sync {
    async fn save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>;
    async fn append(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>;
    async fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>;
    async fn list(&self) -> io::Result<Vec<SessionMeta>>;
    async fn delete(&self, id: &str) -> io::Result<()>;
}
```

Same invariants as `SessionStore`.

---

### `JsonlSessionStore`

```rust
pub struct JsonlSessionStore { /* base_dir: PathBuf */ }

impl JsonlSessionStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> io::Result<Self>;
}

impl SessionStore for JsonlSessionStore { /* ... */ }
impl AsyncSessionStore for JsonlSessionStore { /* ... */ }
```

**Invariants**:
- `new` creates `base_dir` if it does not exist.
- Session files are stored as `{base_dir}/{id}.jsonl`.
- Corrupted JSONL lines are skipped with `tracing::warn!`; remaining messages are loaded.
- Append writes are atomic at the line level (single `write_all` call per line).

---

### `SummarizingCompactor`

```rust
pub struct SummarizingCompactor { /* ... */ }

impl SummarizingCompactor {
    pub fn new(tail_tokens: usize) -> Self;
    pub fn set_summary(&self, summary: String);
    pub fn compaction_fn(&self) -> impl Fn(Vec<LlmMessage>) -> Vec<LlmMessage>;
}
```

**Invariants**:
- `compaction_fn()` returns a closure compatible with `Agent::with_transform_context()`.
- If no summary has been set, the closure returns messages unchanged.
- If a summary exists, the closure injects it as an `AssistantMessage` at the start of the retained tail window and removes older messages.
- `set_summary` is thread-safe (interior `Arc<Mutex<>>`).
- The summary is consumed on use (reset to `None` after injection).

---

### `CompactionResult`

```rust
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub messages: Vec<LlmMessage>,
    pub removed_count: usize,
    pub summary: Option<String>,
}
```

---

### ID Validation (internal, enforced at trait boundary)

```rust
// Not public, but behavior is part of the contract:
// - Rejects IDs containing: '/', '\', "..", '\0'
// - Returns io::Error with ErrorKind::InvalidInput
// - Auto-generated IDs use format: YYYYMMDD_HHMMSS
```

---

### Timestamp Utilities

```rust
pub fn now_utc() -> DateTime<Utc>;
pub fn format_session_id() -> String;  // YYYYMMDD_HHMMSS
```

---

## Error Handling Summary

| Scenario | Error Kind | Message |
|----------|-----------|---------|
| Session not found | `NotFound` | `"session not found: {id}"` |
| Empty session file | `InvalidData` | `"empty session file"` |
| Invalid session ID | `InvalidInput` | `"invalid session id: {reason}"` |
| Filesystem full / permission denied | Propagated | OS-provided `io::Error` |
| Corrupted JSONL line | Warning only | `tracing::warn!`; line skipped, load continues |
