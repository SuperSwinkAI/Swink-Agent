# Feature Specification: Artifact Service

**Feature Branch**: `036-artifact-service`  
**Created**: 2026-04-01  
**Status**: Draft  
**Input**: User description: "Artifact Service — session-attached versioned artifact storage for agent-produced outputs (files, reports, images, data exports)"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Agent Tool Saves a Generated File as an Artifact (Priority: P1)

A library consumer builds an agent with a code-generation tool. When the tool produces a source file, it saves the output as a named artifact attached to the current session. The artifact is versioned — if the tool generates an updated version of the same file on a subsequent turn, a new version is created rather than overwriting the original. The consumer can retrieve any version after the conversation ends.

**Why this priority**: This is the core use case that has no current solution. Today tools can only return text/content blocks in messages — they cannot persist binary or large outputs in a structured, versioned way tied to the session.

**Independent Test**: Can be fully tested by creating a mock tool that saves an artifact via the artifact store, running a multi-turn conversation where the tool saves two versions of the same artifact, and verifying both versions are independently retrievable.

**Acceptance Scenarios**:

1. **Given** an agent with artifact storage enabled, **When** a tool saves an artifact named "report.md" with text content, **Then** the artifact is stored as version 1 and an `ArtifactSaved` event is emitted with the artifact name and version number.
2. **Given** an agent with an existing artifact "report.md" at version 1, **When** a tool saves a new version of "report.md", **Then** the artifact is stored as version 2 and the original version 1 remains accessible.
3. **Given** an agent with an artifact "report.md" at version 3, **When** a consumer loads the artifact without specifying a version, **Then** the latest version (3) is returned.
4. **Given** an agent with an artifact "report.md" at version 3, **When** a consumer loads version 1 explicitly, **Then** version 1 is returned with its original content.

---

### User Story 2 - Agent Lists and Loads Artifacts from a Session (Priority: P1)

A library consumer runs an agent that produces multiple artifacts over the course of a conversation (e.g., a research report, extracted data CSV, a summary image). After the conversation, the consumer lists all artifacts in the session to display them in a UI or export them. Each artifact's metadata (name, version count, content type, size) is available without loading the full content.

**Why this priority**: Retrieval and discovery are essential complements to storage. Without listing and loading, artifacts are write-only — usable only if the consumer already knows the exact artifact names.

**Independent Test**: Can be fully tested by saving three artifacts with different content types, calling list, and verifying all three appear with correct metadata. Then loading each by name and verifying content integrity.

**Acceptance Scenarios**:

1. **Given** a session with artifacts "code.rs", "data.csv", and "chart.png", **When** the consumer lists artifacts for the session, **Then** all three appear with their names, latest version numbers, content types, and creation/update timestamps.
2. **Given** a session with no artifacts, **When** the consumer lists artifacts, **Then** an empty list is returned (not an error).
3. **Given** a session with an artifact "data.csv" (content type "text/csv", 2048 bytes), **When** the consumer loads the artifact, **Then** the returned data includes the raw bytes, content type, and any custom metadata that was stored with it.

---

### User Story 3 - Artifact Store Persists Across Session Boundaries (Priority: P1)

A library consumer saves a session and later restores it. All artifacts that were saved during the original session are still accessible after restore. Artifact storage is independent of the conversation message store — artifacts are not serialized into the JSONL message stream.

**Why this priority**: Artifacts that disappear on session restore are useless for any workflow that spans multiple sessions or requires post-hoc access. Independence from the message store is critical because artifacts can be large (images, data files) and would bloat the message stream.

**Independent Test**: Can be fully tested by saving artifacts during a session, ending the session, creating a new agent with the same session ID and artifact store, and verifying all artifacts are loadable with correct content.

**Acceptance Scenarios**:

1. **Given** a session with artifact "output.json" at version 2, **When** the session is ended and a new agent is created with the same session ID and artifact store, **Then** "output.json" versions 1 and 2 are both accessible.
2. **Given** an artifact store with no data for a given session ID, **When** the consumer lists or loads artifacts, **Then** an empty list or `None` is returned (backward compatible, no error).
3. **Given** a large artifact (multi-megabyte image), **When** it is saved and later loaded, **Then** the content is byte-for-byte identical to the original.

---

