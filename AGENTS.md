# AGENTS.md — Swink Agent

## Project

Pure-Rust library for LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Workspace crates: core (`swink-agent`), adapters (`swink-agent-adapters`), artifacts (`swink-agent-artifacts`), auth (`swink-agent-auth`), eval (`swink-agent-eval`), local-llm (`swink-agent-local-llm`), macros (`swink-agent-macros`), MCP (`swink-agent-mcp`), memory (`swink-agent-memory`), patterns (`swink-agent-patterns`), policies (`swink-agent-policies`), TUI (`swink-agent-tui`), web plugin (`swink-agent-plugin-web`), and `xtask`.

## Development Principles

- **Test-driven.** Run `cargo test --workspace` before every commit. Bug found → regression test first, then fix.
- **Speed.** Minimize allocations on hot paths. `tokio::spawn` for concurrent tool execution, not sequential awaits.
- **No unsafe.** `#[forbid(unsafe_code)]` at every crate root.
- **Lessons learned go in nested AGENTS.md files.** Update the nearest `AGENTS.md` when you discover something non-obvious.
- **Context7 first.** When researching any crate API, dependency docs, or library usage, always query the context7 MCP server before falling back to web search or training data. Training data may be stale; context7 pulls current docs.
- **No parallel builds in agents.** Never have multiple subagents run `cargo build`/`test`/`clippy` concurrently — Cargo's global lock serializes them anyway. Run all compilation in the main conversation first; subagents should only read and analyze code.
- **Check specs and docs first.** Before making large changes, read the relevant spec files in `specs/NNN-*/` and architecture docs in `docs/`. The project uses spec-driven development — changes should align with the agreed design.
- **No GitHub Actions triggers.** Do not create, modify, or use workflows/events that run GitHub Actions for this repo.
- **graphify skill available.** When the user types `/graphify`, invoke the Skill tool with `skill: "graphify"` before doing anything else — converts any input to a knowledge graph.

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
- Prefer runtime-gated live tests over ad hoc host checks. OS/GPU detection lives in `swink_agent::testing` (`TestRuntime`, `TestRuntimeRequirements`, `should_run_test()`), so live/integration tests should skip explicitly on unsupported hosts instead of hanging or failing deep in model startup.

## Active Technologies

| Pillar | Version | Role |
|---|---|---|
| Rust | 1.88 (edition 2024) | Language / MSRV |
| tokio | 1 | Async runtime |
| serde / serde_json | 1 | Serialization |
| reqwest | 0.13 | HTTP client |
| schemars / jsonschema | 1 | JSON schema gen + validation |
| ratatui / crossterm | 0.30 / 0.29 | TUI rendering |
| llama-cpp-2 | latest | Local LLM inference (llama.cpp Rust bindings) |
| rmcp | latest | MCP SDK (stdio + SSE) |

## Build & Test

```bash
cargo fmt --all --check                           # formatting
cargo clippy --workspace -- -D warnings           # zero warnings policy
cargo test --workspace                            # full workspace tests
cargo build --workspace                           # full workspace build
cargo test --workspace --features testkit         # testkit enables test helpers
cargo test -p swink-agent --no-default-features   # verify builtin-tools disabled
just validate                                     # same canonical gate via justfile
cargo run -p swink-agent-tui                      # launch TUI (.env auto-loaded)
```

Workspace-wide `build` / `test` / `clippy` commands compile `swink-agent-local-llm`, which currently pulls `llama-cpp-sys-2` and its `bindgen` build step. Install LLVM/libclang first and set `LIBCLANG_PATH` if your platform does not auto-discover the shared library (especially common on Windows).

MSRV **1.88** (edition 2024). Common workspace deps are centralized in root `Cargo.toml`, with a few crate-specific dependencies declared locally where needed.

## Branch Model

Two long-lived branches:
- **`integration`** — default branch. All feature PRs target here. **New work always branches off `integration`.**
- **`main`** — stable releases only. Every commit is a tagged crates.io publish.

