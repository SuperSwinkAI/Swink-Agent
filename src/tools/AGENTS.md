# AGENTS.md — Built-in Tools

## Scope

`src/tools/` — BashTool, EditFileTool, ReadFileTool, WriteFileTool. Feature-gated behind `builtin-tools` (default-enabled).

## Key Facts

- `MAX_OUTPUT_BYTES` = 100KB, shared for output truncation.
- Two-phase cancellation: pre-check `is_cancelled()` before I/O; BashTool also races via `tokio::select!` and kills child on cancel/timeout.

## Lessons Learned

- **BashTool runs `sh -c`** — not safe for untrusted users.
- **Pipe draining must be concurrent** — stdout/stderr spawned via `tokio::spawn`. Sequential reads deadlock on large outputs (OS pipe buffer fills).
- **EditFileTool matching** — tries exact string match first; falls back to line-by-line match ignoring trailing whitespace. Returns ranges in the original (un-normalised) content so the replacement is byte-accurate. Edits are applied in-memory (fail-fast) before any atomic write.
- **EditFileTool atomic write** — writes to `{filename}.swink-edit.tmp` in the same directory, then renames over the target. On most Unix filesystems `rename` is atomic when src and dst share a directory.
- **New tools** — follow same pattern: schema as `Value` field, validate before execute, cancellation pre-check.