### User Story 4 - LLM Agent Uses Built-in Tools to Manage Artifacts (Priority: P2)

A library consumer configures an agent with built-in artifact tools so the LLM can autonomously save and retrieve artifacts during a conversation. The LLM calls a save-artifact tool to persist generated content and a load-artifact tool to retrieve previously saved content for reference or further processing. A list-artifacts tool lets the LLM discover what artifacts exist in the current session.

**Why this priority**: Built-in tools make artifacts a first-class capability that the LLM can use without custom tool development. However, the underlying storage API (P1) must exist first — tools are a convenience layer on top.

**Independent Test**: Can be fully tested by configuring an agent with the built-in artifact tools, sending a prompt that triggers artifact creation, and verifying the tool call succeeds and the artifact is retrievable.

**Acceptance Scenarios**:

1. **Given** an agent with built-in artifact tools enabled, **When** the LLM calls the save-artifact tool with a name, content, and content type, **Then** the artifact is saved and the tool returns a confirmation with the version number.
2. **Given** an agent with a saved artifact "notes.md", **When** the LLM calls the load-artifact tool with name "notes.md", **Then** the tool returns the artifact content as text (for text content types) or a size/type summary (for binary content types).
3. **Given** an agent with multiple saved artifacts, **When** the LLM calls the list-artifacts tool, **Then** the tool returns a formatted list of artifact names, versions, and content types.
4. **Given** an agent without artifact tools enabled, **When** the agent runs, **Then** no artifact tools appear in the tool list and the agent functions normally.

---

### User Story 5 - Streaming Large Artifacts (Priority: P2)

A library consumer works with large artifacts (multi-megabyte data exports, images, binary files). The artifact store supports efficient I/O without requiring the entire artifact content to be held in memory at once during save or load operations.

**Why this priority**: Memory efficiency is critical for production use where agents may produce large outputs. Without streaming support, a 100MB data export would require 100MB+ of heap allocation just for the save operation.

**Independent Test**: Can be fully tested by saving a large artifact (e.g., 10MB of generated data) and verifying that peak memory usage during the operation is significantly less than the artifact size.

**Acceptance Scenarios**:

1. **Given** an artifact store implementation that supports streaming, **When** a 10MB artifact is saved, **Then** the save operation does not require the entire 10MB to be in memory simultaneously.
2. **Given** an artifact store implementation that supports streaming, **When** a large artifact is loaded, **Then** the content can be consumed incrementally rather than as a single allocation.
3. **Given** a simple artifact store implementation (e.g., in-memory for testing), **When** it does not support streaming, **Then** the non-streaming API (full byte vector in/out) still works correctly — streaming is an optimization, not a requirement for all implementations.

---

### User Story 6 - Artifact Deletion (Priority: P3)

A library consumer wants to remove artifacts that are no longer needed — either to free storage or to clean up before exporting. Deletion removes all versions of a named artifact from the session.

**Why this priority**: Deletion is a lifecycle management convenience. Most workflows will produce and retrieve artifacts without needing to delete them. It rounds out the API but is not required for the core value proposition.

**Independent Test**: Can be fully tested by saving an artifact with multiple versions, deleting it, and verifying that list no longer includes it and load returns `None`.

**Acceptance Scenarios**:

1. **Given** a session with artifact "draft.md" at version 3, **When** the consumer deletes "draft.md", **Then** all three versions are removed and subsequent load returns `None`.
2. **Given** a session with no artifact named "nonexistent", **When** the consumer deletes "nonexistent", **Then** the operation succeeds silently (idempotent, not an error).

---

### Edge Cases

- What happens when an artifact name contains path separators or special characters? Names are validated on save — only alphanumeric characters, hyphens, underscores, dots, and forward slashes are allowed. Invalid names return an error.
- What happens when a consumer requests a version that does not exist? Load returns `None` (not an error), consistent with the pattern for missing artifacts.
- What happens when two tools save to the same artifact name concurrently? Versioning is serialized — both saves succeed with sequential version numbers. The order is non-deterministic but both versions are preserved.
- What happens when the underlying storage (e.g., filesystem) is unavailable or full? Save/load/list/delete return an error. The artifact store does not retry — error handling is the consumer's responsibility.
- What happens when artifact content is empty (zero bytes)? Empty artifacts are valid. A zero-byte artifact is saved and loaded correctly with size 0.
- What happens when the artifact store is not configured on the agent? Tools that require artifact access receive no store reference and return an error indicating artifacts are not enabled. The agent loop itself is unaffected.

