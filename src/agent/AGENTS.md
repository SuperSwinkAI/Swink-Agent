# Agent Lessons

- `pause()` must snapshot the full in-flight message history, not just `in_flight_llm_messages`. The LLM-only snapshot intentionally drops `CustomMessage`, so streamed pause/resume needs a separate authoritative in-flight message clone to avoid serializing stale or incomplete checkpoints.
