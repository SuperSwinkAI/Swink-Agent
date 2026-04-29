#!/usr/bin/env just

# Swink Agent — development task runner
# Usage: just <recipe>    (run `just --list` for all recipes)

cargo := if os_family() == "windows" { "powershell -NoProfile -ExecutionPolicy Bypass -File scripts/cargo-with-sccache.ps1" } else { "scripts/cargo-with-sccache.sh" }

# Run full workspace tests with nextest
test:
    {{cargo}} nextest run --workspace

# Run full workspace tests with testkit enabled
test-testkit:
    {{cargo}} test --workspace --features testkit

# Run core plugin tests with the feature combination required by all plugin regressions
test-plugins:
    {{cargo}} test -p swink-agent --features plugins,testkit --test plugin_integration --test plugin_registry

# Run core crate tests with no default features (verifies builtin-tools disabled)
test-no-features:
    {{cargo}} nextest run -p swink-agent --no-default-features

# Run feature-gate sentinels and no-default build contracts
no-default-sentinels:
    {{cargo}} test -p swink-agent --no-default-features
    {{cargo}} adapters-no-default-features
    {{cargo}} local-llm-no-default-features
    {{cargo}} workspace-no-default-features
    {{cargo}} eval-no-default-features
    {{cargo}} eval-advanced-no-default-features

# Run clippy with zero-warnings policy
lint:
    {{cargo}} clippy --workspace -- -D warnings

# Format all workspace code
fmt:
    {{cargo}} fmt --all

# Check formatting without modifying files
fmt-check:
    {{cargo}} fmt --all -- --check

# Build docs with warnings as errors
doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} doc --no-deps --workspace

# Run benchmarks
bench:
    {{cargo}} bench --workspace

# Launch the TUI (.env auto-loaded by the TUI process)
tui:
    {{cargo}} run -p swink-agent-tui

# Run publish-surface packaging checks for every publishable workspace crate
# Intentionally does not load .env; packaging should not inherit provider secrets.
package-preflight:
    {{cargo}} publish --workspace --dry-run --locked --allow-dirty

# Run the canonical local validation gate required before opening a PR
# Intentionally does not load .env; validation should not inherit provider secrets.
validate:
    {{cargo}} fmt --all --check
    {{cargo}} clippy --workspace -- -D warnings
    {{cargo}} test --workspace
    {{cargo}} build --workspace
    {{cargo}} test --workspace --features testkit
    just test-plugins
    just no-default-sentinels
    just package-preflight

# Backward-compatible alias for the canonical local validation gate
check: validate

# Build the entire workspace
build:
    {{cargo}} build --workspace