## Clarifications

**Internal types in public surface**: The `swink-agent-artifacts` crate exposes `MetaFile`, `VersionRecord`, and the `storage_err()` helper as `pub` items to satisfy Rust visibility requirements. These are implementation details of the storage layer and are not part of the stable consumer API — consumers interact via the `ArtifactStore` and `StreamingArtifactStore` traits only.

### Session 2026-04-02

- Q: How should `ArtifactSaved` integrate with the event system? → A: New `AgentEvent::ArtifactSaved` variant on the existing enum, behind the feature gate.
- Q: How should streaming I/O be exposed for implementations that support it? → A: Separate `StreamingArtifactStore` extension trait; base `ArtifactStore` uses `Vec<u8>` only.
- Q: How should artifact tool feature gating relate to existing gates? → A: New `artifact-tools` feature (depends on `artifact-store` feature); independent of `builtin-tools`.
- Q: Should the `ArtifactStore` trait require `Send + Sync`? → A: Yes, `ArtifactStore: Send + Sync` required, matching existing trait patterns (`AgentTool`, policy traits).
- Q: How do tools get access to the artifact store at execution time? → A: Tools capture `Arc<dyn ArtifactStore>` at construction time; no changes to the core tool execution context.
- Q: Should the artifacts crate ship a built-in `InMemoryArtifactStore`? → A: Yes, always available (not feature-gated), matching project patterns like `InMemoryVersionStore`.
- Q: Should artifact store operations emit tracing spans? → A: Implementations emit `tracing` spans/events; not a trait requirement.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide an `ArtifactStore` trait with operations: save, load, list, and delete, scoped by session ID and artifact name.
- **FR-002**: Save MUST create a new version of the named artifact (append-only versioning). Existing versions MUST NOT be mutated or overwritten.
- **FR-003**: Load MUST support loading a specific version by number or loading the latest version when no version is specified. Loading a nonexistent artifact or version MUST return `None`.
- **FR-004**: List MUST return metadata for all artifacts in a session without loading artifact content. Metadata MUST include artifact name, latest version number, content type, and creation/update timestamps.
- **FR-005**: Delete MUST remove all versions of a named artifact. Deleting a nonexistent artifact MUST succeed silently (idempotent).
- **FR-006**: Artifact data MUST include raw byte content, a MIME content type string, and an arbitrary string-to-string metadata map for consumer-defined key-value pairs.
- **FR-007**: Each saved version MUST be described by a version record containing: artifact name, version number (monotonically increasing per artifact per session, starting at 1), creation timestamp, byte size, and content type.
- **FR-008**: Artifact names MUST be validated: only alphanumeric characters, hyphens, underscores, dots, and forward slashes are allowed. Names MUST NOT be empty. Invalid names MUST return an error on save.
- **FR-009**: The `ArtifactStore` trait methods MUST be asynchronous and the trait MUST require `Send + Sync` bounds to support concurrent tool execution and multi-agent scenarios.
- **FR-010**: A built-in filesystem-backed implementation MUST be provided that stores artifacts as files organized by session ID and artifact name, with version-numbered filenames and a metadata sidecar file.
- **FR-011**: The filesystem implementation MUST handle concurrent access safely — concurrent saves to the same artifact MUST produce sequential version numbers without data corruption.
- **FR-012**: The artifact store MUST be injectable into the agent via configuration, similar to how session stores are configured. If no artifact store is configured, artifact functionality is unavailable but the agent operates normally.
- **FR-013**: Built-in artifact tools (save, load, list) MUST be provided that allow the LLM to manage artifacts during a conversation. These tools MUST be feature-gated independently from the core artifact store trait. Tools MUST capture `Arc<dyn ArtifactStore>` at construction time rather than receiving it via the tool execution context.
- **FR-018**: An `InMemoryArtifactStore` implementation MUST be provided in the artifacts crate, always available (not feature-gated), for use in tests and lightweight scenarios.
- **FR-019**: Built-in artifact store implementations (`FileArtifactStore`, `InMemoryArtifactStore`) MUST emit `tracing` spans and events for diagnostic observability. Tracing is NOT a requirement of the `ArtifactStore` trait itself.
- **FR-014**: An `ArtifactSaved` event MUST be emitted as a new variant on the existing `AgentEvent` enum (behind the `artifact-store` feature gate) whenever an artifact version is successfully saved, containing the session ID, artifact name, and version number.
- **FR-015**: The artifact store MUST operate independently of the session message store. Artifacts MUST NOT be serialized into the conversation message stream.
- **FR-016**: The `ArtifactStore` trait MUST support efficient handling of large artifacts. The base trait MUST work with full byte vectors (`Vec<u8>`) as the minimum API surface. A separate `StreamingArtifactStore` extension trait MUST be defined for implementations that support incremental I/O; implementing it is optional.
- **FR-017**: The artifact service MUST be feature-gated with an `artifact-store` feature for the core trait and storage. Built-in artifact tools MUST be gated under a separate `artifact-tools` feature that depends on `artifact-store` and is independent of the existing `builtin-tools` feature.

