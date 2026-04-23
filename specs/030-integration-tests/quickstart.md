# Quickstart: Integration Tests

**Branch**: `030-integration-tests` | **Date**: 2026-03-20

## Prerequisites

- Rust latest stable (edition 2024)
- All workspace crates build: `cargo build --workspace`
- No external services, API keys, or network access required

## Running the Tests

### Full workspace test suite

The root crate's `tests/` integration suite is gated behind the `testkit`
feature. Use this command when you want the full workspace plus the core
integration tests:

```bash
cargo test --workspace --features testkit
```

### Run the core-crate integration tests

```bash
cargo test -p swink-agent --features testkit --test ac_lifecycle --test ac_tools --test ac_context --test ac_resilience --test ac_structured
```

### Run the TUI integration tests

```bash
cargo test -p swink-agent-tui --test ac_tui
```

### Run a specific test file

```bash
cargo test -p swink-agent --test ac_lifecycle
```

### Run a specific test by name

```bash
cargo test -p swink-agent --test ac_tools concurrent_tool_execution
```

### Run with output visible

```bash
cargo test -p swink-agent --test ac_lifecycle -- --nocapture
```

## Project Layout

```text
tests/
├── common/
│   └── mod.rs           # Shared mocks and helpers (MockStreamFn, MockTool, etc.)
├── integration.rs       # Existing integration tests
├── ac_lifecycle.rs      # AC 1-5: Agent lifecycle and events
├── ac_tools.rs          # AC 6-12: Tool execution and validation
├── ac_context.rs        # AC 13-16: Context management and overflow
├── ac_resilience.rs     # AC 17-22: Retry, steering, abort
└── ac_structured.rs     # AC 23-25: Structured output, proxy reconstruction

tui/tests/
├── ac_tui.rs            # Public TUI wiring/state coverage for AC 26-30
└── public_api.rs        # TUI public API integration coverage
```

## Writing a New Test

1. Identify which acceptance criterion the test covers.
2. Open the corresponding `ac_*.rs` file in `tests/` or `tui/tests/`.
3. For core integration tests under `tests/`, add `mod common;` if the file uses the shared helpers.
4. Use shared helpers in the core test suite when needed:

```rust
mod common;

use common::{MockStreamFn, MockTool, default_model, default_convert, text_only_events, user_msg};
use swink_agent::{Agent, AgentOptions};
use std::sync::Arc;

#[tokio::test]
async fn my_new_acceptance_test() {
    let stream = Arc::new(MockStreamFn::new(vec![
        text_only_events("hello"),
    ]));

    let mut agent = Agent::new(AgentOptions::new(
        "test prompt",
        default_model(),
        stream,
        default_convert,
    ));

    let response = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();
    // assert on response...
}
```

## Verifying Coverage

Each AC maps to an automated check documented in `data-model.md` (Acceptance Criterion Mapping table). For the TUI story, `tui/tests/ac_tui.rs` covers the public integration surface and crate-local unit tests cover private renderer details.

To verify the documented commands still run:

```bash
cargo test --workspace --features testkit
```

## CI Integration

Core integration tests require `--features testkit`; crate-local integration tests such as `tui/tests/ac_tui.rs` run with their own crate defaults. All documented tests are hermetic (no network, no filesystem side effects, no API keys).
