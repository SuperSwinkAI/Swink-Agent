# AGENTS.md — Built-in Tools

## Scope

`src/tools/` — BashTool, EditFileTool, ReadFileTool, WriteFileTool. Feature-gated behind `builtin-tools` (default-enabled).

## Key Invariants

- `MAX_OUTPUT_BYTES` = 100KB shared truncation limit.
- Two-phase cancellation: pre-check `is_cancelled()` before I/O; BashTool also races via `tokio::select!` and kills child on cancel/timeout.
- BashTool runs `sh -c` — not safe for untrusted users.
- Pipe draining must be concurrent (stdout/stderr via `tokio::spawn`). Sequential reads deadlock on large outputs.
- EditFileTool: exact match first, then line-by-line ignoring trailing whitespace. Edits applied in-memory before atomic write (`{filename}.swink-edit.tmp` + rename).
