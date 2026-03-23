# CLAUDE.md — Memory & Session Management

## Scope

`memory/` — Session persistence and summarization-aware context compaction.

## Key Facts

- `SessionStore` trait; `JsonlSessionStore` concrete backend. JSONL: line 1 = `SessionMeta`, lines 2+ = `LlmMessage` (or custom message envelopes when using `save_full`/`load_full`). `save_full`/`load_full` support full `AgentMessage` including custom messages via `serialize_custom_message`/`deserialize_custom_message`.
- `SummarizingCompactor::compaction_fn()` returns closure for `Agent::with_transform_context()`.

## Lessons Learned

- **`TransformContextFn` is synchronous** — cannot make LLM calls inside it. Pattern: pre-compute summaries async after each turn via `set_summary()`, then sync compaction injects them.
- **Summary injected as `AssistantMessage`** — maintains user/assistant alternation since anchor starts with user message.
