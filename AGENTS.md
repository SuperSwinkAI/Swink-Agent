# AGENTS.md — Swink Agent

## Project

Pure-Rust library for LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Workspace: core (`swink-agent`), adapters (`swink-agent-adapters`), memory (`swink-agent-memory`), local-llm (`swink-agent-local-llm`), eval (`swink-agent-eval`), TUI (`swink-agent-tui`).

## Development Principles

- **Test-driven.** Run `cargo test --workspace` before every commit. Bug found → regression test first, then fix.
- **Speed.** Minimize allocations on hot paths. `tokio::spawn` for concurrent tool execution, not sequential awaits.
- **No unsafe.** `#[forbid(unsafe_code)]` at every crate root.
- **Lessons learned go in nested AGENTS.md files.** Update when you discover something non-obvious.

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
- Shared test helpers in `tests/common/mod.rs` (`MockStreamFn`, `MockTool`, `text_only_events`, `tool_call_events`).

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo test -p swink-agent --no-default-features  # verify builtin-tools disabled
cargo clippy --workspace -- -D warnings          # zero warnings policy
cargo run -p swink-agent-tui                     # launch TUI (.env auto-loaded)
```

MSRV **1.88** (edition 2024). Workspace deps centralized in root `Cargo.toml`.

## Lessons Learned

### Agent (`src/agent.rs`)

- `dispatch_event` catches panics via `catch_unwind` and **auto-removes** panicking subscribers (QA-discovered: originally panics were caught but subscribers were not removed).
- `in_flight_llm_messages` filters out `CustomMessage` — they survive compaction but never reach the provider.
- Queues use `Arc<Mutex<>>` with `PoisonError::into_inner()` — never panics on poisoned locks.
- `LoopCheckpoint` is intentionally a resume payload, not a full `LoopState` snapshot: it stores context messages, queued follow-ups, model/system prompt, and session state, but not transient loop counters like `turn_index`, `usage`, `cost`, `overflow_signal`, or `last_assistant_message`.
- Agent internals are split by concern under `src/agent/`: lifecycle helpers live in `control.rs`, event dispatch in `events.rs`, invocation flow in `invoke.rs`, state/tool mutation in `mutation.rs`, pause/resume in `checkpointing.rs`, stream state application in `state_updates.rs`, structured output in `structured_output.rs`, and queue helpers in `queueing.rs`.

### Agent Loop (`src/loop_.rs`)

- Nested outer/inner loop: outer = multi-turn follow-up, inner = single turn.
- `overflow_signal` lives on `LoopState`, **not** `AgentContext`. Resets after `transform_context`.
- `transform_context` is **synchronous** (not async).
- `CONTEXT_OVERFLOW_SENTINEL` triggers overflow retry — loop control signal, not an error.
- Tool dispatch order: Approval → ToolCallTransformer → ToolValidator → Schema validation → `execute()`.
- `RetryStrategy::should_retry()` is the **sole** retryability decision point — `is_retryable()` pre-check was removed.

### Streaming (`src/stream.rs`)

- `accumulate_message` enforces strict ordering: one Start, indexed content blocks, one terminal (Done/Error).
- `partial_json` consumed on `ToolCallEnd` — parsed once. Empty string → `{}`, not null.
- `AssistantMessageEvent::error()` is the canonical error constructor — adapters must use it.
- Adapter SSE parsing should go through `adapters/src/sse.rs` helpers when possible; the proxy adapter no longer uses its own `eventsource-stream` path.
- `adapters/src/sse.rs` is the shared byte-to-SSE-line parser for both generic data-only adapters and Anthropic's event+data pairing logic; avoid reintroducing custom chunk splitters in provider modules.

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
- Approval and API-key callbacks now use shared future/type aliases from `src/agent_options.rs`; prefer reusing those aliases over spelling boxed-future callback signatures inline.
- `src/tool_filter.rs` uses a direct wildcard matcher for glob patterns; avoid recompiling regexes for `*` / `?` matches.

### Policies (`policies/src/content_filter.rs`)

- Non-whole-word keyword filters use direct string matching, while whole-word keywords and explicit regex rules stay on the regex path. Keep the fast keyword path when extending `ContentFilter`.

### Feature Gates

- `builtin-tools` (default-enabled) — gates `BashTool`, `ReadFileTool`, `WriteFileTool`.
- `ProxyStreamFn` lives in adapters crate, not core.

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

## Recent Changes
- 001-workspace-scaffold: Added Rust 1.88 (edition 2024) + serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`)