Release flow: squash-merge `integration` → `main` + push version tag → crates.io publish triggered automatically.
Hotfix flow: branch off `main` → fix → squash-merge to `main` + tag → cherry-pick to `integration`.

## Contribution Gate

PRs are gated by two sequential checks in `.github/workflows/pr-gate.yml`:

1. **Contributor approval** — PRs from unapproved outside contributors are auto-closed. Issues are open to everyone.
2. **Issue reference** — outside contributors must reference an existing issue via `#N`, `Closes #N`, `Related to #N`, etc. PRs with no linked issue are auto-closed. Repo collaborators are exempt from this check.

`.github/workflows/approve-contributor.yml` handles maintainer approval comments.

Approved contributors are stored in `.github/contributors.json`:
```json
{ "lgtm": ["user-a"] }
```

**Org members and repo collaborators are always allowed** — the gate only applies to outside contributors.

### Approving a contributor

Reply `lgtm` (alone on the comment, case-insensitive) to any issue or PR from that contributor. The `approve-contributor.yml` workflow will:
1. Verify the commenter has admin or maintain permission on the repo.
2. Add the issue/PR author to `.github/contributors.json` via the GitHub API.
3. Reopen the PR if it was auto-closed.
4. Reply with a confirmation comment.

### Handling issues and PRs as a maintainer

When reviewing auto-closed issues daily:

```bash
# View auto-closed issues
gh issue list --state closed --label "auto-closed" --limit 20

# Reopen a worthwhile issue (approve-contributor handles this automatically when you comment lgtmi/lgtm)
gh issue comment <number> --body "lgtmi"

# Close spam permanently (no comment needed, just close and block)
gh issue close <number>
```

When working a PR after approval:

```bash
# Review diff without pulling locally first
gh pr diff <number>
gh pr view <number>

# If approved: checkout PR branch, rebase onto integration, merge
gh pr checkout <number>
git rebase integration
# ... review / adjust ...
git checkout integration && git merge --ff-only <branch> && git push
gh pr close <number> --comment "Merged via integration."
```

Post comments via file to avoid shell escaping issues:
```bash
gh issue comment <number> --body-file /tmp/comment.md
gh pr comment <number> --body-file /tmp/comment.md
```

## Lessons Learned

### Agent (`src/agent.rs`)

- `dispatch_event` catches panics via `catch_unwind` and **auto-removes** panicking subscribers (QA-discovered: originally panics were caught but subscribers were not removed).
- `in_flight_llm_messages` filters out `CustomMessage` — they survive compaction but never reach the provider.
- Queues use `Arc<Mutex<>>` with `PoisonError::into_inner()` — never panics on poisoned locks.
- `dispatch_event` wraps event forwarders in `catch_unwind` — panicking forwarders are logged and skipped, not auto-removed (unlike listeners).
- `AgentId` lives in its own module `src/agent_id.rs` (extracted from `registry.rs` to break the `agent.rs` ↔ `registry.rs` circular import).
- `reset()` must call `idle_notify.notify_waiters()` after clearing `loop_active`; `wait_for_idle()` depends on that wakeup for pending waiters when a run is cancelled/reset before the normal stream-end path fires.

### Agent Loop (`src/loop_.rs`)

- Nested outer/inner loop: outer = multi-turn follow-up, inner = single turn.
- `overflow_signal` lives on `LoopState`, **not** `AgentContext`. Resets after `transform_context`.
- `transform_context` is **synchronous** (not async).
- `CONTEXT_OVERFLOW_SENTINEL` triggers overflow retry — loop control signal, not an error.
- Tool dispatch order: PreDispatch policies → Approval → Schema validation → `execute()`.

### Policy Slots (`src/policy.rs`)

