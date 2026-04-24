#!/usr/bin/env just

# Swink Agent — development task runner
# Usage: just <recipe>    (run `just --list` for all recipes)

set dotenv-load

# Run full workspace tests with nextest
test:
    cargo nextest run --workspace

# Run full workspace tests with testkit enabled
test-testkit:
    cargo test --workspace --features testkit

# Run core crate tests with no default features (verifies builtin-tools disabled)
test-no-features:
    cargo nextest run -p swink-agent --no-default-features

# Run clippy with zero-warnings policy
lint:
    cargo clippy --workspace -- -D warnings

# Format all workspace code
fmt:
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Build docs with warnings as errors
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

# Run benchmarks
bench:
    cargo bench --workspace

# Launch the TUI (.env auto-loaded via dotenv-load)
tui:
    cargo run -p swink-agent-tui

# Run publish-surface packaging checks for every publishable workspace crate
package-preflight:
    cargo publish --workspace --dry-run --locked --allow-dirty

# Run the canonical local validation gate required before opening a PR
validate:
    cargo fmt --all --check
    cargo clippy --workspace -- -D warnings
    cargo test --workspace
    cargo build --workspace
    cargo test --workspace --features testkit
    cargo test -p swink-agent --no-default-features
    just package-preflight

# Backward-compatible alias for the canonical local validation gate
check: validate

# Build the entire workspace
build:
    cargo build --workspace
