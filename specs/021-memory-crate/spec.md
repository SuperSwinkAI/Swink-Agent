# Feature Specification: Memory Crate

**Feature Branch**: `021-memory-crate`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Session persistence and memory management. SessionStore trait (sync + async), JsonlSessionStore, JSONL serialization/deserialization of message logs, SummarizingCompactor with summary injection and sliding window wrapper, session metadata, timestamp utilities, rich session entry types, session versioning, interrupt state persistence, and filtered session retrieval. References: HLD Memory Layer, Design Decisions (memory is a separate crate), PRD §2 (non-goals: advanced memory lives elsewhere).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Save and Load Conversation Sessions (Priority: P1)

A developer runs an agent conversation and wants it persisted so it can be resumed later. After the conversation ends, the full message log is saved to a session store. When the developer returns, they load the session by its identifier and resume exactly where they left off, with the full conversation history intact. The session includes metadata such as a human-readable title, creation timestamp, and last-updated timestamp.

**Why this priority**: Persistence is the foundational capability of the memory crate — without save/load, no other memory feature can function.

**Independent Test**: Can be tested by creating a session with several messages, saving it, loading it back, and verifying the message log and metadata are identical.

**Acceptance Scenarios**:

1. **Given** an active conversation with multiple messages, **When** the session is saved, **Then** the full message log is persisted to the store.
2. **Given** a previously saved session, **When** it is loaded by its identifier, **Then** the returned message log matches what was saved, in order.
3. **Given** a saved session, **When** loaded, **Then** the metadata (title, creation timestamp, last-updated timestamp) is preserved.
4. **Given** a request to load a session that does not exist, **When** the load is attempted, **Then** a clear "not found" error is returned.

---

### User Story 2 - Compact Long Conversations via Summarization (Priority: P1)

A developer has a long-running conversation that has grown beyond what the context window can hold. The summarizing compactor condenses older portions of the conversation into a summary message, which is injected at the beginning of the retained window. The developer continues the conversation with a compact history that preserves the essential context while fitting within budget. The compactor uses a sliding window approach: recent messages are kept verbatim, older messages are replaced by the summary.

**Why this priority**: Without compaction, long conversations become unusable — the context window overflows and the agent cannot continue.

**Independent Test**: Can be tested by providing a long message log, running the compactor, and verifying the output contains a summary prefix followed by the most recent messages, with total size within the specified budget.

**Acceptance Scenarios**:

1. **Given** a conversation exceeding the context budget, **When** compaction runs, **Then** older messages are replaced by a summary and recent messages are retained verbatim.
2. **Given** a compacted conversation, **When** the summary is inspected, **Then** it captures the key topics and decisions from the removed messages.
3. **Given** a conversation within the context budget, **When** compaction is requested, **Then** the conversation is returned unchanged (no unnecessary summarization).
4. **Given** a compacted conversation, **When** the agent continues, **Then** the summary provides enough context for coherent follow-up responses.

---

### User Story 3 - Perform Store Operations Asynchronously (Priority: P2)

A developer integrates the session store into an asynchronous application (e.g., a server handling multiple concurrent conversations). The store supports async save, load, list, and delete operations so that I/O does not block the event loop. The developer can also use the synchronous interface when async is unnecessary.

**Why this priority**: Async support is essential for server-side and concurrent use cases, but the core save/load functionality works with the synchronous interface alone.

**Independent Test**: Can be tested by performing concurrent async save and load operations and verifying data integrity and non-blocking behavior.

**Acceptance Scenarios**:

1. **Given** an async runtime, **When** a session is saved asynchronously, **Then** the operation completes without blocking the event loop.
2. **Given** multiple concurrent async operations on different sessions, **When** they execute simultaneously, **Then** all operations complete successfully with correct data.
3. **Given** a synchronous context, **When** the synchronous store interface is used, **Then** save and load operations work correctly without requiring an async runtime.

---

### User Story 4 - List and Delete Sessions (Priority: P2)

A developer wants to browse previously saved sessions and clean up old ones. The store provides a listing of all sessions with their metadata (identifier, title, timestamps). The developer can delete sessions they no longer need, freeing storage.

**Why this priority**: Session management is important for long-term usability but is secondary to core save/load and compaction.

**Independent Test**: Can be tested by creating several sessions, listing them, verifying metadata, deleting one, and confirming it no longer appears in the listing.

**Acceptance Scenarios**:

