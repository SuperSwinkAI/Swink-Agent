# AGENTS.md — swink-agent-memory

## Lessons Learned

- Session transcript saves and session-state saves must share one store-level write path when callers need a consistent snapshot. `SessionStore::save_full()` is the atomic seam for that contract; `JsonlSessionStore` must rewrite messages plus the `_state` line under one lock and one sequence bump so TUI autosave cannot persist mixed transcript/state snapshots.
- `JsonlSessionStore` delete and interrupt-file mutations must lock on the session `.jsonl` path, not their individual file paths. Otherwise a concurrent append/save can recreate a session or orphan an interrupt file after delete returns.
- `SessionStore::save_full()` must not silently fall back to `save()` + `save_state()`. Backends without an explicit atomic implementation now return `io::ErrorKind::Unsupported` so callers cannot assume mixed transcript/state writes are safe.
- File-backed checkpoint persistence must validate checkpoint IDs before turning them into filenames. Reject path separators, `..`, and null bytes so consumer-provided checkpoint IDs cannot escape the configured checkpoint root.
- Stress/perf guards in `memory/tests/` must not hard-fail default `cargo test` runs on fixed wall-clock thresholds. Keep correctness coverage in normal tests and move host-dependent timing assertions into explicit ignored perf-only tests.
