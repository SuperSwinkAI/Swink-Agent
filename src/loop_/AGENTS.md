# Loop Lessons

- `ToolExecutionUpdate` must include the originating tool call `id` and `name`, and partial tool updates must flow through an awaited relay instead of `try_send`; otherwise concurrent tool progress becomes unattributed and can be silently dropped under channel backpressure.
- Post-turn assistant replacements must preserve the original `ToolCall` blocks. A text-only replacement on a tool turn breaks the core invariant that assistant tool calls stay paired with the committed `ToolResult` messages that follow.