1. **Given** multiple saved sessions, **When** the session list is requested, **Then** all sessions are returned with their metadata.
2. **Given** a saved session, **When** it is deleted, **Then** it no longer appears in listings or can be loaded.
3. **Given** an empty store, **When** the session list is requested, **Then** an empty list is returned.

---

### User Story 5 - Persist Sessions in Line-Delimited Data Format (Priority: P3)

A developer wants session data stored in a human-readable, line-delimited format where each line is a self-contained record. This makes sessions easy to inspect with standard text tools, append to incrementally, and recover partially if corruption occurs (only the corrupted line is lost, not the entire session). Each line represents one message in the conversation.

**Why this priority**: The specific storage format is an implementation concern — any working store satisfies the higher-priority stories. However, a line-delimited format provides practical benefits for debugging and partial recovery.

**Independent Test**: Can be tested by saving a session, inspecting the raw file to verify each line is a valid self-contained record, then corrupting one line and verifying the remaining messages are recoverable.

**Acceptance Scenarios**:

1. **Given** a saved session, **When** the raw storage is inspected, **Then** each message is a separate line that can be independently parsed.
2. **Given** a storage file with a corrupted line, **When** loaded, **Then** the non-corrupted messages are recovered and the corruption is reported.
3. **Given** a new message added to a session, **When** persisted, **Then** the message is appended without rewriting the entire file.

---

### User Story 6 - Rich Session Entry Types (Priority: P2) — I9

A developer saves non-message events to a session — model changes, thinking level changes, compaction events, user-defined labels (bookmarks), and custom structured data. Each entry carries a timestamp and is persisted alongside messages in the JSONL stream. Rich entries are NOT sent to the LLM — only `Message` entries go through `convert_to_llm`.

**Why this priority**: Rich entries provide a complete audit trail of what happened during a session, not just what was said. Without them, operators lose visibility into model switches, compaction decisions, and user annotations.

**Independent Test**: Can be tested by saving a session with mixed entry types (messages + model changes + labels), loading it, and verifying all entries are recovered in order with correct types and timestamps.

**Acceptance Scenarios**:

1. **Given** a session with `Message`, `ModelChange`, and `Label` entries, **When** saved and loaded, **Then** all entries are recovered in order with correct types and data.
2. **Given** a `SessionEntry::ModelChange` entry, **When** the session is sent to the LLM, **Then** the model change entry is NOT included in the LLM context.
3. **Given** a `SessionEntry::Compaction` entry recording that 15 messages were dropped, **When** loaded, **Then** the `dropped_count`, `tokens_before`, `tokens_after`, and `timestamp` are preserved.
4. **Given** a `SessionEntry::Custom` with arbitrary JSON data, **When** saved and loaded, **Then** the `type_name`, `data`, and `timestamp` are preserved.
5. **Given** an old-format session (raw `LlmMessage` lines without `entry_type`), **When** loaded, **Then** each line is interpreted as a `SessionEntry::Message` (backward compatible).

---

### User Story 7 - Session Versioning (Priority: P2) — I10

A developer uses session versioning to enable schema migration and detect concurrent modifications. `SessionMeta` includes a `version` field (schema version for migration) and a `sequence` field (monotonic counter for optimistic concurrency). When loading an older session, the store runs migrations to upgrade it to the current format. When saving, the store checks that the sequence hasn't been incremented by another writer.

**Why this priority**: Versioning prevents silent data loss when the session format evolves and enables multi-process safety without locking.

**Independent Test**: Can be tested by creating a session with version 1 format, loading it with a version-2 migrator, and verifying the session is upgraded correctly.

**Acceptance Scenarios**:

1. **Given** a new session, **When** saved, **Then** `version` is set to the current schema version and `sequence` is set to 1.
2. **Given** a saved session with sequence N, **When** saved again, **Then** `sequence` is incremented to N+1.
3. **Given** a session at version 1 and a migrator from v1→v2, **When** loaded, **Then** the session is transparently upgraded to version 2.
4. **Given** two writers saving with the same sequence number, **When** the second writer saves, **Then** a conflict error is returned (optimistic concurrency check).
5. **Given** an old session without `version` or `sequence` fields, **When** loaded, **Then** defaults are applied (`version: 1`, `sequence: 0`) — backward compatible.

---

### User Story 8 - Interrupt State Persistence (Priority: P2) — I11

An agent is interrupted at a tool approval gate or by cancellation. The interrupt state — pending tool calls, context snapshot, system prompt, and model — is persisted as a separate file so the agent can resume exactly where it left off after a restart. When a session is loaded and has a pending interrupt, the caller is notified so it can offer to resume.

