# CLAUDE.md — Swink Agent

## Project

Pure-Rust library for LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Workspace: core (`swink-agent`), adapters (`swink-agent-adapters`), memory (`swink-agent-memory`), local-llm (`swink-agent-local-llm`), eval (`swink-agent-eval`), TUI (`swink-agent-tui`).

## Development Principles

- **Test-driven.** Run `cargo test --workspace` before every commit. Bug found → regression test first, then fix.
- **Speed.** Minimize allocations on hot paths. `tokio::spawn` for concurrent tool execution, not sequential awaits.
- **No unsafe.** `#[forbid(unsafe_code)]` at every crate root.
- **Lessons learned go in nested CLAUDE.md files.** Update when you discover something non-obvious.

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

### Feature Gates

- `builtin-tools` (default-enabled) — gates `BashTool`, `ReadFileTool`, `WriteFileTool`.
- `ProxyStreamFn` lives in adapters crate, not core.

## Active Technologies
- Rust 1.88 (edition 2024) + serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`) (001-workspace-scaffold)
- N/A (scaffold only) (001-workspace-scaffold)
- Rust 1.88 (edition 2024) + serde, serde_json, thiserror, uuid, schemars (all workspace deps) (002-foundation-types-errors)
- N/A (in-memory types only) (002-foundation-types-errors)

## Recent Changes
- 001-workspace-scaffold: Added Rust 1.88 (edition 2024) + serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`)
