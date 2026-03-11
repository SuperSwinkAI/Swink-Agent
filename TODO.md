# TODO

## Quick Wins

- [x] **dotenvy support in TUI** — Added `dotenvy::dotenv().ok()` to `tui/src/main.rs`
- [x] **CI pipeline** — `.github/workflows/ci.yml`: clippy + test on push/PR
- [x] **Integration test for one adapter** — All three adapters have wiremock tests in `adapters/tests/`

## Medium Effort

- [x] **Fix panic auto-unsubscription** — `agent.rs` now removes panicking subscribers from the listener map after `catch_unwind` failure
- [x] **Adapter-level wiremock tests** — All three adapters (Anthropic, OpenAI, Ollama) have wiremock-based tests

## Later

- [x] **Thinking collapsibility in TUI** — Removed incorrect "collapsible" claim from docs; thinking sections are rendered dimmed (no toggle needed)
- [ ] **Structured output retry tuning** — Current retry logic works but needs real-world usage data to calibrate attempt counts and backoff
