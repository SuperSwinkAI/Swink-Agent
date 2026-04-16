# AGENTS.md — xtask

## Scope

`xtask/` — Developer task runner (`cargo xtask <command>`). Not published; workspace-internal tooling only.

## Commands

- `verify-catalog [--provider <key>] [--github]` — hits each provider's live API to confirm every model ID in `src/model_catalog.toml` resolves. `--github` writes a Markdown summary to `$GITHUB_STEP_SUMMARY` for CI annotations.

## Key Facts

- Lives in `xtask/src/`: `main.rs` (CLI), `verifier.rs` (per-provider checks), `catalog.rs` (catalog parsing), `report.rs` (output formatting).
- Requires live API keys in the environment — not suitable for offline CI.
- Run via `cargo xtask verify-catalog` from the workspace root.
