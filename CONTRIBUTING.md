# Contributing to Swink Agent

Thank you for your interest in contributing. This document covers everything you need to get started.

## Prerequisites

- **Rust 1.88+** (MSRV). Install via [rustup](https://rustup.rs).
- After installing Rust, add the required toolchain components:
  ```bash
  rustup component add clippy rustfmt
  ```
- For TUI development: copy `.env.example` to `.env` and populate your API keys.

## Development Setup

```bash
git clone https://github.com/SuperSwinkAI/Swink-Agent.git
cd Swink-Agent
cargo build --workspace
```

For the TUI, create a `.env` file at the repo root (see `.env.example` for all supported keys).

## Branch Naming

| Type | Pattern | Example |
|---|---|---|
| Feature | `feature/<short-description>` | `feature/tool-approval-ui` |
| Bug fix | `fix/<short-description>` | `fix/overflow-retry-loop` |
| Refactor | `refactor/<short-description>` | `refactor/split-agent-context` |
| Docs | `docs/<short-description>` | `docs/streaming-guide` |

Branch off of `main`. Keep branches focused — one concern per PR.

## Before You Submit

Run this checklist locally before opening a PR:

```bash
cargo fmt --check                                    # formatting
cargo clippy --workspace -- -D warnings              # zero warnings
cargo test --workspace                               # all tests
cargo test -p swink-agent --no-default-features      # feature gate coverage
```

Some tests hit live APIs and are marked `#[ignore]`. Run them with:
```bash
cargo test -p swink-agent-adapters -- --ignored      # requires API keys in .env
```

## Pull Request Process

1. Open a PR against `main` with a clear title and description.
2. Describe *what* changed and *why* — not just what the diff shows.
3. Reference any related issues with `Closes #<issue>` or `Related to #<issue>`.
4. All CI checks must pass before merge.
5. At least one maintainer review is required.

## Commit Messages

Use concise, imperative-mood subject lines:
```
Add tool approval callback to agent loop
Fix context overflow sentinel not resetting after transform
Refactor streaming accumulator into separate module
```

No ticket numbers required in commit messages (link in the PR description instead).

## Workspace Structure

| Crate | Path | Purpose |
|---|---|---|
| `swink-agent` | `.` | Core agentic loop, tool system, context management |
| `swink-agent-adapters` | `adapters/` | Provider adapters (Anthropic, OpenAI, Ollama, …) |
| `swink-agent-memory` | `memory/` | Session persistence and compaction |
| `swink-agent-eval` | `eval/` | Trajectory tracing and evaluation framework |
| `swink-agent-local-llm` | `local-llm/` | On-device inference via mistral.rs |
| `swink-agent-tui` | `tui/` | Interactive terminal UI |

Each crate has a `CLAUDE.md` with architecture notes, gotchas, and lessons learned. Read the relevant one before making changes in that crate.

## Code Style

See `CLAUDE.md` for full conventions. Key rules:

- No `unsafe` code — enforced by `#[forbid(unsafe_code)]` at every crate root.
- `new()` primary constructor; `with_*()` builder chain.
- No `get_` prefix on getters. `is_*`/`has_*` for predicates.
- Test names: descriptive `snake_case` without a `test_` prefix.
- Bug found → write a failing regression test first, then fix.

## Reporting Issues

Use [GitHub Issues](https://github.com/SuperSwinkAI/Swink-Agent/issues). Include:
- What you expected to happen
- What actually happened
- A minimal reproduction (code snippet or test case)
- Rust version (`rustc --version`) and OS
