# AGENTS.md — Swink Agent

## Project

Pure-Rust library for LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Workspace: core (`swink-agent`), adapters (`swink-agent-adapters`), memory (`swink-agent-memory`), local-llm (`swink-agent-local-llm`), eval (`swink-agent-eval`), TUI (`swink-agent-tui`).

## Development Principles

- **Test-driven.** Run `cargo test --workspace` before every commit. Bug found → regression test first, then fix.
- **Speed.** Minimize allocations on hot paths. `tokio::spawn` for concurrent tool execution, not sequential awaits.
- **No unsafe.** `#[forbid(unsafe_code)]` at every crate root.
- **Lessons learned go in nested AGENTS.md files.** Update when you discover something non-obvious.
- **Context7 first.** When researching any crate API, dependency docs, or library usage, always query the context7 MCP server before falling back to web search or training data. Training data may be stale; context7 pulls current docs.
- **No parallel builds in agents.** Never have multiple subagents run `cargo build`/`test`/`clippy` concurrently — Cargo's global lock serializes them anyway, causing extended build times. Run all compilation in the main conversation first; subagents should only read and analyze code.
- **Check specs and docs first.** Before making large changes, read the relevant spec files in `specs/NNN-*/` (spec.md, plan.md, tasks.md) and architecture docs in `docs/` (HLD, subsystem READMEs, planning docs). The project uses spec-driven development — changes should align with the agreed design.

## Style (project-specific conventions)

- Follows standard Rust conventions (RFC 430, API Guidelines, clippy defaults).
- Trailing `_` for reserved-word modules: `loop_.rs`.
- Closure type aliases suffixed with `Fn`: `ConvertToLlmFn`, `GetApiKeyFn`.
- `new()` primary constructor; `with_*()` builder chain. Named constructors on error types: `AgentError::network(err)`.
- No `get_` prefix on getters. `is_*`/`has_*` for predicates.
- `lib.rs` re-exports the public API — consumers never reach into submodules.
- One concern per file. Split at ~1500 lines.
- Import order: `std` → external (alphabetical) → `crate::`/`super::`.
- Test names: descriptive `snake_case` without `test_` prefix. Mocks prefixed `Mock`.
- Shared test helpers in `src/testing.rs` (gated behind `testkit` feature), re-exported via `tests/common/mod.rs`.

## Build & Test

```bash
cargo build --workspace
cargo test --workspace --features testkit         # testkit enables test helpers
cargo test -p swink-agent --no-default-features   # verify builtin-tools disabled
cargo clippy --workspace -- -D warnings           # zero warnings policy
cargo run -p swink-agent-tui                      # launch TUI (.env auto-loaded)
```

MSRV **1.88** (edition 2024). Workspace deps centralized in root `Cargo.toml`.

## Lessons Learned

### Agent (`src/agent.rs`)

- `dispatch_event` catches panics via `catch_unwind` and **auto-removes** panicking subscribers (QA-discovered: originally panics were caught but subscribers were not removed).
- `in_flight_llm_messages` filters out `CustomMessage` — they survive compaction but never reach the provider.
- Queues use `Arc<Mutex<>>` with `PoisonError::into_inner()` — never panics on poisoned locks.
- `dispatch_event` now wraps event forwarders in `catch_unwind` — matching the existing listener panic safety. Panicking forwarders are logged and skipped, not auto-removed (unlike listeners).
- `AgentId` lives in its own module `src/agent_id.rs` (extracted from `registry.rs` to break the `agent.rs` ↔ `registry.rs` circular import).

### Agent Loop (`src/loop_.rs`)

- Nested outer/inner loop: outer = multi-turn follow-up, inner = single turn.
- `overflow_signal` lives on `LoopState`, **not** `AgentContext`. Resets after `transform_context`.
- `transform_context` is **synchronous** (not async).
- `CONTEXT_OVERFLOW_SENTINEL` triggers overflow retry — loop control signal, not an error.
- Tool dispatch order: PreDispatch policies → Approval → Schema validation → `execute()`. (Old order was Approval → Transformer → Validator → Schema → Execute, superseded by 031-policy-slots.)

