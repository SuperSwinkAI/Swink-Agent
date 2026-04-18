# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.9] - 2026-04-16

### Changed
- **Breaking**: `swink-agent-local-llm` backend replaced from `mistralrs 0.8`
  to `llama-cpp-2` (Rust bindings for llama.cpp). All models now use GGUF
  format uniformly. Feature flags changed: removed `cudnn`, `flash-attn`,
  `mkl`, `accelerate`; added `vulkan`.
- Gemma 4 presets updated to GGUF repos (`bartowski/`). EmbeddingGemma
  updated to GGUF (`unsloth/embeddinggemma-300m-GGUF`).
- TUI no longer auto-wires the local model as a default/fallback. The
  `local` feature is kept for explicit opt-in.
- Spec 041 (Gemma 4 local adapter) folded into spec 022 (local-llm crate).

### Fixed
- SmolLM3 GGUF models now produce text output instead of empty responses
  (#594, #586). The `mistralrs` backend rejected the SmolLM3 architecture;
  `llama-cpp-2` supports it natively.
- Gemma 4 E2B/E4B models now produce text output. The GGUF-embedded Jinja
  template was too complex for llama.cpp's template engine; prompt is now
  formatted manually for Gemma 4 models.
- Tool pre-dispatch is now cancellation-aware (#592).
- Turn index increments correctly after no-tool turns (#595).

## [0.7.8] - 2026-04-16

### Changed
- Model catalog: add GPT-5 series (`gpt-5`, `gpt-5-mini`, `gpt-5-nano` +
  dated variants) and GPT-5.4 series (`gpt-5.4`, `gpt-5.4-mini`,
  `gpt-5.4-nano`). Remove deprecated OpenAI models below version 5
  (`gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `o3-mini`, `o1`).
- CI consolidated from 9 jobs to 3 on PRs (~5 min instead of ~20 min).
  Full platform matrix, semver, and MSRV checks now run only on main
  pushes or weekly schedule. `integration` branch removed from push trigger
  to avoid redundant double-runs.

### Fixed
- Fail fast with a clear error when the unsupported SmolLM3 local preset
  is selected instead of silently producing garbage output (#587).
- Force-reinstall `cargo-binstall` tools (`cargo-hack`, `cargo-nextest`,
  `cargo-semver-checks`) on every CI run to prevent stale binary cache
  failures (#589).

## [0.7.7] - 2026-04-16

### Fixed
- Remove `version` field from internal workspace path **dev-dependencies**
  (8 entries across the workspace). Dev-deps are stripped on publish, so
  adding `version` does nothing useful, and worse — cargo tries to resolve
  them via the registry during packaging, failing for crates that aren't on
  crates.io yet. The v0.7.6 publish job failed at dry-run on swink-agent
  itself with "no matching package named `swink-agent-adapters` found"
  because the root crate's dev-dep on adapters was given a version field.
- Fix topological publish order in `release.yml`: `swink-agent-adapters`
  was published before `swink-agent-auth`, but adapters has a regular dep
  on auth. Reordered to: tier 1 (auth, memory, policies, artifacts, eval,
  local-llm, mcp, patterns, plugin-web) → tier 2 (adapters) → tier 3 (tui).
  This bug would have surfaced if v0.7.6 had reached the publish step.

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

[Unreleased]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.8...HEAD
[0.7.8]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.6.2...v0.7.0
[0.6.x]: https://github.com/SuperSwinkAI/Swink-Agent/compare/v0.5.0...v0.6.2
