# AGENTS.md — Adapters

## Lessons Learned

- In `src/openai_compat.rs`, OpenAI-compatible providers can stream tool-call arguments before `function.name`. Buffer those arguments and delay `ToolCallStart` until a non-empty name is known; otherwise the harness locks in an empty tool name and later deltas cannot repair it.
- Runtime SSE adapters must thread `StreamOptions.on_raw_payload` into the callback-aware shared parser (`sse_data_lines_with_callback` or an equivalent hook). Calling the callback-free helper silently disables payload observers in production even though the shared SSE unit tests still pass.
