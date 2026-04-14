# AGENTS.md — eval

## Lessons Learned

- `FsEvalStore` must validate eval set IDs before any path join. Reject empty IDs, `.`/`..`, NUL, and both `/` and `\` separators even on non-Windows hosts so logical identifiers cannot escape `sets/` or `results/` when tests or artifacts move across platforms.
- `FsEvalStore` set/result persistence must go through `swink_agent::atomic_fs` helpers rather than direct `fs::write`, so interrupted rewrites never leave partial JSON or clobber the last good file.
