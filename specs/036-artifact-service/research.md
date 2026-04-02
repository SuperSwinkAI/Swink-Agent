# Research: Artifact Service

**Feature**: 036-artifact-service | **Date**: 2026-04-02

## 1. Filesystem Layout for Versioned Artifacts

**Decision**: Version-numbered files (`v1.bin`, `v2.bin`, ...) with a single `meta.json` sidecar per artifact directory.

**Rationale**: Simple, human-inspectable layout. Each version is a standalone file — easy to debug, backup, or migrate. The sidecar JSON consolidates all version metadata in one read, avoiding per-version metadata files. Atomic version numbering is handled by reading `meta.json` under a lock, incrementing, and writing back.

**Alternatives considered**:
- Content-addressed storage (SHA-based filenames): Adds deduplication but complicates version ordering and requires hash computation on every save. Overkill for per-session artifacts.
- SQLite per session: Provides strong concurrency guarantees but adds a heavy dependency (`rusqlite`) for a simple versioned blob store. Violates constitution principle IV (prefer existing workspace deps).
- Append-only log file: Would require custom parsing and makes random-access to specific versions expensive.

## 2. Concurrency Strategy for FileArtifactStore

**Decision**: In-process `tokio::sync::Mutex` keyed by `(session_id, artifact_name)` for version numbering. File writes use temp-file + atomic rename.

**Rationale**: The artifact store is used within a single Tokio runtime (agent process). Cross-process concurrency is not a requirement (spec assumption: per-session, single-agent access). `tokio::sync::Mutex` avoids blocking the runtime. Temp-file + rename ensures no partial writes are visible.

**Alternatives considered**:
- File locking (`flock`/`lockfile`): Adds cross-process safety but is platform-dependent and unnecessary given single-process assumption.
- Lock-free CAS on version counter file: Complex, error-prone, and gains nothing when we already hold a Tokio mutex.
- `std::sync::Mutex`: Would block the async runtime during I/O operations.

## 3. Metadata Sidecar Format

**Decision**: JSON (`meta.json`) using `serde_json`.

**Rationale**: `serde_json` is already a workspace dependency. JSON is human-readable and easily inspectable. The metadata is small (version records with name, number, timestamp, size, content type, custom metadata map) — no performance concern from JSON parsing.

**Alternatives considered**:
- TOML: Less natural for arrays of version records. Would add complexity without benefit.
- Bincode/MessagePack: Faster but not human-readable. Debugging artifact stores becomes harder. Not justified for small metadata files.

## 4. Streaming Trait Design

**Decision**: Separate `StreamingArtifactStore` trait with `save_stream` and `load_stream` methods using `futures::Stream<Item = Result<bytes::Bytes, ArtifactError>>`.

**Rationale**: Keeps the base `ArtifactStore` trait simple (just `Vec<u8>`). Implementations opt into streaming by additionally implementing the extension trait. Uses `bytes::Bytes` (already in the dependency tree via `reqwest`) for zero-copy chunk passing. `Result` in the stream item allows implementations to surface I/O errors during streaming.

**Alternatives considered**:
- `AsyncRead`/`AsyncWrite` from `tokio::io`: Lower-level, requires callers to manage buffering. Less ergonomic for consumers who just want chunks.
- Default trait methods on `ArtifactStore`: Would force all implementations to deal with stream types even if they only support `Vec<u8>`.

## 5. Artifact Name Validation

**Decision**: Regex-free validation function checking each character against the allowed set: `[a-zA-Z0-9\-_./]`. Names must not be empty, must not start or end with `/`, and must not contain `//` (consecutive slashes).

**Rationale**: Simple character-by-character check is faster than regex compilation for a validation that runs on every save. Additional path-safety rules (no leading/trailing slash, no consecutive slashes) prevent filesystem edge cases.

**Alternatives considered**:
- Regex: Overhead of compilation for a simple character set check. Would need `regex` crate (not currently a core dependency).
- No validation (consumer responsibility): Risks filesystem injection via `../` or OS-specific path issues. Validation at the framework level is safer.

## 6. Built-in Tool Content Handling

**Decision**: `SaveArtifactTool` accepts string content and encodes to UTF-8 bytes. `LoadArtifactTool` returns text content as-is for text MIME types, and a `"[binary: {size} bytes, type: {content_type}]"` summary for non-text types.

**Rationale**: LLMs work with text. Providing binary content directly in tool results would produce garbled output. The size/type summary gives the LLM enough information to reference the artifact without attempting to render binary data. Consumers who need binary artifact access use the programmatic API, not the LLM tools.

**Alternatives considered**:
- Base64-encoding binary content: Would bloat tool results (33% overhead) and most LLMs can't meaningfully process base64 binary data.
- Refusing to load binary artifacts: Too restrictive — the LLM should at least know the artifact exists and its type.

## 7. Feature Gate Structure

**Decision**: Two independent features on the core crate: `artifact-store` (trait + types + event variant) and `artifact-tools` (built-in tools, depends on `artifact-store`). The `artifacts` crate has no feature gates (both `FileArtifactStore` and `InMemoryArtifactStore` always available).

**Rationale**: Follows the established pattern from spec 033 (workspace feature gates). Consumers who want custom artifact tools but not the built-in ones can enable only `artifact-store`. The artifacts crate is opt-in at the dependency level — no need for internal feature gates.

**Alternatives considered**:
- Single `artifacts` feature: Would force built-in tools on consumers who only want the storage trait.
- Feature gates on the artifacts crate: Unnecessary complexity — if you depend on the crate, you want both implementations.