- Four configurable policy slots: PreTurn, PreDispatch, PostTurn, PostLoop.
- Two verdict enums: `PolicyVerdict` (Continue/Stop/Inject) and `PreDispatchVerdict` (+ Skip). Compile-time enforcement that Skip is only valid in PreDispatch.
- Slot runner uses `AssertUnwindSafe` + `catch_unwind` — traits only need `Send + Sync`, not `UnwindSafe`.
- Pre-dispatch policy panics must restore the prior `arguments` snapshot before later policies or approval run.
- `ToolDispatchContext.execution_root` carries the tool's working directory. Policies that validate relative paths must reject them when this context is absent instead of assuming lexical containment.
- `RetryStrategy::should_retry()` is the **sole** retryability decision point — `is_retryable()` pre-check was removed.

### Policies Crate (`policies/`)

- Separate workspace crate — depends only on `swink-agent` public API, no internal imports. See `policies/AGENTS.md` for implementation details.
- `SandboxPolicy` must resolve checked paths against a canonical allowed root **plus** `ToolDispatchContext.execution_root`; lexical `starts_with` checks are insufficient because relative paths and symlinked parents can escape the sandbox.

### Plugin System (`src/plugin.rs`)

- `Plugin` trait requires only `name()` — all contribution methods default to no-op/empty.
- `PluginRegistry` deduplicates by name on `register()` — the new plugin **replaces** the old one (with a `tracing::warn`), not an error.
- `list()` returns plugins sorted by priority descending; insertion order preserved for ties.
- `NamespacedTool` prefixes as `"{plugin_name}_{tool_name}"` (underscore, not dot — Anthropic/Bedrock/OpenAI reject dots) and sanitizes both components to the common subset `^[a-zA-Z][a-zA-Z0-9_]{0,63}$` accepted by every provider. Prevents tool name collisions across plugins and guarantees wire-level validity.
- Long namespaced tool names must keep a deterministic hash suffix when truncated; raw prefix truncation can collapse distinct plugin/tool pairs onto the same wire name.
- Contributions merged in `Agent::new()`: plugin policies **prepended** (priority-sorted), direct policies appended; plugin tools appended after direct tools.
- `Agent::new()` and `Agent::set_tools()` must reject duplicate final tool names after composition instead of relying on dispatch's "keep first" fallback; schema export and lookup need the same unique wire-name set.
- `on_init(&self, &Agent)` dispatched in priority order, wrapped in `catch_unwind` — panicking `on_init` is logged and skipped, construction continues.
- Entire module behind `#[cfg(feature = "plugins")]` — opt-in, not default-enabled, zero cost when disabled.

### Orchestrator (`src/orchestrator.rs`)

- `AgentOrchestrator::add_agent()` and `add_child()` reject duplicate names with a panic instead of replacing existing entries. This preserves `parent_of()` / `children_of()` consistency until a dedicated replace/relink API exists.

### MCP Integration (`mcp/`)

- `McpManager::shutdown()` must operate on shared connection-owned state, not `Arc::try_unwrap()`: exported `McpTool` handles keep `Arc<McpConnection>` clones alive, so deterministic disconnect has to clear the session/monitor even when callers still retain tool `Arc`s.

### Streaming (`src/stream.rs`)

- `StreamErrorKind` lives in `src/stream_error_kind.rs`; `stream.rs` re-exports it so `AssistantMessageEvent` helpers and the crate root keep the same public API while `types` stays decoupled from `stream`.
- `accumulate_message` enforces strict ordering: one Start, indexed content blocks, one terminal (Done/Error).
- `partial_json` consumed on `ToolCallEnd` — parsed once. Empty string → `{}`, not null.
- `Done(Length)` tolerance is only for unfinished `ToolCall` blocks that preserve `partial_json` for `recover_incomplete_tool_calls`; unterminated text/thinking blocks must still be rejected as malformed.
- `AssistantMessageEvent::error()` is the canonical error constructor — adapters must use it.

### Test Infrastructure (`src/testing.rs`)

