# AGENTS.md — loop_

## Lessons Learned

- `ToolExecutionStart` is reserved for tool calls that reach the committed execution path. Emit it only after pre-dispatch policies, approval, and schema validation succeed, and use the effective arguments that will actually be executed.
