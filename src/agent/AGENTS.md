# Agent Lessons

- `pause()` must snapshot the full in-flight message history, not just `in_flight_llm_messages`. The LLM-only snapshot intentionally drops `CustomMessage`, so streamed pause/resume needs a separate authoritative in-flight message clone to avoid serializing stale or incomplete checkpoints.
- Checkpoint restore must validate the optional `session_state` snapshot before mutating `messages`, `system_prompt`, `model`, or live queues. A corrupt state snapshot must leave the in-memory agent unchanged.
