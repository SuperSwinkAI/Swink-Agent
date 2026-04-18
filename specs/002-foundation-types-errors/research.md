# Research: Foundation Types & Errors

**Feature**: 002-foundation-types-errors
**Date**: 2026-03-20

## Technical Context Resolution

No NEEDS CLARIFICATION items. All technical decisions are determined by
the project conventions (AGENTS.md), the PRD data model (§3, §10.3), and
the clarification session.

## Decisions

### Error Derive Strategy

- **Decision**: Use `thiserror` for all error type derivation.
- **Rationale**: `thiserror` is the standard Rust crate for deriving
  `std::error::Error` with `#[error(...)]` display formatting and
  `#[from]` source chaining. It produces zero-overhead code and is
  already a workspace dependency.
- **Alternatives considered**: Manual `impl Error` — rejected per
  constitution principle IV (leverage the ecosystem). `anyhow` — rejected
  because it erases error types; this feature needs matchable variants.

### Serialization Format

- **Decision**: Use `serde` with `serde_json` for JSON serialization.
  All types derive `Serialize` and `Deserialize`.
- **Rationale**: JSON is the interchange format for LLM provider APIs.
  Serde is the de facto Rust serialization framework and is already a
  workspace dependency.
- **Alternatives considered**: Manual serialization — rejected per
  constitution principle IV. Bincode/MessagePack — rejected because
  JSON is the provider interchange format.

### Custom Message Extension Mechanism

- **Decision**: Trait object with `Any` for runtime downcasting, returning
  `Result<&T, DowncastError>` on failure.
- **Rationale**: Trait objects keep the API non-generic (no type parameter
  pollution through Agent, AgentContext, etc.). The `DowncastError` return
  type was selected in the clarification session for better debugging.
  The trait requires `Send + Sync + Any` bounds.
- **Alternatives considered**: Generic `AgentMessage<M>` — rejected
  because it would propagate the type parameter through the entire API.
  Enum with `serde_json::Value` — rejected because it loses type safety.

### Timestamp Representation

- **Decision**: Use `std::time::SystemTime` for message timestamps.
  Serde serialization via a custom module that converts to/from
  Unix epoch milliseconds.
- **Rationale**: `SystemTime` is in the standard library (no extra
  dependency). Unix epoch millis is a widely supported interchange
  format. Avoids adding `chrono` as a dependency for a single use case.
- **Alternatives considered**: `chrono::DateTime<Utc>` — rejected per
  constitution principle IV (avoid adding a dependency when std suffices).
  `u64` epoch millis directly — rejected because `SystemTime` provides
  better type safety and arithmetic.

### Image Source Types

- **Decision**: Three variants: `Base64 { data: String, media_type: String }`,
  `Url { url: String, media_type: String }`,
  `File { path: PathBuf, media_type: String }`.
- **Rationale**: Covers the three patterns used by LLM providers (inline
  base64, URL reference) plus local file paths for developer convenience.
  Selected in clarification session (option C).
- **Alternatives considered**: Single opaque bytes buffer — rejected
  because it loses source type information needed by adapters.

### Usage Counter Types

- **Decision**: `u64` for all token counters. `f64` for cost values.
  Zero counters are valid (no "empty" concept).
- **Rationale**: Token counts are never negative and can be large.
  Costs require decimal precision. Zero validity confirmed in
  clarification session.
- **Alternatives considered**: `u32` — rejected because large context
  windows can exceed 32-bit token counts in aggregated usage.
