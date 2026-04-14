# AGENTS.md — Adapters

## Lessons Learned

- In `src/openai_compat.rs`, OpenAI-compatible providers can stream tool-call arguments before `function.name`. Buffer those arguments and delay `ToolCallStart` until a non-empty name is known; otherwise the harness locks in an empty tool name and later deltas cannot repair it.
