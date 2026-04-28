# AGENTS.md — Swink Agent

## Project

Pure-Rust library for LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Workspace crates: core (`swink-agent`), adapters, artifacts, auth, eval, eval-judges, local-llm, macros, MCP, memory, patterns, policies, TUI, web plugin, and xtask.

## Development Principles

- **Test-driven.** `cargo test --workspace` before every commit. Bug → regression test first, then fix.
- **Speed.** Minimize allocations on hot paths. `tokio::spawn` for concurrent tool execution.
- **No unsafe.** `#[forbid(unsafe_code)]` at every crate root.
- **Lessons in nested AGENTS.md.** Update the nearest `AGENTS.md` when you discover something non-obvious.
- **Context7 first.** Query context7 MCP before web search for any crate/library docs.
- **No parallel builds in agents.** Never have subagents run `cargo build`/`test`/`clippy` concurrently.
- **Check specs and docs first.** Read `specs/NNN-*/` and `docs/` before large changes.
- **No GitHub Actions triggers.** Do not create/modify/use workflows that run GitHub Actions.

## Style

- Standard Rust (RFC 430, API Guidelines, clippy defaults). Trailing `_` for reserved-word modules.
- Closure type aliases suffixed `Fn`. `new()` primary; `with_*()` builder chain. Named error constructors.
- No `get_` prefix. `is_*`/`has_*` predicates. `lib.rs` re-exports public API.
- One concern per file; split at ~1500 lines. Imports: `std` → external → `crate::`/`super::`.
- Test names: descriptive `snake_case` without `test_` prefix. Mocks prefixed `Mock`.
- Shared test helpers in `src/testing.rs` (`testkit` feature). Runtime host detection via `TestRuntime`/`should_run_test()`.

## Technologies

| Pillar | Version | Role |
|---|---|---|
| Rust | 1.95 (edition 2024) | Language / MSRV |
| tokio | 1 | Async runtime |
| serde / serde_json | 1 | Serialization |
| reqwest | 0.13 | HTTP |
| schemars / jsonschema | 1 | JSON schema |
| ratatui / crossterm | 0.30 / 0.29 | TUI |
| llama-cpp-2 | latest | Local LLM (llama.cpp) |
| rmcp | latest | MCP SDK |

## Build & Test

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo test --workspace --features testkit
just validate                              # canonical gate
```

Workspace builds compile `swink-agent-local-llm` which needs LLVM/libclang. Set `LIBCLANG_PATH` if auto-discovery fails. Common deps centralized in root `Cargo.toml`.

## Branch Model

- **`integration`** — default branch. All PRs target here. **New work branches off `integration`.**
- **`main`** — stable releases only. Every commit is a tagged crates.io publish.

Always pass `--base integration` to `gh pr create`. Release: squash-merge integration → main + tag. Hotfix: branch off main → fix → merge to main + tag → cherry-pick to integration.

## Contribution Gate

PRs gated by `.github/workflows/pr-gate.yml`: (1) unapproved outside contributors auto-closed, (2) outside contributors must reference an issue. Org members/collaborators exempt.

Approve a contributor: reply `lgtm` (case-insensitive) to any of their issues/PRs (requires admin/maintain). List in `.github/contributors.json`. Post comments via `--body-file` to avoid shell escaping.

## Lessons Learned

See crate-specific `AGENTS.md` files for per-module details. Cross-cutting invariants below.

### Policy Slots (`src/policy.rs`)

- Four slots: PreTurn, PreDispatch, PostTurn, PostLoop. Two verdict enums: `PolicyVerdict` (Continue/Stop/Inject) and `PreDispatchVerdict` (+ Skip, compile-time PreDispatch-only).
- Pre-dispatch panics must restore the prior `arguments` snapshot. `ToolDispatchContext.execution_root` carries the tool's working directory.
- `RetryStrategy::should_retry()` is the sole retryability decision point.

### Plugin System (`src/plugin.rs`)

- `NamespacedTool` prefixes as `"{plugin}_{tool}"` (underscore, not dot). Sanitizes to `^[a-zA-Z][a-zA-Z0-9_]{0,63}$`. Truncated names keep a deterministic hash suffix.
- Namespaced plugin tool colliding with a direct tool: direct wins, plugin skipped with warning. Post-disambiguation must re-check against direct tools.
- Plugin policies prepended (priority-sorted), direct policies appended; plugin tools appended after direct tools.
- `on_init` panics logged and skipped. Entire module behind `#[cfg(feature = "plugins")]`.

### Streaming (`src/stream.rs`)

- `accumulate_message` enforces strict ordering: one Start, indexed content blocks, one terminal (Done/Error).
- `Done(Length)` tolerance only for unfinished `ToolCall` blocks (for `recover_incomplete_tool_calls`); unterminated text/thinking blocks are malformed.
- `AssistantMessageEvent::error()` is the canonical error constructor.

### Context (`src/context.rs`)

- Sliding window: anchor (first N) + tail (recent), middle removed to fit budget.
- Tool-result pairs preserved together even if it exceeds budget. Anchor compaction also preserves tool parity.
- Token estimation: chars/4. `CustomMessage` = 100 tokens flat. `TiktokenCounter` feature-gated behind `tiktoken`.

### Error / Retry

- Retryable: `ModelThrottled` and `NetworkError` only. Custom `RetryStrategy` can override.
- No `MaxTokensReached` variant — handled via `CONTEXT_OVERFLOW_SENTINEL` + loop retry.
- Jitter: `[0.5, 1.5)` × capped delay. Default 3 max attempts.

### Tool System (`src/tool.rs`)

- `AgentToolResult.is_error` — use `error()` / `text()` constructors.
- `ToolCallTransformer` runs unconditionally (not gated by approval). Distinct from `ToolValidator`.
- `#[tool]` macro and `FnTool::with_execute_typed` return `AgentToolResult::error(...)` on serde failures, retry from `{}` for zero-param tools.
- `ToolApprovalRequest` debug keeps `arguments` fully redacted; `redact_sensitive_values()` on context.
- `ScriptTool` TOML uses `[parameters_schema]`, not `[parameters]`.

### Atomic FS / Checkpoint

- `atomic_fs`: single `rename` on Windows (no delete-then-rename). Directory sync only on Unix.
- `add_agent()`/`add_child()` reject duplicate names with a panic (orchestrator).
- `CustomMessage` checkpoint persistence via `custom_messages` field. Old checkpoints backward-compat via `#[serde(default)]`.

## Feature Gates

- `builtin-tools` (default) — `BashTool`, `ReadFileTool`, `WriteFileTool`.
- `testkit` — `testing` module. Not default; add as dev-dep feature.
- `plugins` — `plugin` module. Not default.
- Root crate cannot re-export adapters/local-llm/TUI (cyclic dep). Consumers depend on sub-crates directly.

See per-crate `AGENTS.md` for crate feature gates.
