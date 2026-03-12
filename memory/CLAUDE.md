# CLAUDE.md — Memory & Session Management

## Scope

`memory/` — Session persistence and memory management for swink-agent. Provides pluggable session storage (`SessionStore` trait), a JSONL file backend (`JsonlSessionStore`), and summarization-aware context compaction (`SummarizingCompactor`).

## References

- **PRD:** §5 (Agent Context), §10.1 (Context Window Overflow)
- **Architecture:** `memory/docs/architecture/README.md` (multi-layer memory vision)
- **Research:** `memory/docs/planning/research.md` (experiment designs)

## Key Facts

- `SessionStore` is the trait; `JsonlSessionStore` is the concrete JSONL implementation extracted from `tui/src/session.rs`.
- JSONL format: line 1 = `SessionMeta` (JSON), lines 2+ = one `LlmMessage` per line. `CustomMessage` variants are filtered out (not serializable).
- `SummarizingCompactor::compaction_fn()` returns a closure compatible with `Agent::with_transform_context()`. It wraps `sliding_window` and injects a pre-computed summary after anchor messages when compaction occurs.
- Summary injection is synchronous (runs inside `TransformContextFn`). Summary generation is async and happens outside the agent loop — callers provide the summary text via `set_summary()`.
- `time.rs` helpers (`days_to_ymd`, `unix_now`) are `pub(crate)` — internal only.

## Lessons Learned

- **`TransformContextFn` is synchronous** — cannot make LLM calls inside it. The pattern is: pre-compute summaries async after each turn, store them, then the sync compaction pass injects the stored summary.
- **Summary is injected as `AssistantMessage`** — maintains user/assistant alternation since the anchor typically starts with a user message.
- **`PoisonError::into_inner()`** — matches the pattern in `agent.rs` for `Mutex` guards. Never panics on poisoned locks.
