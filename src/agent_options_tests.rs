//! Tests for `agent_options`.
#![cfg(all(test, feature = "plugins"))]

use super::*;
use crate::testing::{MockPlugin, SimpleMockStreamFn};
use crate::types::ModelSpec;

fn test_options() -> AgentOptions {
    AgentOptions::new_simple(
        "test",
        ModelSpec::new("test-model", "test-model"),
        Arc::new(SimpleMockStreamFn::from_text("hello")),
    )
}

#[test]
fn with_plugin_deduplicates_by_name() {
    let opts = test_options()
        .with_plugin(Arc::new(MockPlugin::new("alpha").with_priority(1)))
        .with_plugin(Arc::new(MockPlugin::new("alpha").with_priority(5)));

    assert_eq!(opts.plugins.len(), 1);
    assert_eq!(opts.plugins[0].priority(), 5);
}

#[test]
fn with_plugin_keeps_distinct_names() {
    let opts = test_options()
        .with_plugin(Arc::new(MockPlugin::new("alpha")))
        .with_plugin(Arc::new(MockPlugin::new("beta")));

    assert_eq!(opts.plugins.len(), 2);
}

#[test]
fn with_plugins_deduplicates_within_batch() {
    let opts = test_options().with_plugins(vec![
        Arc::new(MockPlugin::new("alpha").with_priority(1)),
        Arc::new(MockPlugin::new("beta")),
        Arc::new(MockPlugin::new("alpha").with_priority(9)),
    ]);

    assert_eq!(opts.plugins.len(), 2);
    // Last "alpha" wins
    let alpha = opts.plugins.iter().find(|p| p.name() == "alpha").unwrap();
    assert_eq!(alpha.priority(), 9);
}

#[test]
fn with_plugins_deduplicates_against_existing() {
    let opts = test_options()
        .with_plugin(Arc::new(MockPlugin::new("alpha").with_priority(1)))
        .with_plugins(vec![
            Arc::new(MockPlugin::new("alpha").with_priority(7)),
            Arc::new(MockPlugin::new("beta")),
        ]);

    assert_eq!(opts.plugins.len(), 2);
    let alpha = opts.plugins.iter().find(|p| p.name() == "alpha").unwrap();
    assert_eq!(alpha.priority(), 7);
}
