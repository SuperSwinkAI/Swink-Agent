//! Tests for `agent_options`.
#![cfg(all(test, feature = "testkit"))]

use super::*;
use crate::testing::SimpleMockStreamFn;
use crate::types::ModelSpec;

fn test_options() -> AgentOptions {
    AgentOptions::new_simple(
        "test",
        ModelSpec::new("test-model", "test-model"),
        Arc::new(SimpleMockStreamFn::from_text("hello")),
    )
}

#[test]
fn credential_timeout_defaults_to_30_seconds() {
    let opts = test_options();
    assert_eq!(opts.credential_timeout, std::time::Duration::from_secs(30));
}

#[test]
fn with_credential_timeout_overrides_default() {
    let opts = test_options().with_credential_timeout(std::time::Duration::from_millis(250));
    assert_eq!(
        opts.credential_timeout,
        std::time::Duration::from_millis(250)
    );
}
