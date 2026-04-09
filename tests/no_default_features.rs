//! Regression test for issue #205: `--no-default-features` must not re-enable
//! `builtin-tools` or `transfer` through dev-dependency feature unification.
//!
//! This file compiles under `cargo test -p swink-agent --no-default-features`.
//! If a dev-dependency leaks those features back in, the `#[cfg]` assertions
//! below will fail at compile time, catching the regression immediately.

/// When `builtin-tools` is genuinely disabled, `BashTool` must not exist.
#[cfg(not(feature = "builtin-tools"))]
#[test]
fn builtin_tools_feature_is_off() {
    // If this file compiles, the feature is truly absent.
    // A leaking dev-dep would re-enable it, making this test vanish from the
    // `--no-default-features` run and the companion assertion below fire instead.
}

/// Safety net: if `builtin-tools` IS enabled during a `--no-default-features`
/// test run, something is leaking features.
#[cfg(feature = "builtin-tools")]
#[test]
fn builtin_tools_must_not_leak_into_no_default_features() {
    panic!(
        "builtin-tools feature is enabled but should not be. \
         A dev-dependency is likely leaking features via Cargo unification."
    );
}

#[cfg(not(feature = "transfer"))]
#[test]
fn transfer_feature_is_off() {}

#[cfg(feature = "transfer")]
#[test]
fn transfer_must_not_leak_into_no_default_features() {
    panic!(
        "transfer feature is enabled but should not be. \
         A dev-dependency is likely leaking features via Cargo unification."
    );
}
