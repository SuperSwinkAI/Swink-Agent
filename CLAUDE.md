# CLAUDE.md — Swink Agent

Pure-Rust workspace for LLM-powered agentic loops: provider-agnostic core (`swink-agent`, at repo root) plus the member crates listed in `Cargo.toml`. MSRV 1.95, edition 2024, `unsafe_code = "forbid"`.

`AGENTS.md` (root and nested) is the tool-agnostic engineering manual: development principles, style, technologies, per-crate invariants. Read it first — it is the source of truth wherever the two files overlap. This file adds the agent-facing operating layer: commands, workflow, and gotchas.

## Commands

`just` routes cargo through `scripts/cargo-with-sccache.sh`.

- `just test` — `cargo nextest run --workspace`; large suite, allow 10+ min
- `just lint` — `cargo clippy --workspace -- -D warnings`; pedantic + nursery are warn-level workspace-wide
- `just fmt` / `just fmt-check` — rustfmt
- `just doc` — rustdoc with `-D warnings`; catches broken intra-doc links clippy misses
- `just validate` (alias `just check`) — pre-PR gate: fmt-check, clippy, tests, build, testkit tests, plugin tests, no-default sentinels, `package-preflight` (`cargo publish --workspace --dry-run --locked`). Very long; background it
- Targeted: `just test-plugins`, `just test-no-features`, `just no-default-sentinels`, `just test-testkit`, `just coverage`, `just bench`, `just tui`

## Git workflow

- **`integration` is the default branch.** Branch from it, PR back into it. `main` carries releases only — never merge feature work there directly.
- Delete merged branches (local and remote) as soon as they land.
- Stay on one branch across all speckit phases (clarify → plan → tasks → analyze) unless told otherwise.
- CI runs `--locked`; commit an updated `Cargo.lock` with any dependency or version change.
- Do not create, modify, or trigger GitHub Actions workflows.

## Gotchas

- **Hooks add latency, not hangs**: `post-commit`/`post-checkout` rebuild a graphify knowledge graph (seconds per commit and branch switch); `.pre-commit-config.yaml` runs cargo fmt + cargo check.
- **Never run cargo concurrently** — one shared target dir and sccache. This includes subagents and other repos on the machine.
- **Interrupted `llama-cpp-sys-2` builds poison its build dir durably** (`.always_configure(false)` skips reconfigure, so the installed-libs glob keeps finding zero and `build.rs` panics). Fix: `cargo clean -p llama-cpp-sys-2 [--release]`, matching the failing profile, then rebuild.
- **`local-llm` needs LLVM/libclang and cmake** (`llama-cpp-sys-2` runs bindgen); first build ~5 min.
- **TLS is rustls+ring** — call `ensure_default_crypto_provider()` before constructing any reqwest client.
- **API stability lints**: exported structs/enums need `#[non_exhaustive]` unless deliberately frozen with an explicit allow plus a comment (`clippy::exhaustive_structs` / `exhaustive_enums`).
- **Specs**: `specs/NNN-name/` with `.specify/` templates; `specs/spec-status.md` is generated — read the relevant spec before large changes, and drop the file on merge conflicts.
- **Project skills** in `.claude/skills/` (`issue_pickup`, `prune`, `qa`, `speckit-status-report-show`) — prefer them over ad-hoc equivalents.