**Why this priority**: Without interrupt persistence, any crash or restart during a tool approval gate loses the entire pending state, forcing the user to re-prompt.

**Independent Test**: Can be tested by saving an interrupt state with pending tool calls, restarting, loading the interrupt, and verifying all fields are recovered.

**Acceptance Scenarios**:

1. **Given** an interrupted agent with 2 pending tool calls, **When** `save_interrupt()` is called, **Then** the interrupt state is persisted as `{session_id}.interrupt.json`.
2. **Given** a session with a persisted interrupt, **When** `load_interrupt()` is called, **Then** the pending tool calls, context snapshot, system prompt, and model are returned.
3. **Given** a session without a persisted interrupt, **When** `load_interrupt()` is called, **Then** `None` is returned.
4. **Given** a resumed agent, **When** `clear_interrupt()` is called, **Then** the interrupt file is deleted.
5. **Given** an interrupt state file, **When** the session is deleted, **Then** the interrupt file is also deleted.

---

### User Story 9 - Filtered Session Retrieval (Priority: P3) — N12

A developer loads only a subset of a session's entries — the last N entries, entries after a timestamp, or entries of specific types. This avoids loading entire multi-thousand-entry sessions into memory when only recent context is needed.

**Why this priority**: Nice-to-have optimization for large sessions. Full loading works for most use cases, but filtered retrieval prevents memory issues with very long-running sessions.

**Independent Test**: Can be tested by saving a session with 100 entries, loading with `last_n_entries: Some(10)`, and verifying only the last 10 are returned.

**Acceptance Scenarios**:

1. **Given** a session with 100 entries, **When** loaded with `last_n_entries: Some(10)`, **Then** only the last 10 entries are returned.
2. **Given** a session with entries spanning timestamps T1–T100, **When** loaded with `after_timestamp: Some(T50)`, **Then** only entries after T50 are returned.
3. **Given** a session with mixed entry types, **When** loaded with `entry_types: Some(vec!["message"])`, **Then** only `Message` entries are returned.
4. **Given** `LoadOptions` with no filters set (all `None`), **When** loaded, **Then** the full session is returned (backward compatible).

---

### User Story 10 - Search Across Saved Sessions (Priority: P2) — N13

A developer searches persisted conversations for prior decisions, corrections, and notes without knowing which session contains the relevant entry. Search returns matching session entries with the session ID, title, score, and a compact snippet so callers can show useful recall results.

**Why this priority**: Cross-session recall makes saved sessions useful beyond exact restore flows and provides the retrieval surface needed by proactive memory features.

**Independent Test**: Can be tested by saving multiple sessions, searching for terms that appear in one session, and verifying only matching entries are returned with snippets and metadata.

**Acceptance Scenarios**:

1. **Given** multiple saved sessions, **When** searching for terms that appear in one session, **Then** only entries containing all query terms are returned.
2. **Given** search filters for session IDs, entry types, and a timestamp range, **When** search runs, **Then** hits outside those filters are excluded.
3. **Given** a maximum result count, **When** more entries match than the limit, **Then** only the top-scoring limited result set is returned.
4. **Given** a store implementation without search support, **When** `SessionStore::search()` is called, **Then** it returns an empty result set for backward compatibility.

---

### Edge Cases

