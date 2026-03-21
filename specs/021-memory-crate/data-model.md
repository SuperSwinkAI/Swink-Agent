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

### SessionStore (trait, sync)

Synchronous session persistence abstraction.

| Method | Signature | Description |
|--------|-----------|-------------|
| `save` | `(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>` | Persist a full session (metadata + messages). Overwrites existing. |
| `append` | `(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>` | Append messages to an existing session. Updates `updated_at`. |
| `load` | `(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>` | Load a session by ID. Returns `NotFound` if missing, `InvalidData` if empty. |
| `list` | `(&self) -> io::Result<Vec<SessionMeta>>` | List all sessions with metadata. |
| `delete` | `(&self, id: &str) -> io::Result<()>` | Delete a session by ID. |

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
```

## JSONL File Layout

```
{"id":"20260320_143000","title":"Debug session","created_at":"2026-03-20T14:30:00Z","updated_at":"2026-03-20T15:00:00Z"}
{"type":"user","content":"How do I fix this error?"}
{"type":"assistant","content":"The error is caused by...","stop_reason":"end_turn"}
{"type":"user","content":"Thanks, what about the warning?"}
{"type":"assistant","content":"That warning means...","stop_reason":"end_turn"}
```
