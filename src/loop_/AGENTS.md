# Loop Lessons

- `ToolExecutionUpdate` must include the originating tool call `id` and `name`, and partial tool updates must flow through an awaited relay instead of `try_send`; otherwise concurrent tool progress becomes unattributed and can be silently dropped under channel backpressure.
