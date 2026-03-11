# CLAUDE.md — Agent Harness

## Project

A pure-Rust library for building LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events. Three-crate workspace: core (`agent-harness`), adapters (`agent-harness-adapters`), TUI (`agent-harness-tui`).

## Development Principles

- **Test-driven development.** Write tests before or alongside implementation. Run `cargo test --workspace` before every commit. If a bug is found, write a regression test first, then fix.
- **Speed.** Minimize allocations in the hot path (streaming, tool dispatch). Prefer `&str` over `String` where lifetimes allow. Use `tokio::spawn` for concurrent tool execution, not sequential awaits.
- **Maintainability.** Keep modules small and focused. One concern per file. Traits at boundaries (`StreamFn`, `AgentTool`, `RetryStrategy`). No `unsafe` code (enforced by `#[forbid(unsafe_code)]`).
- **Lessons learned go in nested CLAUDE.md files.** Each subdirectory with a CLAUDE.md captures key patterns, gotchas, and decisions specific to that area of code. Update them when you discover something non-obvious.

## Build & Test

```bash
cargo build --workspace          # compile all crates
cargo test --workspace           # run all tests
cargo clippy --workspace -- -D warnings  # lint (zero warnings policy)
cargo run -p agent-harness-tui   # launch TUI (.env auto-loaded via dotenvy)
```

MSRV is **1.88** (edition 2024). `rust-toolchain.toml` pins to stable channel.

## Architecture Quick Reference

| Module | PRD Section | Architecture Doc | Purpose |
|---|---|---|---|
| `src/types.rs` | §3 | `docs/architecture/data-model/` | Content blocks, messages, Usage, Cost, AgentResult |
| `src/tool.rs` | §4 | `docs/architecture/tool-system/` | AgentTool trait, JSON Schema validation |
| `src/context.rs` | §5 | `docs/architecture/agent-context/` | Sliding window, two-context design |
| `src/agent.rs` | §6, §13 | `docs/architecture/agent/` | Agent struct, state, queues, subscriptions |
| `src/stream.rs` | §7 | `docs/architecture/streaming/` | StreamFn trait, AssistantMessageEvent protocol |
| `src/proxy.rs` | §7.4 | `docs/architecture/streaming/` | ProxyStreamFn (SSE) |
| `src/loop_.rs` | §8, §9, §12 | `docs/architecture/agent-loop/` | Agent loop, events, cancellation |
| `src/error.rs` | §10 | `docs/architecture/error-handling/` | HarnessError variants, retryable classification |
| `src/retry.rs` | §11 | `docs/architecture/error-handling/` | RetryStrategy trait, exponential backoff |
| `src/tools/` | §4 | `docs/architecture/tool-system/` | BashTool, ReadFileTool, WriteFileTool |
| `adapters/` | §7, §14.1, §15.1 | `docs/architecture/streaming/` | Ollama, Anthropic, OpenAI StreamFn adapters |
| `tui/` | §16 | `docs/architecture/tui/` | Terminal UI binary |

## Core Module Lessons Learned

### Agent (`src/agent.rs`) — PRD §6, §13

- `dispatch_event(&mut self)` catches panics via `catch_unwind` and auto-removes panicking subscribers. This was a QA-discovered bug — originally panics were caught but subscribers were not removed.
- SteeringMode and FollowUpMode default to `OneAtATime`, not `All`. One message per poll drains from queues.
- Queues use `Arc<Mutex<>>` with `PoisonError::into_inner()` — never panics on poisoned locks.
- `idle_notify` uses Tokio `Notify` pattern — `wait_for_idle()` blocks callers until the loop calls `notify_waiters()`.
- `in_flight_llm_messages` filters out `CustomMessage` variants — they survive context compaction but never reach the provider.

### Agent Loop (`src/loop_.rs`) — PRD §8, §9, §12

- Nested outer/inner loop: outer drives multi-turn (follow-up), inner is a single turn.
- `overflow_signal` lives on `LoopState`, **not** `AgentContext`. Resets after `transform_context` is called.
- `transform_context` is **synchronous** (not async). Runs on the main loop task.
- CancellationToken hierarchy: child token per tool batch. Steering interrupts cancel the batch mid-flight.
- `CONTEXT_OVERFLOW_SENTINEL` triggers overflow retry — this is not an error, it's a loop control signal.

### Streaming (`src/stream.rs`) — PRD §7

- `StreamFn` requires `Send + Sync` and is stored as `Arc<dyn StreamFn>`.
- `accumulate_message` enforces strict event ordering: exactly one Start, indexed content blocks, exactly one terminal event (Done/Error).
- `partial_json` is consumed on `ToolCallEnd` — parsed exactly once. Empty string becomes `{}`, not null.

### Context (`src/context.rs`) — PRD §5

- Sliding window keeps anchor (first N messages) + tail (recent). Removes middle to fit budget.
- Overflow budget is smaller than normal budget — forces more aggressive pruning on retry.
- Tool-result pairs are preserved together even if it exceeds budget. Correctness > token count.
- Token estimation uses chars/4 heuristic. CustomMessage = 100 tokens flat.

### Error Handling (`src/error.rs`) — PRD §10

- `is_retryable()` returns true only for `ModelThrottled` and `NetworkError`. Everything else is terminal.
- There is **no** `MaxTokensReached` error variant. Max tokens is handled silently via `CONTEXT_OVERFLOW_SENTINEL` + loop retry.
- Error classification happens in `loop_.rs::classify_stream_error()`, not in error.rs.

### Types (`src/types.rs`) — PRD §3

- Usage/Cost implement `Add + AddAssign`. No overflow checks.
- AgentResult includes both `usage` (token counts) and `cost` (floating-point dollars).
- Compile-time `Send + Sync` assertions at module bottom. Compile error if a type loses thread safety.

### Tool System (`src/tool.rs`) — PRD §4

- `AgentTool::execute()` returns `Pin<Box<dyn Future>>` for object safety.
- `validate_tool_arguments` runs before `execute()` — if validation fails, tool is never executed.
- `validation_error_result` joins all schema violations with newlines into a single text block.

### Retry (`src/retry.rs`) — PRD §11

- Jitter range: `[0.5, 1.5)` — multiplies capped delay by `0.5 + rand()`.
- Formula: `base_delay * multiplier^(attempt-1)`, capped at `max_delay` (60s).
- Default 3 max attempts — attempts 1-2 retry, attempt 3 fails without retry.