### Key Entities

- **ArtifactStore**: The pluggable storage trait — abstraction over where and how artifacts are persisted. Implementations may target local filesystem, cloud storage, or in-memory (for testing).
- **ArtifactData**: The content payload for an artifact — raw bytes, MIME content type, and custom metadata. Represents what is being stored.
- **ArtifactVersion**: A record describing a specific saved version — name, version number, timestamp, size, content type. Returned on save as confirmation. Used for version discovery.
- **ArtifactMeta**: Summary metadata for an artifact across all its versions — name, latest version number, creation and update timestamps. Used in list results without loading content.
- **FileArtifactStore**: The built-in filesystem implementation. Organizes artifacts as versioned files under a configurable root directory.
- **InMemoryArtifactStore**: A built-in in-memory implementation for testing and lightweight use. Always available (not feature-gated).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Agents can produce and persist versioned outputs (files, reports, data exports) that survive beyond the conversation turn in which they were created.
- **SC-002**: Consumers can discover and retrieve any version of any artifact from a session without replaying the conversation.
- **SC-003**: Artifacts persist independently across session save/load cycles with byte-for-byte content integrity.
- **SC-004**: The LLM can autonomously save, list, and load artifacts during a conversation using built-in tools without custom tool development by the consumer.
- **SC-005**: Large artifacts (10MB+) can be saved and loaded without proportional heap allocation in implementations that support streaming.
- **SC-006**: Concurrent tool executions saving artifacts to the same session produce no data corruption or lost versions.
- **SC-007**: Agents without artifact storage configured experience no behavioral change, no additional dependencies, and no measurable overhead.
- **SC-008**: A library consumer can add artifact support to an existing agent by providing an artifact store implementation with no changes to loop configuration or stream function.

## Assumptions

- Artifacts are opaque byte sequences from the framework's perspective. The framework stores and retrieves them but does not interpret content beyond MIME type classification.
- The artifact store is per-session but shared across all tools within that session. There is no per-tool artifact namespace — all tools see the same artifact space. Consumers can adopt naming conventions (e.g., `tool_name/artifact`) but the system does not enforce them.
- Versioning is per-artifact-per-session. Version numbers are simple monotonically increasing integers (1, 2, 3, ...) — not UUIDs or content hashes. There is no cross-session versioning.
- Artifact storage lives in a new `swink-agent-artifacts` crate rather than in the memory crate. Rationale: artifacts involve potentially large binary I/O with different performance characteristics than the JSONL-based session message store. Separation keeps the memory crate focused on conversation persistence and avoids coupling artifact storage decisions to message storage decisions.
- The `ArtifactStore` trait is defined in the core `swink-agent` crate (behind a feature gate) so that tool signatures can reference it. The `FileArtifactStore` implementation lives in the `swink-agent-artifacts` crate.
- In multi-agent scenarios (spec 009), child agents do not inherit the parent's artifact store. If a child agent needs artifact access, the consumer must explicitly configure it with an artifact store.
- There is no artifact size limit enforced by the framework. Consumers and store implementations are responsible for managing storage capacity.
- Delete is all-or-nothing per artifact name — there is no API to delete individual versions. This simplifies the API and avoids version-gap complexity. If selective version deletion is needed, it can be added in a future specification.
