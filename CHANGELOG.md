# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - TBD

### Added — spec 043 (evals: advanced features)

- **`swink-agent-eval-judges` crate** — nine per-provider `JudgeClient` implementations (Anthropic, OpenAI, Bedrock, Gemini, Mistral, Azure, xAI, Ollama, Proxy) plus `Blocking<Provider>JudgeClient` sync wrappers, behind the `all-judges` umbrella feature.
- **Prompt-template registry** (`judge-core` feature) — `JudgePromptTemplate`, `MinijinjaTemplate`, `PromptTemplateRegistry`, built-in `*_v0` templates; duplicate-version registration rejected.
- **24 evaluators across seven families** — Quality (10), Safety (7), RAG (3 + `Embedder`), Agent (9), Simple (2), Structured (2), Code (4) plus Multimodal (`ImageSafetyEvaluator`). Shared `JudgeEvaluatorConfig` + `JudgeEvaluatorBuilder` trait expose `.with_prompt()`, `.with_few_shot()`, `.with_system_prompt()`, `.with_output_schema()`, `.with_use_reasoning()`, `.with_feedback_key()`, `.with_aggregator()` on every judge-backed evaluator.
- **`EvalRunner` upgrades** — parallelism, `num_runs`, disk-backed judge cache, cancellation, initial-session hydration.
- **Multi-turn simulation + experiment generation** — `ActorSimulator`, `ToolSimulator`, `ExperimentGenerator`, `TopicPlanner` behind `simulation` / `generation` features.
- **Trace ingestion** (`trace-ingest`) — `OtelInMemoryTraceProvider`, `OpenInferenceSessionMapper`, `LangChainSessionMapper`, `OtelGenAiSessionMapper`, `SwarmExtractor`, `GraphExtractor`, `ToolLevelExtractor`. Optional backends: `OtlpHttpTraceProvider` (`trace-otlp`), `LangfuseTraceProvider` (`trace-langfuse`), `OpenSearchTraceProvider` (`trace-opensearch`), `CloudWatchTraceProvider` (`trace-cloudwatch`, takes a caller-supplied `CloudWatchLogsFetcher`).
- **`EvalsTelemetry`** — OTel span emission inside the runner (`telemetry` feature).
- **Reporters** — `ConsoleReporter`, `JsonReporter` (schema-stable via `SCHEMA_VERSION`), `MarkdownReporter`, `HtmlReporter` (`html-report` feature, self-contained artifact), `LangSmithExporter` (`langsmith` feature, pushes runs + feedback with partial-failure reporting).
- **`swink-eval` CLI binary** (`cli` feature) — `run`/`report`/`gate` subcommands with stable exit codes (0/1/2/3). Bundled GitHub Actions templates (`pr-eval.yml`, `nightly-eval.yml`, `release-eval.yml`, `pre-commit-hook.yml`) surfaced as `include_str!` constants via the new `ci` module.
- **SC-008 deterministic replay** — `OpenInferenceSessionMapper` round-trips scores bit-identically between in-process and reloaded OTel sessions.

### Changed — spec 043

- **`EvalCase`** extended with `expected_assertion`, `expected_interactions`, `few_shot_examples`, `attachments`, `session_id`, `metadata` (serde backwards-compatible — new fields default on deserialize).
- **`EvaluatorRegistry::add`** now rejects duplicate evaluator names with `EvalError::DuplicateEvaluator`; `register` panics on collision for ergonomic setup.
- **Judge scores** are clamped to `[0.0, 1.0]` with a structured `Detail::ScoreClamped { original, clamped }` recorded in `EvalMetricResult::details` when the raw verdict is out of range (FR-021).
- **`GateConfig`** now derives `Serialize`/`Deserialize` so the `swink-eval gate` subcommand can load thresholds from JSON.

### Breaking changes — spec 043

- `EvalCase` no longer has a default judge model id — `JudgeRegistry::builder(client, model_id)` now requires `model_id` as the second positional arg (FR-007 clarification Q9).
- FR-044 legacy-result converter was deliberately **not** shipped. The converter was a no-op shim for a shape that never reached a public release; downstream users already consume `EvalCaseResult` / `EvalSetResult` directly.

## [0.8.1] - 2026-04-22