### Policy Slots (`src/policy.rs`)

- Four configurable policy slots: PreTurn (before LLM call), PreDispatch (per tool call), PostTurn (after turn), PostLoop (after inner loop).
- Two verdict enums: `PolicyVerdict` (Continue/Stop/Inject) and `PreDispatchVerdict` (+ Skip). Compile-time enforcement that Skip is only valid in PreDispatch.
- Slot runner uses `AssertUnwindSafe` + `catch_unwind` — traits only need `Send + Sync`, not `UnwindSafe`.
- Empty policy vecs = zero overhead, no allocation. Default is anything-goes.
- **All policy implementations live in `swink-agent-policies` crate** — core only defines traits and runners.
- Old fields removed from `AgentLoopConfig`: `budget_guard`, `loop_policy`, `post_turn_hook`, `tool_validator`, `tool_call_transformer`. Replaced by 4 policy vecs.
- `PolicyContext.new_messages` contains only messages added since the last evaluation for that slot. PreTurn: pending batch (tracked via `new_messages_start` index before append). PostTurn/PostLoop/PreDispatch: `&[]` (current-turn data is in `TurnPolicyContext`/`ToolPolicyContext`). This is a slice borrow (zero-copy), not a clone.
- `RetryStrategy::should_retry()` is the **sole** retryability decision point — `is_retryable()` pre-check was removed.

### Policies Crate (`policies/`)

- Separate workspace crate `swink-agent-policies` — depends only on `swink-agent` public API, no internal imports.
- **All policy implementations live here** (10 total, each feature-gated independently):
  - Core: `BudgetPolicy`, `MaxTurnsPolicy`, `ToolDenyListPolicy`, `SandboxPolicy`, `LoopDetectionPolicy`, `CheckpointPolicy`.
  - Application: `PromptInjectionGuard`, `PiiRedactor`, `ContentFilter`, `AuditLogger`.
- Feature gates: `budget`, `max-turns`, `deny-list`, `sandbox`, `loop-detection`, `checkpoint`, `prompt-guard`, `pii`, `content-filter`, `audit`, `all` (default).
- Stateful policies (e.g., `LoopDetectionPolicy`) use interior mutability (`Mutex`) — trait takes `&self`.
- `CheckpointPolicy` bridges sync/async via `tokio::spawn` fire-and-forget. Captures `Handle::current()` at construction.
- `SandboxPolicy` checks configured field names (default: `["path", "file_path", "file"]`) — Skip with error, no silent rewriting.
- `PromptInjectionGuard` implements both `PreTurnPolicy` and `PostTurnPolicy` — single struct, dual trait.
- `PiiRedactor` Inject verdict constructs `AgentMessage::Llm(LlmMessage::Assistant(...))` preserving original metadata.
- `ContentFilter` converts keywords to regex at construction time (with `` for whole-word, `(?i)` for case-insensitive).
- `AuditSink` trait is sync (`fn write(&self, record: &AuditRecord)`) — defined in this crate, not in core.
- All regex patterns compiled once at construction, `evaluate()` only runs matches.

### Plugin System (`src/plugin.rs`)

