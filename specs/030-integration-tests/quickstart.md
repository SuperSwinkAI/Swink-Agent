# Quickstart: Integration Tests

**Branch**: `030-integration-tests` | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- All workspace crates build: `cargo build --workspace`
- No external services, API keys, or network access required

## Running the Tests

### Full workspace test suite (includes integration tests)

```bash
cargo test --workspace
```

### Run only integration tests in the core crate

```bash
cargo test --test ac_lifecycle --test ac_tools --test ac_context --test ac_resilience --test ac_structured --test ac_tui
```

### Run a specific test file

```bash
cargo test --test ac_lifecycle
```

### Run a specific test by name

```bash
cargo test --test ac_tools concurrent_tool_execution
```

### Run with output visible

```bash
cargo test --test ac_lifecycle -- --nocapture
```

## Project Layout

```
tests/
├── common/
│   └── mod.rs           # Shared mocks and helpers (MockStreamFn, MockTool, etc.)
├── integration.rs       # Existing integration tests
├── ac_lifecycle.rs      # AC 1–5:  Agent lifecycle and events
├── ac_tools.rs          # AC 6–12: Tool execution and validation
├── ac_context.rs        # AC 13–16: Context management and overflow
├── ac_resilience.rs     # AC 17–22: Retry, steering, abort
├── ac_structured.rs     # AC 23–25: Structured output, proxy reconstruction
└── ac_tui.rs            # AC 26–30: TUI rendering and interaction
```

## Writing a New Test

1. Identify which acceptance criterion the test covers.
2. Open the corresponding `ac_*.rs` file.
3. Add `mod common;` if not already present (all files need it).
4. Use shared helpers:

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

    let agent = Agent::new(
        default_model(),
        stream,
        default_convert,
        AgentOptions::default(),
    );

    let response = agent.message(user_msg("hi")).await.unwrap();
    // assert on response...
}
```

## Verifying Coverage

Each AC maps to a test function documented in `data-model.md` (Acceptance Criterion Mapping table). To verify all 30 ACs are covered:

```bash
cargo test --workspace 2>&1 | grep -c "test .* ok"
```

## CI Integration

Tests run as part of the standard `cargo test --workspace` command. No special CI configuration is needed. All tests are hermetic (no network, no filesystem side effects, no API keys).
