# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.1] - 2026-04-15

### Fixed
- Enforce proxy terminal event before `[DONE]` to prevent stray trailing tokens (#552).
- Web plugin rate-limiter cutoff underflow when body is shorter than the byte window (#551).
- Preserve custom message envelopes during JSONL entry saves (#550).
- `atomic_fs` replace semantics on Windows — use `MOVEFILE_REPLACE_EXISTING` flag (#549).
- Guard checkpoint restore against concurrent agent runs to prevent state corruption (#548).
- Thread raw SSE payload callbacks through all runtime adapters (#547).
- SSE parser now handles field lines without a trailing space after the colon (#546).
- Custom tool execution partition validation to reject mismatched call/result pairings (#545).
- Abort in-flight tool batches when parent `CancellationToken` fires (#544).
- Delay OpenAI tool-call `Start` event until the tool name is fully known (#532).
- Validate eval store filesystem IDs to reject path-traversal inputs (#531).
- Make Gemini final tool-call deltas deterministic (#530).
- Prevent steering message drop in concurrent tool-dispatch workers (#529).
- Emit terminal error on local-LLM EOF without a `Response::Done` frame (#528).
- Apply session migrators in `JsonlSessionStore::load` (#527).
- Preserve steering interrupt messages across checkpoint cycles (#526).
- Make artifact streaming saves incremental rather than full-file rewrites (#515).
- Centralize local LLM preset defaults to avoid divergence across callers (#514).
- Reject duplicate orchestrator registrations (#513).
- Emit pipeline failure events on execution errors (#512).

## [0.7.0] - 2026-04-09

### Breaking
- **Stabilize public API surface (#263).** 15 internal modules (`stream`, `types`, `tool`, `policy`, `plugin`, `convert`, etc.) changed from `pub mod` to `pub(crate) mod`. All public items remain accessible via root re-exports (`use swink_agent::StreamFn` instead of `use swink_agent::stream::StreamFn`). Downstream consumers must update module-path imports.

### Added
- Feature-matrix smoke tests for all optional root features: `artifact-store`, `artifact-tools`, `hot-reload`, `tiktoken`, `plugins`, `otel` (#292).
- Expanded `tests/public_api.rs` with 9 new type-existence assertions covering stream, policy, tool, convert, model, display, metrics, and plugin re-exports.
- `pub const VERSION` re-exported from `swink-agent` lib root, sourced from `CARGO_PKG_VERSION`.
- Release engineering: `.github/workflows/release.yml` triggered on `v*` tags, dry-run publishing every workspace crate in topological order and producing a GitHub release with generated notes and `Cargo.lock` attached.
- Workspace-wide publish metadata via `[workspace.package]` inheritance: dual-licensing (`MIT OR Apache-2.0`), keywords, categories, exclude patterns, and per-crate READMEs pointing at the workspace README.
- Top-level `LICENSE-MIT` and `LICENSE-APACHE` files.
- Windows CI coverage for default builtin tools (#294).

### Fixed
- Remove duplicate `#![forbid(unsafe_code)]` attributes in policies and mcp crates (#262).
- Replace panicking unwraps in xtask report with proper error handling (#288).
- `SessionState::set` now returns `Result` instead of panicking on serialization failure (#291).
- Gate builtin-tools references behind feature flag in tests and examples (#261).
- Make `InputEditor` fields private and remove unwrap panics (#300).

### Changed
- Centralize shared workspace dependencies: `regex`, `dirs`, `toml`, `bytes` (#264).
- License bumped from `MIT` to `MIT OR Apache-2.0` across all publishable workspace crates.

## [0.6.2] - 2026-04-05

### Fixed
- Preserve repeated same-name Ollama tool calls (#213).
- Preserve TUI session metadata across saves and surface failures (#200).
- Make Gemma 4 delimiter parsing UTF-8 safe (#182).
- Expose session store injection and public resume API in `swink-agent-tui` (#191).
- Remove hand-coded remote preset keys; the catalog is now authoritative for preset identity (#198).

### Changed
- Migrate `GeminiStreamState` to the shared `BlockAccumulator` (#199).
- Extract `BlockAccumulator` to consolidate streaming event assembly (#194).
- Replace the bespoke tool-schema proc-macro engine with `schemars` (#192).
- Remove the unused SQLite session store backend (#190).
- Collapse memory persistence codecs (#181).

## [0.6.1] - 2026-03-20

### Fixed
- Satisfy workspace clippy across all crates (#184).

### Changed
- Clear workspace test warnings (#183).

## [0.6.0] - 2026-03-10

### Added
- Gemma 4 E2B direct local inference (GPU-validated) (#163).
- `Gemma4_31B` preset for `google/gemma-4-31B-it`.

[Unreleased]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.2...HEAD
[0.6.2]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/SuperSwinkAI/Swink-Agent/releases/tag/v0.6.0
