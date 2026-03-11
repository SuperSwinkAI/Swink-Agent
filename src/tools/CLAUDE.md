# CLAUDE.md — Built-in Tools

## Scope

`src/tools/` — BashTool, ReadFileTool, WriteFileTool. Three ordinary `AgentTool` implementations shipped with the harness.

## References

- **PRD:** §4 (Tool System), §4.1 (AgentTool trait), §4.2 (AgentToolResult)
- **Architecture:** `docs/architecture/tool-system/README.md`

## Key Facts

- `MAX_OUTPUT_BYTES` (mod.rs) = 100KB. Shared across BashTool and ReadFileTool for output truncation.
- All tools define inline JSON schemas as `serde_json::Value` in their constructors. Parameters are deserialized with serde at execute time.
- All tools follow the same cancellation contract (see below).

## Cancellation Pattern

Every built-in tool implements two-phase cancellation:

1. **Pre-check** — Call `cancellation_token.is_cancelled()` before starting any I/O. Return error immediately if true.
2. **During work** — BashTool uses `tokio::select!` racing the child process against both the cancellation token and a timeout. ReadFileTool and WriteFileTool do single async operations, so the pre-check is sufficient.

On cancellation or timeout, BashTool kills the child process and waits for it. Reader tasks for stdout/stderr are spawned concurrently and joined after completion to prevent zombie processes.

## Lessons Learned

- **BashTool runs `sh -c`** — arbitrary command execution. Not suitable for agents exposed to untrusted users. This is documented but easy to forget.
- **Pipe draining must be concurrent** — stdout/stderr reader tasks are spawned via `tokio::spawn` while the process runs. Sequential reads deadlock on large outputs because OS pipe buffers fill up.
- **WriteFileTool creates parent directories** — `create_dir_all` is called before writing. Idempotent, no error if parent exists. Uses `if let Some(parent) && let Err(e)` guard clause.
- **Output truncation is proportional** — BashTool splits the MAX_OUTPUT_BYTES budget between stdout and stderr, favoring stdout. Not a hard cut at a byte boundary.
- **Schema consistency** — if you add a new tool, follow the same pattern: schema as Value field, validate before execute, cancellation pre-check.
