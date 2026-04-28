# AGENTS.md — swink-agent-memory

## Key Invariants

- `SessionStore::save_full()` is the atomic seam for transcript + state persistence. `JsonlSessionStore` rewrites both under one lock/sequence bump.
- `JsonlSessionStore::append()` is true append: reserves padding on metadata line, only full-rewrites when metadata can't fit.
- In-place append patches metadata sequence before writing records (crash-safe ordering).
- Delete and interrupt-file mutations lock on the session `.jsonl` path, not individual file paths.
- `save_full()` returns `Unsupported` (not silent fallback) for backends without atomic implementation.
- Checkpoint IDs validated before becoming filenames (reject separators, `..`, `:`, control chars).
- Timing-dependent perf guards go in `#[ignore]` tests, not default `cargo test`.