- Runtime host detection for tests is centralized in `swink_agent::testing`. Use `test_runtime()` / `should_run_test()` with `TestRuntimeRequirements` instead of duplicating `cfg!`, `system_profiler`, or `nvidia-smi` logic in individual test files.
- Gemma 4 live tests must gate on both compiled backend and detected hardware (`metal` implies macOS + Apple Silicon; `cuda`/`cudnn` implies NVIDIA). Unsupported hosts should print a clear `skipping:` reason and return early.
- Adapter crates that reuse these helpers need a dev-dependency on `swink-agent` with the `testkit` feature enabled.

### Context (`src/context.rs`)

- Sliding window: anchor (first N) + tail (recent), middle removed to fit budget.
- Tool-result pairs preserved together even if it exceeds budget. Correctness > token count.
- Token estimation: chars/4 heuristic. CustomMessage = 100 tokens flat.
- `TiktokenCounter` is feature-gated behind `tiktoken`; it keeps `CustomMessage` at the same flat 100-token estimate because those messages never reach provider tokenizers.

### Error / Retry

- `is_retryable()` = true only for `ModelThrottled` and `NetworkError`. Custom `RetryStrategy` can override.
- No `MaxTokensReached` variant — handled via `CONTEXT_OVERFLOW_SENTINEL` + loop retry.
- Retry jitter: `[0.5, 1.5)` × capped delay. Default 3 max attempts.

### Tool System (`src/tool.rs`)

- `AgentToolResult.is_error` replaces old `text.starts_with("error")` heuristic. Use `error()` / `text()` constructors.
- `ToolCallTransformer` runs unconditionally (not gated by approval). Distinct from `ToolValidator` (rejects vs rewrites).
- `#[tool]` macro param decoding must return `AgentToolResult::error("invalid parameters: ...")` on serde failures rather than panicking. The generated code still retries deserialization from `{}` so zero-param / all-optional tools can execute when appropriate.
- `FnTool::with_execute_typed::<T>()` is the zero-boilerplate typed path: it derives the schema from `T` and must mirror the rest of the tool stack by returning `AgentToolResult::error("invalid parameters: ...")` on serde decode failures.
- `ToolApprovalRequest` debug output must keep `arguments` fully redacted and run `redact_sensitive_values()` over `context`. MCP tools intentionally pass raw params through `approval_context()` for policy/approval inspection, so the logging boundary is where sanitization has to happen.

### Credential Management (`auth/`)

- `DefaultCredentialResolver` can reuse a per-key `SingleFlightTokenSource`, but the credential store remains the source of truth. Clear the token source's cached value before resolving an expired key from the store, or a previously refreshed token can mask later external store updates.

### Atomic FS (`src/atomic_fs.rs`)

- `atomic_fs` must keep replacement to a single `std::fs::rename` call on Windows. Delete-then-rename widens the crash window enough to lose both old and new files.
- `atomic_fs` only syncs the parent directory on Unix where directory `sync_all()` is supported.

### Checkpoint / Persistence

- Checkpoint and SessionStore support `CustomMessage` persistence via `custom_messages` field and `save_full`/`load_full`. Old checkpoints without `custom_messages` deserialize fine (backward compat via `#[serde(default)]`).

## Feature Gates

**Root crate (`swink-agent`):**
- `builtin-tools` (default-enabled) — gates `BashTool`, `ReadFileTool`, `WriteFileTool`.
- `testkit` — gates the `testing` module. Not default-enabled; consumers add `features = ["testkit"]` in dev-dependencies. Integration tests in `/tests/` are gated with `#![cfg(feature = "testkit")]`.
- `plugins` — gates `plugin` module. Not default-enabled. `MockPlugin` in `testing.rs` also gated behind this feature.
- Root crate cannot re-export adapters/local-llm/TUI (cyclic dependency). Consumers depend on sub-crates directly.

See `adapters/AGENTS.md`, `local-llm/AGENTS.md`, and `policies/AGENTS.md` for per-crate feature gate details.
