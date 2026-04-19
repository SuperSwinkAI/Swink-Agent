# Contributing to Swink Agent

This guide exists to save both sides time.

## The Standard

**You must understand your code.** If you cannot explain what your changes do and how they interact with the rest of the system, your PR will be closed.

Using AI to write code is fine. Submitting AI-generated code without understanding it is not.

If you use an agent, run it from the repo root so it picks up `AGENTS.md` automatically. Your agent must follow the rules in that file.

## Contribution Gate

PRs from new contributors are auto-closed by default. Issues are open to everyone.

Maintainers review auto-closed PRs and reopen worthwhile ones. Reply `lgtm` on any issue or PR from a contributor to grant them PR rights going forward.

## Quality Bar for Issues

Use one of the [GitHub issue templates](https://github.com/SuperSwinkAI/Swink-Agent/issues/new/choose).

Keep it short and concrete:

- One screen or less. If it does not fit, it is too long.
- Write in your own voice.
- State the bug or request clearly.
- Explain why it matters to users of this library.
- If you want to implement the fix yourself, say so.

## Blocking

If you spam the tracker with agent-generated issues or PRs, your GitHub account will be permanently blocked.

## Prerequisites

- **Rust 1.88+** (MSRV). Install via [rustup](https://rustup.rs).
- Add required toolchain components:
  ```bash
  rustup component add clippy rustfmt
  ```
- For TUI development, live adapter tests, or any work touching provider crates: copy `.env.example` to `.env` and populate the API keys for the providers you need. The `.env` file is auto-loaded by the TUI and by `cargo test` in the adapters crate.

## Development Setup

```bash
git clone https://github.com/SuperSwinkAI/Swink-Agent.git
cd Swink-Agent
cargo build --workspace
```

## Spec-Driven Development

This project uses spec-driven development via [GitHub Specify](https://github.com/apps/specify).

- **New features require a spec first.** Open an issue requesting a new spec, or propose one directly via the Specify app. Implementation PRs for unspecced features are closed.
- **Changes to existing features must update the spec.** If your PR changes behavior covered by a spec in `specs/`, that spec must be updated in the same PR.
- Specs live in `specs/NNN-<name>/` (spec.md, plan.md, tasks.md). Read the relevant spec before touching the code it covers.
- It is perfectly valid to open an issue requesting a new or revised spec without implementing it yourself.

## Before Submitting a PR

Three requirements before opening a PR:

1. **Open an issue first.** Discuss the change, get feedback, and confirm the direction before writing code. PRs without a linked issue are auto-closed.
2. **New features need an approved spec** (see Spec-Driven Development above).
3. **Get `lgtm` approval** (see Contribution Gate above).

Link your PR to its issue using `Closes #<issue>` or `Related to #<issue>` in the PR description.

Run this locally before opening:

```bash
cargo fmt --all --check                              # formatting
cargo clippy --workspace -- -D warnings              # zero warnings
cargo test --workspace                               # all workspace tests
cargo build --workspace                              # full workspace build
cargo test --workspace --features testkit            # testkit-enabled workspace tests
cargo test -p swink-agent --no-default-features      # builtin-tools disabled coverage
```

`just validate` and `just check` run the same canonical gate.

All six commands must pass. Do not edit `CHANGELOG.md` — entries are added by maintainers.

Some tests hit live APIs and are `#[ignore]`. Run them with:
```bash
cargo test -p swink-agent-adapters -- --ignored      # requires API keys in .env
```

## Pull Request Process

1. Open a PR against `main` with a clear title and description.
2. Describe *what* changed and *why* — not just what the diff shows.
3. Reference related issues with `Closes #<issue>` or `Related to #<issue>`.
4. All CI checks must pass before merge.
5. At least one maintainer review is required.

## Commit Messages

Concise, imperative-mood subject lines:
```
Add tool approval callback to agent loop
Fix context overflow sentinel not resetting after transform
Refactor streaming accumulator into separate module
```

No ticket numbers in commit messages — link issues in the PR description.

## Branch Model

This repo uses a two-branch model:

| Branch | Purpose |
|---|---|
| `main` | Stable releases only — every commit is a tagged crates.io publish |
| `integration` | Active development — all feature PRs target here |

**All PRs must target `integration`.** PRs against `main` from outside contributors are auto-closed.

Releases are cut by squash-merging `integration` into `main` and pushing a version tag. Release candidates (`v0.8.0-rc.1`) are tagged directly from `integration` for pre-release testing.

**Hotfixes** (critical bugs in a published release): branch off `main`, fix, squash-merge back to `main` + tag, then cherry-pick to `integration`.

## Branch Naming

| Type | Pattern | Example |
|---|---|---|
| Feature | `feature/<short-description>` | `feature/tool-approval-ui` |
| Bug fix | `fix/<short-description>` | `fix/overflow-retry-loop` |
| Refactor | `refactor/<short-description>` | `refactor/split-agent-context` |
| Docs | `docs/<short-description>` | `docs/streaming-guide` |
| Hotfix | `hotfix/<short-description>` | `hotfix/context-overflow-panic` |

Branch off `integration`. One concern per PR.

## Workspace Structure

| Crate | Path | Purpose |
|---|---|---|
| `swink-agent` | `.` | Core agentic loop, tool system, context management |
| `swink-agent-adapters` | `adapters/` | Provider adapters (Anthropic, OpenAI, Ollama, …) |
| `swink-agent-artifacts` | `artifacts/` | Versioned artifact storage |
| `swink-agent-auth` | `auth/` | Credential management and OAuth2 |
| `swink-agent-eval` | `eval/` | Trajectory tracing and evaluation framework |
| `swink-agent-local-llm` | `local-llm/` | On-device inference via llama.cpp |
| `swink-agent-macros` | `macros/` | `#[derive(ToolSchema)]` and `#[tool]` proc macros |
| `swink-agent-mcp` | `mcp/` | Model Context Protocol integration |
| `swink-agent-memory` | `memory/` | Session persistence and compaction |
| `swink-agent-patterns` | `patterns/` | Multi-agent pipeline patterns |
| `swink-agent-plugin-web` | `plugins/web/` | Web browsing and search plugin |
| `swink-agent-policies` | `policies/` | Feature-gated policy implementations |
| `swink-agent-tui` | `tui/` | Interactive terminal UI |
| `xtask` | `xtask/` | Developer task runner (`cargo xtask`) |

Each crate has an `AGENTS.md` with architecture notes, gotchas, and lessons learned. Read the relevant one before making changes in that crate.

## Code Style

See `AGENTS.md` for full conventions. Key rules:

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