- What happens when two processes attempt to save to the same session simultaneously — last-writer-wins; no file locking. Single-writer assumption is documented. Concurrent writes may corrupt the file.
- How does the store handle a session file that is completely empty (zero bytes) — returns `InvalidData` error ("empty session file").
- What happens when the compactor receives a conversation with only one message — within any budget; no compaction occurs (returned unchanged).
- How does the store behave when the underlying storage medium is full — OS `io::Error` propagates through `io::Result`; caller handles.
- What happens when a session file contains lines in an unrecognized format — corrupted/unrecognized lines are skipped with a warning logged; remaining messages are loaded successfully. Partial recovery over total failure.
- What happens when a `SessionEntry::Custom` has a `type_name` that conflicts with built-in types — `type_name` is a free-form string; the `entry_type` discriminator in JSONL uses fixed values (`message`, `model_change`, `compaction`, `label`, `custom`). Custom type names live inside the `Custom` variant and cannot collide.
- What happens when a session has an interrupt file but the interrupt state is corrupted — `load_interrupt()` returns an `InvalidData` error. The caller decides whether to clear it and start fresh.
- What happens when `load_interrupt()` is called for a session that doesn't exist — returns `None` (not an error). The interrupt file can't exist without the session.
- What happens when `last_n_entries` is larger than the session — the full session is returned (no error).
- What happens when a session has a version higher than the current code supports — returns an error indicating unsupported version. Future-version sessions cannot be silently downgraded.
- How does the compactor handle messages that contain the summary marker — summaries are injected as regular `AssistantMessage` values, not specially marked. No collision issue; summary is just a message in the conversation.
- What happens when the session identifier contains filesystem-unsafe characters — IDs are validated; unsafe characters (`/`, `\`, `..`, null bytes) are rejected with an error. Auto-generated IDs use safe `YYYYMMDD_HHMMSS` format.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST provide a session store abstraction with save, load, list, and delete operations.
- **FR-002**: The system MUST support both synchronous and asynchronous store operations.
- **FR-003**: The system MUST serialize message logs in a line-delimited format where each line is a self-contained record.
- **FR-004**: The system MUST deserialize message logs, tolerating individual corrupted lines without losing the entire session.
- **FR-005**: The system MUST support a summarizing compactor that replaces older messages with a summary while retaining recent messages verbatim.
- **FR-006**: The compactor MUST inject the summary as a message at the beginning of the retained window.
- **FR-007**: The compactor MUST skip compaction when the conversation is already within the context budget.
- **FR-008**: The system MUST associate metadata with each session: identifier, title, creation timestamp, and last-updated timestamp.
- **FR-009**: The system MUST return a clear error when attempting to load a non-existent session.
- **FR-010**: The system MUST support appending messages to an existing session without rewriting the entire file. When `save()` rewrites an existing session file, it preserves non-Message rich entry types (`ModelChange`, `Label`, `Compaction`, `Custom`, and `State` entries) while replacing Message entries. `save_entries()` preserves only `State` entries during rewrite.
- **FR-011**: The system MUST support rich session entry types: `Message`, `ModelChange`, `ThinkingLevelChange`, `Compaction`, `Label`, and `Custom`, each with a timestamp.
- **FR-012**: Rich entry types (non-Message) MUST NOT be sent to the LLM — only `Message` entries participate in `convert_to_llm`.
- **FR-013**: The JSONL format MUST use an `entry_type` discriminator field for rich entries, with backward compatibility for old-format sessions (lines without `entry_type` are interpreted as `Message`).
- **FR-014**: `SessionMeta` MUST include `version: u32` (schema version) and `sequence: u64` (monotonic counter for optimistic concurrency).
- **FR-015**: The system MUST support session migration via a `SessionMigrator` trait, running migrations transparently on load when the session version is older than the current version.
- **FR-016**: The system MUST reject saves where the session's `sequence` does not match the stored sequence (optimistic concurrency conflict detection).
- **FR-017**: The system MUST support persisting and loading interrupt state (pending tool calls, context snapshot, system prompt, model) as a separate file alongside the session.
- **FR-018**: `save_interrupt`, `load_interrupt`, and `clear_interrupt` MUST be added to the `SessionStore` trait.
- **FR-019**: Session deletion MUST also delete any associated interrupt state file.
- **FR-020**: The system MUST support filtered session loading via `LoadOptions` with `last_n_entries`, `after_timestamp`, and `entry_types` filter parameters.
- **FR-021**: `LoadOptions` with all fields `None` MUST return the full session (backward compatible).
- **FR-022**: The system MUST expose `SessionStore::search(query, options)` with a default empty-result implementation for backward compatibility.
- **FR-023**: The JSONL store MUST support cross-session term search over persisted `SessionEntry` values.
- **FR-024**: Search options MUST support session ID filters, entry type filters, timestamp range filters, and a maximum result count.
- **FR-025**: Search hits MUST include session ID, session title, matched entry, score, and snippet.

### Key Entities

- **SessionStore**: The abstraction for persisting and retrieving conversation sessions. Supports save, load, list, and delete operations in both synchronous and asynchronous forms.
- **Session**: A persisted conversation containing a message log and associated metadata (identifier, title, creation timestamp, last-updated timestamp).
- **SummarizingCompactor**: The component that condenses older conversation messages into a summary, using a sliding window to retain recent messages verbatim.
- **SessionMetadata**: Descriptive information about a session, including its identifier, human-readable title, timestamps, schema version, and sequence counter.
- **SessionEntry**: Discriminated union of entry types persisted in a session: `Message`, `ModelChange`, `ThinkingLevelChange`, `Compaction`, `Label`, `Custom`.
- **InterruptState**: Snapshot of agent state at an interrupt point: pending tool calls, context, system prompt, model.
- **PendingToolCall**: A tool call awaiting approval: tool call ID, name, arguments.
- **SessionMigrator**: Trait for upgrading old session formats to the current version.
- **LoadOptions**: Filter parameters for partial session loading.
- **SessionSearchOptions**: Filter and limit parameters for cross-session search.
- **SessionHit**: A search result containing matched session metadata, entry, score, and snippet.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Sessions survive process restarts — a saved session can be loaded in a new process with identical message content.
- **SC-002**: The summarizing compactor reduces a conversation exceeding the budget to within the budget while preserving a summary of removed content.
- **SC-003**: Async store operations do not block the calling thread.
- **SC-004**: Partial file corruption results in partial recovery, not total data loss.
- **SC-005**: Session metadata (title, timestamps) is preserved across save/load cycles.
- **SC-006**: Rich session entries (ModelChange, Label, Compaction, Custom) survive save/load roundtrips with correct types, data, and timestamps.
- **SC-007**: Old-format sessions (without `entry_type` discriminator or version/sequence fields) load successfully with defaults applied.
- **SC-008**: Interrupt state survives process restart — saved interrupt can be loaded in a new process with all pending tool calls and context intact.
- **SC-009**: Filtered loading with `last_n_entries` returns exactly the requested number of entries (or fewer if the session is smaller).
- **SC-010**: Optimistic concurrency check rejects saves with stale sequence numbers.
- **SC-011**: Cross-session search returns matching persisted entries while respecting filters and result limits.

## Clarifications

### Session 2026-03-20

- Q: How should concurrent session saves be handled? → A: Last-writer-wins; no file locking. Single-writer assumption documented.
- Q: Should user-provided session IDs be sanitized? → A: Yes — reject unsafe characters (`/`, `\`, `..`, null bytes) with an error.
- Q: Should corrupted JSONL lines fail the load or be skipped? → A: Skip bad lines, log warning, continue loading (partial recovery).
- Q: Empty session file? → A: Returns `InvalidData` error.
- Q: Compactor with one message? → A: No compaction; returned unchanged.
- Q: Storage medium full? → A: OS io::Error propagates; caller handles.
- Q: Summary marker collision? → A: No issue; summaries are regular AssistantMessage values.

### Session 2026-03-31

- Q: How should optimistic concurrency checking work in `save()`? → A: Always check — compare `meta.sequence` against the stored file's sequence. If they don't match, the save is rejected with an error. No API change needed; callers use the sequence from their loaded `SessionMeta`, which matches unless another writer intervened. New sessions (no existing file) skip the check.
- Q: Should `SessionEntry::Message` flatten `AgentMessage` fields or nest under a `data` key? → A: Nest — `{"entry_type": "message", "data": {AgentMessage fields}}`. Avoids field name collisions with `entry_type` and simplifies backward compatibility (old lines without `entry_type` are raw `AgentMessage`, distinct from nested format).

### Session 2026-04-15

- **Custom message persistence**: `SessionStore::save()` and `load()` operate on `AgentMessage` (not `LlmMessage`), which includes `AgentMessage::Custom` variants. Custom messages are serialized with a `_custom: true` marker in JSONL. Deserialization requires a `CustomMessageRegistry` at load time — without one, custom messages cannot be restored. Callers must supply a compatible registry when loading sessions that contain custom messages.

## Assumptions

- The memory crate is a separate, independently publishable package with no dependency on the core agent crate's internals.
- The summarizing compactor requires a language model call to generate summaries — the compactor accepts a summarization function rather than embedding a specific provider.
- Advanced memory features (semantic search, long-term knowledge graphs) are out of scope for this crate, as stated in the PRD non-goals.
- The line-delimited store implementation targets local filesystem storage; remote storage backends are a future extension via the store abstraction.
- Timestamps use a standard representation that is both human-readable and sortable.
- Rich entry types are an extension of the existing JSONL format — backward compatibility with old sessions is mandatory.
- Interrupt state is stored as a separate JSON file (`{session_id}.interrupt.json`), not inline in the JSONL stream, because the interrupt state is transient and should not be part of the permanent session log.
- Session version starts at 1 for the original format. The current schema version is a constant in the crate.
- The `sequence` field is incremented on every write (save or append). Optimistic concurrency is always checked by comparing `meta.sequence` against the stored sequence — callers use the loaded sequence, which matches unless another writer intervened. New sessions skip the check.
- `LoadOptions` filtering is best-effort for JSONL — `last_n_entries` may require reading from the end of the file. Implementations may read the entire file and filter in memory if tail-reading is impractical.
