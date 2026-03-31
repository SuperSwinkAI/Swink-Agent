# Data Model: Memory Crate

**Branch**: `021-memory-crate` | **Date**: 2026-03-20

## Entities

### SessionMeta

Descriptive information about a persisted session.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique session identifier. Validated: no `/`, `\`, `..`, null bytes. Auto-generated format: `YYYYMMDD_HHMMSS`. |
| `title` | `String` | Human-readable session title. |
| `created_at` | `DateTime<Utc>` | Timestamp when the session was first created. |
| `updated_at` | `DateTime<Utc>` | Timestamp of the most recent save. Updated on every save. |
| `version` | `u32` | Schema version for migration (default: `1`, `#[serde(default = "default_version")]`). |
| `sequence` | `u64` | Monotonic counter incremented on every write (default: `0`, `#[serde(default)]`). |

### SessionStore (trait, sync)

Synchronous session persistence abstraction.

| Method | Signature | Description |
|--------|-----------|-------------|
| `save` | `(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>` | Persist a full session (metadata + messages). Overwrites existing. |
| `append` | `(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>` | Append messages to an existing session. Updates `updated_at`. |
| `load` | `(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>` | Load a session by ID. Returns `NotFound` if missing, `InvalidData` if empty. |
| `list` | `(&self) -> io::Result<Vec<SessionMeta>>` | List all sessions with metadata. |
| `delete` | `(&self, id: &str) -> io::Result<()>` | Delete a session by ID (also deletes interrupt file). |
| `save_interrupt` | `(&self, id: &str, state: &InterruptState) -> io::Result<()>` | Persist interrupt state for a session. |
| `load_interrupt` | `(&self, id: &str) -> io::Result<Option<InterruptState>>` | Load interrupt state, or `None` if none exists. |
| `clear_interrupt` | `(&self, id: &str) -> io::Result<()>` | Delete the interrupt state file. |

### AsyncSessionStore (trait, async)

Asynchronous session persistence abstraction. Same methods as `SessionStore` but returning `Future`s.

| Method | Signature | Description |
|--------|-----------|-------------|
| `save` | `async fn(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>` | Async persist. |
| `append` | `async fn(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>` | Async append. |
| `load` | `async fn(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>` | Async load. |
| `list` | `async fn(&self) -> io::Result<Vec<SessionMeta>>` | Async list. |
| `delete` | `async fn(&self, id: &str) -> io::Result<()>` | Async delete. |

### JsonlSessionStore

Concrete `SessionStore` + `AsyncSessionStore` implementation using JSONL files on the local filesystem.

| Field | Type | Description |
|-------|------|-------------|
| `base_dir` | `PathBuf` | Directory where session files are stored. Each session is `{base_dir}/{id}.jsonl`. |

**File format**: Line 1 = JSON-serialized `SessionMeta`. Lines 2+ = JSON-serialized `LlmMessage` (one per line). `CustomMessage` values are filtered out (not serialized).

**Corruption handling**: On load, lines that fail to parse are skipped with a `tracing::warn!`. If all lines fail or the file is empty, returns `io::ErrorKind::InvalidData`.

### SummarizingCompactor

Orchestrates context compaction using a sliding window and externally provided summarization.

| Field | Type | Description |
|-------|------|-------------|
| `summary` | `Arc<Mutex<Option<String>>>` | Pre-computed summary, set asynchronously after each turn. |
| `tail_tokens` | `usize` | Number of tokens (estimated) to retain as the recent window. |

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(tail_tokens: usize) -> Self` | Create a compactor with the given tail window size. |
| `set_summary` | `(&self, summary: String)` | Store a pre-computed summary for the next compaction. |
| `compaction_fn` | `(&self) -> impl Fn(Vec<LlmMessage>) -> Vec<LlmMessage>` | Returns a closure suitable for `Agent::with_transform_context()`. If a summary exists, injects it as an `AssistantMessage` at the start of the retained tail window. |

### CompactionResult

Output of a compaction operation.

| Field | Type | Description |
|-------|------|-------------|
| `messages` | `Vec<LlmMessage>` | The compacted message list (summary + tail). |
| `removed_count` | `usize` | Number of messages replaced by the summary. |
| `summary` | `Option<String>` | The generated summary text, if compaction occurred. |

### SessionEntry (enum)

Discriminated union of entry types persisted in a session. Used in JSONL serialization with an `entry_type` field.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Message` | `AgentMessage` | An LLM message (user, assistant, tool result). The primary entry type. |
| `ModelChange` | `from: ModelSpec, to: ModelSpec, timestamp: u64` | Records a model switch during the session. |
| `ThinkingLevelChange` | `from: String, to: String, timestamp: u64` | Records a thinking level change. |
| `Compaction` | `dropped_count: usize, tokens_before: usize, tokens_after: usize, timestamp: u64` | Records a context compaction event. |
| `Label` | `text: String, message_index: usize, timestamp: u64` | A user bookmark/annotation on a specific message. |
| `Custom` | `type_name: String, data: Value, timestamp: u64` | Arbitrary structured data for extensibility. |