### Added
- `swink-agent-adapters::build_remote_connection_with_credential` and public `build_connection_from_preset` — explicit-credential remote-connection builders for embedders that manage secrets in keychains/Vault and cannot mutate `std::env` (#791, #792).
- `swink-agent-eval` semantic trajectory matching (spec 023): `SemanticToolSelectionEvaluator`, `SemanticToolParameterEvaluator`, `EnvironmentStateEvaluator`, and a `JudgeClient` trait with pluggable providers. Each evaluator wraps judge calls in a configurable `tokio::time::timeout` (5 min default, `with_timeout` override) so evals own their own non-hang guarantee. Includes `MockJudge` in `swink-agent-eval::testing`.
- `swink-agent-eval` foundational score aggregators (#747) and deterministic case-session IDs — enables downstream experiment tooling.
- `swink-agent-eval-judges` crate scaffold (spec 043 Phase 1) — advanced evals framework foundation.
- `swink-agent-eval` default URL filter — a built-in `url_filter` module for trajectory/content scoring.
- Panic isolation across eval scorers (#731, #767) — a panicking scorer no longer tears down the evaluator run.
- `FnTool::with_execute_async` alias for untyped async builder discoverability (#663).
- Built-in `TiktokenCounter` for token counting without external dependencies (#662).
- TUI click-drag text selection and copy in chat view (#605, #606).
- Resolver-backed SSE MCP auth bootstrap (#679).
- `ApprovalMode` and `ToolMiddleware` exports from the prelude (#659, #660).

### Changed
- `BudgetGuard` ported to the `BudgetPolicy` loop-policy interface (spec 023 Phase 13) — budget constraints now compose through the same slot vectors as other policies.
- MCP tool registration names are now sanitized for provider compatibility (#702).
- Composed plugin tool names use hash-tail truncation to prevent long-name collisions; `Agent::new()` and `Agent::set_tools()` fail fast on duplicate final names (#674).
- Agent loop `PreTurn` now exposes the initial prompt batch as the first-turn `new_messages` slice; post-turn policy-injected messages processed before follow-up polling or `AgentEnd` (#676).

### Fixed
- **Streams and cancellation**: honor cooperative cancellation in web tools (#734); short-circuit pre-cancelled local-LLM streams; emit aborted stop reason on local-LLM cancel; honor pre-send stream cancellation in adapters; preserve single `MessageStart` across overflow recovery (#721); bound per-tool update channel (#770, #777); bound tool-update buffering.
- **MCP**: clear stdio child environments; fix reconnect and shutdown lifecycle (#701); emit connect/discovery/call lifecycle events (#625); roll back MCP collisions (#723); refresh SSE resolver auth on recovery (#680).
- **Adapters**: reject nameless terminal tool calls; hard-fail malformed Anthropic SSE events (#720); stop retrying parse and protocol faults (#629); gate Azure auth dependency (#631); sanitize incomplete `tool_use` arguments before dispatch (#621); normalize parse error classification for OAI/Gemini (#703).
- **Auth and secrets**: sanitize OAuth2 refresh diagnostics; include sanitized endpoint in OAuth2 refresh-failure debug log; redact OAuth2 refresh error bodies (#626); sanitize credential store tool errors (#706); redact `#key` secrets from TUI input history (#628).
- **Artifacts**: enforce streaming metadata integrity; treat missing content as corruption; serialize delete mutations (#682); make delete exact-name-safe for nested IDs (#705); validate `session_id` and enforce canonical artifact root (#622).
- **Memory**: require explicit atomic `save_full` (#683); serialize JsonlSessionStore delete locking (#724); take `Checkpoint` by value in `save_checkpoint` (#661).
- **Loop**: enforce two-pass `PreDispatch` before approval (#627); stop loop on pre-dispatch Stop (#699); emit single terminal event on overflow failure (#644); drain steering after text-only turns; block post-turn tool-call injection; preserve one retry message lifecycle (#677); preserve dynamic prompt during overflow retry (#700).
- **Patterns**: isolate parallel branch failures.
- **Eval**: reject duplicate evaluator registrations; restore cache-prefix tracking.
- **TUI**: harden setup wizard and editor temp files; fail closed on approval channel errors.
- **CI**: unbreak integration clippy + deny on rust 1.95; repair malformed YAML in bench/approve-contributor workflows; replace unsupported expression functions in approve-contributor.
- **Telemetry**: redact custom message warning logs.

### Internal
- Centralized workspace clippy config (`[workspace.lints]`).
- Pinned toolchain to Rust 1.95 stable (#737).
- Dependabot cadence changed from weekly to daily for cargo updates.
- Dependency bumps: `rmcp` 1.3.0 → 1.5.0 (#787), `scraper` 0.25.0 → 0.26.0 (#786), `notify` 7.0.0 → 8.2.0 (#790).

## [0.8.0] - 2026-04-19

### Added
- `FileCheckpointStore` in `swink-agent-memory` — durable file-backed checkpoint persistence across process restarts (#666).
- `FnTool::with_execute_async` alias — explicit untyped async builder for discoverability (#663).
- Custom SSE MCP headers (`McpTransport::Sse { headers }`) — supports `x-api-key` and other non-standard MCP server auth; also fixes bearer-token prefix duplication (#665).
- Built-in `TiktokenCounter` for token counting without external dependencies (#662).
- TUI click-drag text selection and copy in chat view (#605).

### Changed
- Composed plugin tool names now use hash-tail truncation to prevent long-name collisions; `Agent::new()` and `Agent::set_tools()` fail fast on duplicate final names (#674).
- Agent loop `PreTurn` now exposes the initial prompt batch as the first-turn `new_messages` slice; post-turn policy-injected messages are processed before follow-up polling or `AgentEnd` (#676).

### Fixed
- OAuth2 refresh failures no longer leak `error_description` or raw token-endpoint bodies into tool-facing errors or debug logs (#675).
- Fixed pre-dispatch state snapshot reuse, two-pass `PreDispatch` enforcement before approval, and cache-miss retry strategy (#627, #639, #643).
- Fixed overflow terminal event emission, stream/import cycle, and sync runtime init errors (#642, #644, #649).
- Fixed TUI corrupted session state load, Bedrock terminal frame requirement, and approval debug context redaction (#646, #647, #650).
- Fixed Azure auth dependency gate, adapter retry on parse/protocol faults, plugin tool name sanitization for provider compatibility, and incomplete `tool_use` argument sanitization (#620, #621, #629, #631).
- Fixed MCP lifecycle event emission and artifact session/root validation (#622, #625).
- `ApprovalMode` and `ToolMiddleware` exported from prelude (#659, #660).
- `Checkpoint` now taken by value in `save_checkpoint` (#661).

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
