# TODO

Project polish checklist — items to complete before/after public launch.

## CI & Testing

- [x] **Property-based testing** — add `proptest` to workspace deps; write property tests for `compact_sliding_window` (token count ≤ budget post-compaction), the streaming accumulator (ordering invariants), and retry jitter (output always within `[0.5, 1.5) × delay`).
- [x] **macOS in CI** — add `macos-latest` runner to the `check` job so platform-specific async and file I/O bugs are caught. The `split-debuginfo = "unpacked"` dev profile setting is macOS-only and currently untested in CI.
- [x] **Examples compiled in CI** — add `cargo build --examples --workspace` to the `check` job. Ensures `custom_agent`, `simple_prompt`, `with_tools`, and `custom_adapter` never silently break as the API evolves.
- [x] **Benchmark regression detection** — integrate `critcmp` or a GitHub Actions workflow that runs criterion benchmarks on PRs and posts a comparison comment. Prevents silent performance regressions.
- [x] **`cargo-mutants` audit** — run `cargo mutants --workspace` periodically (not every PR) to find tests that pass but don't meaningfully assert anything. Already in `.gitignore` (`**/mutants.out*/`).

- [x] **`typos` in CI** — add `typos-cli` as a CI step to catch spelling errors in source code, comments, doc strings, and string literals. Near-zero false positives. Typos in public API names are a semver-breaking fix.
- [x] **Nightly lints on a schedule** — weekly cron CI job running `cargo clippy` and `cargo test` against the nightly toolchain. Early warning of upcoming deprecations before they land in stable and break contributor CI.

## Developer Experience

- [x] **`justfile`** — add a root-level `justfile` with recipes for common commands: `just test`, `just bench`, `just tui`, `just doc`, `just lint`. Reduces contributor friction for remembering feature-flag incantations.
- [x] **`bacon.toml`** — commit a `bacon` watch-mode config so contributors get fast feedback via `bacon test` and `bacon clippy` with zero setup.
- [x] **Pre-commit hooks** — add `.pre-commit-config.yaml` running `cargo fmt` and `cargo check` locally before commits. Closes the feedback loop so contributors catch issues before pushing rather than waiting for CI.

## API Design

- [x] **`#[non_exhaustive]` audit** — review all public enums in `src/types.rs` and `src/error.rs` (`AgentError`, `ContentBlock`, `StopReason`, `AgentEvent`, etc.) and add `#[non_exhaustive]` where appropriate. Without it, adding a variant in a minor release is a breaking change for every downstream match.

## Community

- [ ] **GitHub Discussions** — enable in repo Settings → General → Features. Gives contributors a place for questions, design proposals, and show-and-tell without polluting the issue tracker.

## Future

