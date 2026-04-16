# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.6] - 2026-04-16

### Fixed
- Add explicit `version` field to every internal workspace path dependency
  (e.g. `swink-agent = { path = "..", version = "0.7.6", ... }`). `cargo
  publish` requires a version on every dep so it can strip the path and
  resolve via crates.io. The v0.7.5 publish run shipped `swink-agent` and
  `swink-agent-macros` (no internal deps) but failed on
  `swink-agent-adapters` with "all dependencies must have a version
  requirement specified when publishing." The CI `cargo package --list`
  fallback used for downstream crates does not validate this; only real
  `cargo publish` does. v0.7.6 republishes everything; `swink-agent` and
  `swink-agent-macros` v0.7.5 remain on crates.io but have no dependents.

## [0.7.5] - 2026-04-16

### Fixed
- Replace invalid crates.io category slugs (`development-tools::proc-macros` →
  `development-tools::procedural-macro-helpers` in `swink-agent-macros`;
  `machine-learning` → `science` in `swink-agent-local-llm`). The v0.7.4
  publish run uploaded `swink-agent` to crates.io but failed at the macros step
  because crates.io validates categories server-side only during real upload —
  `cargo publish --dry-run` cannot detect this. v0.7.5 republishes everything
  with corrected metadata; `swink-agent` v0.7.4 remains on crates.io but has no
  dependents.

## [0.7.4] - 2026-04-16

### Changed
- Repo made public. Open-source readiness: MIT-only license, full Cargo.toml
  metadata across all crates, crates.io + docs.rs badges, CONTRIBUTING.md,
  SECURITY.md, THANKYOU.md, branch model (`main` + `integration`), PR gate,
  approve-contributor workflow, issue templates, and AGENTS.md for all crates.

## [0.7.3] - 2026-04-15

### Added
- `EditFileTool` — surgical find-and-replace file editing tool, re-exported from crate root.
- Mid-stream steering interrupt: queued messages now land at the turn boundary without aborting in-flight tool batches.

### Fixed
- Adapter pre-stream `Start`/`Error` event ordering (#571).
- Preserve Ollama NDJSON UTF-8 chunk boundaries (#570).
- Pre-dispatch stop result parity (#568).
- TUI streaming jitter and per-token redraw churn eliminated.

## [0.7.2] - 2026-04-10

### Fixed
- TUI approval mode: `Agent` is now the single source of truth (#567).
- Inline aborted tool turns instead of surfacing them as errors (#566).
- Isolate `adapters` no-default-features sentinel (#564).
- Include loop context in pause snapshot to prevent message loss (#563).
- Abort spawned tool handles on `ChannelClosed` (#562).

### Changed
- Examples migrated to [SuperSwinkAI/Swink-Agent-Examples](https://github.com/SuperSwinkAI/Swink-Agent-Examples).

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
- **Stabilize public API surface (#263).** 15 internal modules changed from `pub mod` to `pub(crate) mod`. All public items remain accessible via root re-exports (`use swink_agent::StreamFn`). Downstream consumers must update module-path imports.

### Added
- Feature-matrix smoke tests for all optional root features (#292).
- `pub const VERSION` re-exported from the lib root, sourced from `CARGO_PKG_VERSION`.
- Release workflow triggered on `v*` tags: dry-run publish of all workspace crates, GitHub release with generated notes and `Cargo.lock` attached.
- Windows CI coverage for default builtin tools (#294).

### Fixed
- Remove duplicate `#![forbid(unsafe_code)]` attributes in policies and mcp crates (#262).
- Replace panicking unwraps in xtask report with proper error handling (#288).
- `SessionState::set` now returns `Result` instead of panicking (#291).
- Gate builtin-tools references behind feature flag in tests and examples (#261).

### Changed
- Centralize shared workspace dependencies: `regex`, `dirs`, `toml`, `bytes` (#264).
- License simplified to MIT-only.

## [0.6.x] - 2026-03-10 to 2026-04-05

Major additions: Gemma 4 local inference, `BlockAccumulator` for streaming event assembly, `schemars`-based proc-macro engine, multi-agent patterns and artifact service, MCP integration, plugin system, policy slots, credential management, TUI session management, and web browse plugin. 42 specs implemented across the 0.6 lifecycle.

[Unreleased]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.6...HEAD
[0.7.6]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.2...v0.7.0
[0.6.x]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.5.0...v0.6.2
