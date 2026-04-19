# AGENTS.md — swink-agent-memory

## Lessons Learned

- Session transcript saves and session-state saves must share one store-level write path when callers need a consistent snapshot. `SessionStore::save_full()` is the atomic seam for that contract; `JsonlSessionStore` must rewrite messages plus the `_state` line under one lock and one sequence bump so TUI autosave cannot persist mixed transcript/state snapshots.
- File-backed checkpoint persistence must validate checkpoint IDs before turning them into filenames. Reject path separators, `..`, and null bytes so consumer-provided checkpoint IDs cannot escape the configured checkpoint root.
