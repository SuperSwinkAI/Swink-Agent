# AGENTS.md — Built-in Tools

## Scope

`src/tools/` — BashTool, EditFileTool, ReadFileTool, WriteFileTool. Feature-gated behind `builtin-tools` (default-enabled).

## Key Invariants

- `MAX_OUTPUT_BYTES` = 100KB shared truncation limit.
- Two-phase cancellation: pre-check `is_cancelled()` before I/O; BashTool also races via `tokio::select!` and kills child on cancel/timeout.
- BashTool runs `sh -c` — not safe for untrusted users.
- Pipe draining must be concurrent (stdout/stderr via `tokio::spawn`). Sequential reads deadlock on large outputs.
- EditFileTool: exact match first, then line-by-line ignoring trailing whitespace. Edits applied in-memory before atomic write (`{filename}.swink-edit.tmp` + rename).
- WriteFileTool `approval_context()` mirrors the `details` shape (`path`/`is_new_file`/`old_content`/`new_content`) so one parser serves both the pre-approval preview and the post-write result. It resolves via `resolve_readable_path_blocking()` — sync, read-only, fails closed to `None` (meaning "no preview", never "write permitted"). Blocking I/O is fine there: the approval path is already gated on human input.
