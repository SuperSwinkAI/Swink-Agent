# AGENTS.md — eval

## Lessons Learned

- `FsEvalStore` must validate eval set IDs before any path join. Reject empty IDs, `.`/`..`, NUL, and both `/` and `\` separators even on non-Windows hosts so logical identifiers cannot escape `sets/` or `results/` when tests or artifacts move across platforms.
