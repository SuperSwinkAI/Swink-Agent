# AGENTS.md — Adapters

## Lessons Learned

- In `src/ollama.rs`, the NDJSON parser must buffer raw bytes until it has a full newline-delimited record. Decoding each transport chunk independently with `from_utf8_lossy` corrupts split multibyte UTF-8 in streamed text and tool arguments.
- In `src/openai_compat.rs`, OpenAI-compatible providers can stream tool-call arguments before `function.name`. Buffer those arguments and delay `ToolCallStart` until a non-empty name is known; otherwise the harness locks in an empty tool name and later deltas cannot repair it.
- Runtime SSE adapters must thread `StreamOptions.on_raw_payload` into the callback-aware shared parser (`sse_data_lines_with_callback` or an equivalent hook). Calling the callback-free helper silently disables payload observers in production even though the shared SSE unit tests still pass.
- In `src/proxy.rs`, treat transport `data: [DONE]` as a protocol error unless the proxy has already emitted a typed `done` or `error` JSON event. The SSE sentinel only closes the transport; it is not the adapter's semantic terminal event.
- In `tests/no_default_features.rs`, keep feature-leak sentinels behind an explicit hidden cargo feature (currently `__no_default_features_sentinel`). Workspace test runs can unify adapter features from other packages, so an always-on sentinel becomes a false failure even though the dedicated `--no-default-features` check is still valid.