- `Plugin` trait requires only `name()` — all contribution methods (`pre_turn_policies`, `post_turn_policies`, `tools`, `on_event`, etc.) default to no-op/empty.
- `PluginRegistry` deduplicates by name on `register()` — the new plugin **replaces** the old one (with a `tracing::warn`), not an error.
- `list()` returns plugins sorted by priority descending; insertion order preserved for ties (stable sort via `sort_by_key` with `std::cmp::Reverse`).
- `NamespacedTool` prefixes as `"{plugin_name}.{tool_name}"` — prevents tool name collisions across plugins. `metadata()` also sets `namespace` on the inner `ToolMetadata`.
- Contributions merged in `Agent::new()`: plugin policies **prepended** (priority-sorted), direct policies appended; plugin tools appended after direct tools.
- `on_init(&self, &Agent)` dispatched in priority order, wrapped in `catch_unwind` — panicking `on_init` is logged and skipped, construction continues (plugin's other contributions remain active).
- Entire module behind `#[cfg(feature = "plugins")]` — opt-in, not default-enabled, zero cost when disabled.

### Streaming (`src/stream.rs`)

- `accumulate_message` enforces strict ordering: one Start, indexed content blocks, one terminal (Done/Error).
- `partial_json` consumed on `ToolCallEnd` — parsed once. Empty string → `{}`, not null.
- `AssistantMessageEvent::error()` is the canonical error constructor — adapters must use it.

### Local LLM Streaming (`local-llm/src/stream.rs`)

- Gemma 4 delimiter scanners must only slice `&str` at UTF-8 character boundaries. For partial `<|channel>thought\n` and `<tool_call|>` matches, use the shared UTF-8-safe suffix helper instead of raw byte-offset suffix slicing.

### Context (`src/context.rs`)

- Sliding window: anchor (first N) + tail (recent), middle removed to fit budget.
- Tool-result pairs preserved together even if it exceeds budget. Correctness > token count.
- Token estimation: chars/4 heuristic. CustomMessage = 100 tokens flat.

### Error / Retry

- `is_retryable()` = true only for `ModelThrottled` and `NetworkError`. Custom `RetryStrategy` can override.
- No `MaxTokensReached` variant — handled via `CONTEXT_OVERFLOW_SENTINEL` + loop retry.
- Retry jitter: `[0.5, 1.5)` × capped delay. Default 3 max attempts.

### Tool System (`src/tool.rs`)

- `AgentToolResult.is_error` replaces old `text.starts_with("error")` heuristic. Use `error()` / `text()` constructors.
- `ToolCallTransformer` runs unconditionally (not gated by approval). Distinct from `ToolValidator` (rejects vs rewrites).

### Checkpoint / Persistence

- Checkpoint and SessionStore now support CustomMessage persistence via `custom_messages` field and `save_full`/`load_full`. Old checkpoints without `custom_messages` field deserialize fine (backward compat via `#[serde(default)]`).

### Feature Gates

**Root crate (`swink-agent`):**
- `builtin-tools` (default-enabled) — gates `BashTool`, `ReadFileTool`, `WriteFileTool`.
- `testkit` — gates the `testing` module (mock `StreamFn`/`AgentTool` implementations, event/message builders). Not default-enabled; consumers add `features = ["testkit"]` in dev-dependencies. Integration tests in `/tests/` are gated with `#![cfg(feature = "testkit")]`.
- `plugins` — gates `plugin` module (`Plugin` trait, `PluginRegistry`, `NamespacedTool`). Not default-enabled. `MockPlugin` in `testing.rs` also gated behind this feature.
- Root crate cannot re-export adapters/local-llm/TUI (cyclic dependency). Consumers depend on sub-crates directly.

**Adapters crate (`swink-agent-adapters`):**
- `default = ["all"]` — backward compatible, all 9 adapters enabled.
- Individual flags: `anthropic`, `openai`, `ollama`, `gemini`, `proxy`, `azure`, `bedrock`, `mistral`, `xai`.
- `gemini` feature gates the `google` module (file is `google.rs`, public type is `GeminiStreamFn`).
- `proxy` activates `eventsource-stream` dep. `bedrock` activates `sha2` dep. All others are marker flags.
- Shared infra (`base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`) compiles unconditionally.
- Pattern follows `swink-agent-policies`: paired `#[cfg(feature)]` on `mod` + `pub use`.

**Local-LLM crate (`swink-agent-local-llm`):**
- Backend flags: `metal`, `cuda`, `cudnn`, `flash-attn`, `mkl`, `accelerate` — each forwards to `mistralrs/<flag>`.
- No default backend. CPU-only inference when none enabled.

**Policies crate (`swink-agent-policies`):**
- `default = ["all"]`, 10 individual policy flags. Established pattern for feature gating.

## Active Technologies
- Rust 1.88 (edition 2024) + serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`) (001-workspace-scaffold)
- N/A (scaffold only) (001-workspace-scaffold)
- Rust 1.88 (edition 2024) + serde, serde_json, thiserror, uuid, schemars (all workspace deps) (002-foundation-types-errors)
- N/A (in-memory types only) (002-foundation-types-errors)
- Rust 1.88 (edition 2024) + okio (spawn, select!, CancellationToken), futures (Stream), serde_json (tool args) (004-agent-loop)
- N/A (stateless loop) (004-agent-loop)
- Rust 1.88 (edition 2024) + serde_json (tool args), jsonschema (validation), tokio (async), tokio-util (CancellationToken), futures (Stream), rand (jitter) (003-core-traits)
- N/A (traits and types only) (003-core-traits)
- Rust 1.88 (edition 2024) + okio, tokio-util (CancellationToken), futures (Stream), serde_json (Value), tracing (005-agent-struct)
- N/A (in-memory state; optional CheckpointStore trait for persistence) (005-agent-struct)
- Rust 1.88 (edition 2024) + okio (async runtime), serde_json (Value for tool arguments/extensions), tracing (006-context-management)
- N/A (in-memory `InMemoryVersionStore`; pluggable `ContextVersionStore` trait for persistence) (006-context-management)
- Rust 1.88, edition 2024 + `tokio`, `tokio-util` (CancellationToken), `serde_json`, `schemars`, `jsonschema`, `regex` (007-tool-system-extensions)
- Rust 1.88, edition 2024 + `serde` (deserialization), `toml` (catalog parsing), `tokio` (async runtime), `tokio-util` (CancellationToken), `futures` (stream primitives) (008-model-catalog-presets)
- Embedded TOML file (`src/model_catalog.toml`) compiled into the binary via `include_str!` (008-model-catalog-presets)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde_json`, `thiserror`, `tokio` (011-adapter-shared-infra)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing` (012-adapter-anthropic)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid` (013-adapter-openai)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid` (014-adapter-ollama)
- NDJSON streaming (not SSE); no authentication required; zero cost (local model) (014-adapter-ollama)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `serde`/`serde_json`, `eventsource-stream`, `tokio`, `tokio-util` (020-adapter-proxy)
- Rust 1.88 (edition 2024) + `ratatui` 0.30, `crossterm` 0.29 (event-stream), `syntect` 5 (syntax highlighting), `swink-agent` (core types) (026-tui-input-conversation)
- N/A (input history is per-session, in-memory) (026-tui-input-conversation)
- Rust 1.88 (edition 2024) + `ratatui` 0.30, `crossterm` 0.29 (event-stream), `arboard` (clipboard), `swink-agent` (core types), `swink-agent-memory` (`SessionStore`, `JsonlSessionStore`, `SessionMeta`) (028-tui-commands-editor-session)
- JSONL files via `swink-agent-memory` `JsonlSessionStore` (line 1 = `SessionMeta`, lines 2+ = `AgentMessage`) (028-tui-commands-editor-session)
- Rust 1.88, edition 2024 + `swink-agent` (core), `swink-agent-adapters` (proxy), `swink-agent-tui` (headless rendering), `tokio` (async runtime), `serde_json` (mock data), `futures` (stream combinators) (030-integration-tests)
- N/A — all state is in-memory mocks (030-integration-tests)
- Rust 1.88 (edition 2024) + okio (spawn, mpsc, oneshot, select!), tokio-util (CancellationToken), serde_json (Value), tracing (info, warn) (009-multi-agent-system)
- N/A (in-memory state only) (009-multi-agent-system)
- Rust 1.88 (edition 2024) + okio (async runtime), tokio-util (CancellationToken), futures (Stream, StreamExt), serde / serde_json (serialization), tracing (diagnostics) (010-loop-policies-observability)
- N/A (in-memory by default; `CheckpointStore` trait abstracts persistence) (010-loop-policies-observability)
- Rust 1.88 (edition 2024) + `serde`, `serde_json`, `tokio` (fs), `chrono` (timestamps), `tracing` (warning on corrupted lines) (021-memory-crate)
- Local filesystem via JSONL files (one file per session) (021-memory-crate)
- Rust 1.88 (edition 2024) + `mistralrs` (0.7, GGUF inference engine), `hf-hub` (HuggingFace model download with ETag/SHA verification), `tokio`, `tokio-stream`, `futures`, `serde`/`serde_json`, `thiserror`, `tracing`, `uuid` (022-local-llm-crate)
- Model weights cached in `~/.cache/huggingface/hub/` (managed by `hf-hub`) (022-local-llm-crate)
- Rust 1.88, edition 2024 + ratatui 0.30 (terminal UI framework), crossterm 0.29 (terminal control, event-stream feature), tokio (async runtime), toml 0.8 (config parsing), dirs 6 (platform-native config/data dirs), keyring 3 (OS keychain), thiserror (error types), tracing + tracing-subscriber + tracing-appender (file-based logging) (025-tui-scaffold-config)
- TOML config file at `dirs::config_dir()/swink-agent/tui.toml`; OS keychain for credentials (macOS Keychain, Windows Credential Manager, Linux secret-service) (025-tui-scaffold-config)
- Rust 1.88 (edition 2024) + `ratatui` 0.30, `crossterm` 0.29 (event-stream), `syntect` 5 (syntax highlighting for code blocks), `swink-agent` (core types — `Agent`, `ToolApproval`, `ToolApprovalRequest`, event system) (027-tui-tools-diffs-status)
- N/A (all state is in-memory per session) (027-tui-tools-diffs-status)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `ratatui` 0.30, `crossterm` 0.29, `tokio`, `tokio-util` (029-tui-plan-mode-approval)
- Rust 1.88 (edition 2024) + `swink-agent` (core types: `AgentEvent`, `ContentBlock`, `AssistantMessage`, `Cost`, `Usage`, `ModelSpec`, `StopReason`), `serde`/`serde_json` (serialization), `tokio`/`tokio-util` (async runtime, `CancellationToken`), `futures` (stream combinators), `regex` (response pattern matching), `sha2` (audit hashes), `thiserror` (error types), `tracing` (diagnostics), `uuid` (IDs) (023-eval-trajectory-matching)
- N/A (in-memory types; `FsEvalStore` for optional JSON persistence — covered by spec 024) (023-eval-trajectory-matching)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `serde`/`serde_json`, `tokio`/`tokio-util`, `futures`, `sha2`, `regex`, `thiserror`, `tracing`, `uuid`; optional `serde_yaml` via `yaml` feature gate (024-eval-runner-governance)
- Local filesystem via JSON files (`FsEvalStore`); optional YAML input via feature gate (024-eval-runner-governance)
- Rust 1.88 (edition 2024) + `tokio` (async runtime), `tokio-util` (CancellationToken), `serde_json` (Value for arguments), `tracing` (debug/warn logging), `std::panic::catch_unwind` (panic isolation) (031-policy-slots)
- N/A (in-memory policy evaluation; CheckpointPolicy delegates to existing `CheckpointStore` trait) (031-policy-slots)
- Rust 1.88 (edition 2024) + `swink-agent` (core types — policy traits, message types, verdict enums), `regex` (pattern matching for injection/PII/content), `chrono` (timestamps for audit records), `serde`/`serde_json` (audit record serialization), `tracing` (error logging in audit sink) (032-policy-recipes-crate)
- Local filesystem via JSONL (AuditLogger's `JsonlAuditSink` only) (032-policy-recipes-crate)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing` (015-adapter-gemini)
- Rust 1.88 (edition 2024) + Workspace deps centralized in root Cargo.toml. Key deps for this feature: `mistralrs` 0.7 (backend features), `eventsource-stream` 0.2 (proxy-only), `sha2` (bedrock-only). (033-workspace-feature-gates)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `reqwest`, `futures`, `serde`/`serde_json`, `tokio`, `tokio-util`, `tracing`, `rand` (ID generation) (018-adapter-mistral)
- Rust 1.88 (edition 2024) + serde, serde_json, tokio, std::sync::RwLock (no new external deps) (034-session-state-store)
- JSONL via swink-agent-memory crate (extends existing SessionStore) (034-session-state-store)
- Rust 1.88, edition 2024 + `swink-agent` (core types/traits), `reqwest` (OAuth2 refresh HTTP), `chrono` (expiry timestamps), `serde`/`serde_json` (credential serialization), `tokio` (async runtime), `futures` (Shared combinator for dedup), `thiserror` (error types), `tracing` (diagnostics) (035-credential-management)
- In-memory only (`Arc<RwLock<HashMap>>>`). No persistent storage built-in. (035-credential-management)
- Rust 1.88, edition 2024 + `swink-agent` core types (policy traits, AgentTool, AgentEvent), `tracing` (diagnostics) (037-plugin-system)
- N/A (in-memory registry only) (037-plugin-system)
- Rust 1.88 (edition 2024) + `rmcp` (official MCP SDK — stdio, SSE, tool discovery, tool invocation), `swink-agent` (core types: `AgentTool`, `AgentToolResult`, `ContentBlock`, `AgentEvent`), `tokio` (async runtime, subprocess management), `serde`/`serde_json` (serialization), `thiserror` (errors), `tracing` (diagnostics) (038-mcp-integration)
- N/A (in-memory state only — connection handles and discovered tool lists) (038-mcp-integration)
- Rust 1.88, edition 2024 + `reqwest` (HTTP + token acquisition), `tokio` (async runtime), `serde`/`serde_json` (serialization), `tracing` (diagnostics), `swink-agent` (core types). All workspace deps. (016-adapter-azure)
- N/A (in-memory token cache only) (016-adapter-azure)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `swink-agent-adapters` (shared infra: `openai_compat`, `classify`, `sse`, `convert`, `base`) (017-adapter-xai)
- N/A (stateless adapter) (017-adapter-xai)
- Rust 1.88 (edition 2024) + `swink-agent` (core), `swink-agent-adapters` (shared infra), `sha2`/`hmac` (SigV4 signing), `chrono` (timestamps), `aws-smithy-eventstream` (NEW — event-stream frame decoding), `aws-smithy-types` (NEW — event-stream types) (019-adapter-bedrock)
- Rust 1.88 (edition 2024) + `serde`, `serde_json`, `tokio` (fs + sync), `chrono` (timestamps), `tracing` (diagnostics), `thiserror` (errors), `futures` (streaming trait), `schemars` (tool schemas) — all workspace deps (036-artifact-service)
- Local filesystem (versioned files + JSON metadata sidecar); in-memory (`HashMap`) for testing (036-artifact-service)
- Rust 1.88 (edition 2024) + `swink-agent` (path = ".."), `tokio` (async runtime), `tokio-util` (CancellationToken), `serde`/`serde_json` (serialization), `regex` (exit conditions), `uuid` (PipelineId generation), `tracing` (diagnostics), `thiserror` (error types) (039-multi-agent-patterns)
- N/A (in-memory registries only) (039-multi-agent-patterns)
- Rust 1.88 (edition 2024) + `swink-agent` core types (`AgentTool`, `AgentRegistry`, `StopReason`, `AgentToolResult`, `AgentResult`), `serde`/`serde_json` (serialization), `schemars` (tool schema), `tokio-util` (CancellationToken) (040-agent-transfer-handoff)
- N/A (in-memory only) (040-agent-transfer-handoff)
- Rust 1.88, edition 2024 + `mistralrs` 0.8+ (upgrade from 0.7), `hf-hub` 0.5, `tokio`, `futures`, `serde_json`, `tracing` (041-adapter-gemma4-local)
- Local filesystem — model weights cached in `~/.cache/huggingface/hub/` (managed by `hf-hub`) (041-adapter-gemma4-local)
- Rust 1.88 (edition 2024) + `swink-agent` (core — Plugin, AgentTool, policy traits, ContentBlock, AgentEvent), `reqwest` 0.13 (HTTP + redirects), `readability` 0.3 (content extraction), `scraper` 0.23 (HTML parsing / CSS selectors for DuckDuckGo Lite endpoint), `serde`/`serde_json` (serialization), `tokio` (async runtime, subprocess management), `base64` (screenshot encoding), `url` (URL parsing/validation), `regex` (injection pattern matching), `tracing` (diagnostics) (042-web-browse-plugin)
- N/A (in-memory state only — rate limiter counter, Playwright subprocess handle) (042-web-browse-plugin)

## Recent Changes
- 001-workspace-scaffold: Added Rust 1.88 (edition 2024) + serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`)
