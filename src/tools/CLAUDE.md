# CLAUDE.md — Built-in Tools

## Scope

`src/tools/` — BashTool, ReadFileTool, WriteFileTool. Feature-gated behind `builtin-tools` (default-enabled).

## Key Facts

- `MAX_OUTPUT_BYTES` = 100KB, shared for output truncation.
- Two-phase cancellation: pre-check `is_cancelled()` before I/O; BashTool also races via `tokio::select!` and kills child on cancel/timeout.

## Lessons Learned

- **BashTool runs `sh -c`** — not safe for untrusted users.
- **Pipe draining must be concurrent** — stdout/stderr spawned via `tokio::spawn`. Sequential reads deadlock on large outputs (OS pipe buffer fills).
- **New tools** — follow same pattern: schema as `Value` field, validate before execute, cancellation pre-check.
