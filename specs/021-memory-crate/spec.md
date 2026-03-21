# Feature Specification: Memory Crate

**Feature Branch**: `021-memory-crate`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Session persistence and memory management. SessionStore trait (sync + async), JsonlSessionStore, JSONL serialization/deserialization of message logs, SummarizingCompactor with summary injection and sliding window wrapper, session metadata, timestamp utilities. References: HLD Memory Layer, Design Decisions (memory is a separate crate), PRD §2 (non-goals: advanced memory lives elsewhere).

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

### Edge Cases

- What happens when two processes attempt to save to the same session simultaneously — last-writer-wins; no file locking. Single-writer assumption is documented. Concurrent writes may corrupt the file.
- How does the store handle a session file that is completely empty (zero bytes) — returns `InvalidData` error ("empty session file").
- What happens when the compactor receives a conversation with only one message — within any budget; no compaction occurs (returned unchanged).
- How does the store behave when the underlying storage medium is full — OS `io::Error` propagates through `io::Result`; caller handles.
- What happens when a session file contains lines in an unrecognized format — corrupted/unrecognized lines are skipped with a warning logged; remaining messages are loaded successfully. Partial recovery over total failure.
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
- **FR-010**: The system MUST support appending messages to an existing session without rewriting the entire file.

### Key Entities

- **SessionStore**: The abstraction for persisting and retrieving conversation sessions. Supports save, load, list, and delete operations in both synchronous and asynchronous forms.
- **Session**: A persisted conversation containing a message log and associated metadata (identifier, title, creation timestamp, last-updated timestamp).
- **SummarizingCompactor**: The component that condenses older conversation messages into a summary, using a sliding window to retain recent messages verbatim.
- **SessionMetadata**: Descriptive information about a session, including its identifier, human-readable title, and timestamps.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Sessions survive process restarts — a saved session can be loaded in a new process with identical message content.
- **SC-002**: The summarizing compactor reduces a conversation exceeding the budget to within the budget while preserving a summary of removed content.
- **SC-003**: Async store operations do not block the calling thread.
- **SC-004**: Partial file corruption results in partial recovery, not total data loss.
- **SC-005**: Session metadata (title, timestamps) is preserved across save/load cycles.

## Clarifications

### Session 2026-03-20

- Q: How should concurrent session saves be handled? → A: Last-writer-wins; no file locking. Single-writer assumption documented.
- Q: Should user-provided session IDs be sanitized? → A: Yes — reject unsafe characters (`/`, `\`, `..`, null bytes) with an error.
- Q: Should corrupted JSONL lines fail the load or be skipped? → A: Skip bad lines, log warning, continue loading (partial recovery).
- Q: Empty session file? → A: Returns `InvalidData` error.
- Q: Compactor with one message? → A: No compaction; returned unchanged.
- Q: Storage medium full? → A: OS io::Error propagates; caller handles.
- Q: Summary marker collision? → A: No issue; summaries are regular AssistantMessage values.

## Assumptions

- The memory crate is a separate, independently publishable package with no dependency on the core agent crate's internals.
- The summarizing compactor requires a language model call to generate summaries — the compactor accepts a summarization function rather than embedding a specific provider.
- Advanced memory features (semantic search, long-term knowledge graphs) are out of scope for this crate, as stated in the PRD non-goals.
- The line-delimited store implementation targets local filesystem storage; remote storage backends are a future extension via the store abstraction.
- Timestamps use a standard representation that is both human-readable and sortable.