**Derives**: `Debug`, `Clone`, `Serialize`, `Deserialize`.
**Serde**: Adjacently tagged via `#[serde(tag = "entry_type", content = "data", rename_all = "snake_case")]`. The `Message` variant nests the `AgentMessage` under the `data` key: `{"entry_type": "message", "data": {...}}`. Old-format lines (without `entry_type`) are deserialized as `Message` via a custom fallback.

---

### InterruptState (struct)

Snapshot of agent state at an interrupt point, persisted as a separate JSON file.

| Field | Type | Description |
|-------|------|-------------|
| `interrupted_at` | `u64` | Unix timestamp of the interrupt. |
| `pending_tool_calls` | `Vec<PendingToolCall>` | Tool calls waiting for approval. |
| `context_snapshot` | `Vec<AgentMessage>` | Full conversation context at interrupt point. |
| `system_prompt` | `String` | Active system prompt. |
| `model` | `ModelSpec` | Active model at interrupt time. |

**Derives**: `Debug`, `Clone`, `Serialize`, `Deserialize`.

---

### PendingToolCall (struct)

A tool call that was awaiting approval at interrupt time.

| Field | Type | Description |
|-------|------|-------------|
| `tool_call_id` | `String` | Unique tool call ID. |
| `tool_name` | `String` | Name of the tool being called. |
| `arguments` | `Value` | Arguments passed to the tool. |

**Derives**: `Debug`, `Clone`, `Serialize`, `Deserialize`.

---

### SessionMigrator (trait)

Trait for upgrading sessions from one schema version to another.

| Method | Signature | Description |
|--------|-----------|-------------|
| `source_version` | `&self -> u32` | Version this migrator upgrades from. |
| `target_version` | `&self -> u32` | Version this migrator upgrades to. |
| `migrate` | `&self, meta: &mut SessionMeta, entries: &mut Vec<SessionEntry>) -> io::Result<()>` | Transform entries in place. |

---

### LoadOptions (struct)

Filter parameters for partial session loading.

| Field | Type | Description |
|-------|------|-------------|
| `last_n_entries` | `Option<usize>` | Only return the last N entries. |
| `after_timestamp` | `Option<u64>` | Only return entries after this Unix timestamp. |
| `entry_types` | `Option<Vec<String>>` | Only return entries matching these type names. |

**Derives**: `Debug`, `Clone`, `Default`.

---

## Relationships

```
SessionStore (trait)
    ^
    |  implements
JsonlSessionStore ----uses----> SessionMeta
    |
    v  implements
AsyncSessionStore (trait)

SummarizingCompactor ----produces----> CompactionResult
    |
    v  returns closure for
Agent::with_transform_context()

SessionEntry (enum)
    ├── Message(AgentMessage)       -- the only variant sent to LLM
    ├── ModelChange                 -- audit/display only
    ├── ThinkingLevelChange         -- audit/display only
    ├── Compaction                  -- audit/display only
    ├── Label                       -- user bookmark
    └── Custom                      -- extensibility

InterruptState ----persisted-as----> {session_id}.interrupt.json
    ├── pending_tool_calls: Vec<PendingToolCall>
    ├── context_snapshot: Vec<AgentMessage>
    └── model: ModelSpec

SessionMigrator ----upgrades----> SessionMeta + Vec<SessionEntry>
LoadOptions ----filters----> SessionStore::load() output
```

## JSONL File Layout

```
{"id":"20260320_143000","title":"Debug session","created_at":"2026-03-20T14:30:00Z","updated_at":"2026-03-20T15:00:00Z"}
{"type":"user","content":"How do I fix this error?"}
{"type":"assistant","content":"The error is caused by...","stop_reason":"end_turn"}
{"type":"user","content":"Thanks, what about the warning?"}
{"type":"assistant","content":"That warning means...","stop_reason":"end_turn"}
```
