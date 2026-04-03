#![cfg(feature = "testkit")]
use std::sync::Arc;

use swink_agent::{Agent, AgentOptions, AgentRegistry};

mod common;
use common::*;

fn make_agent() -> Agent {
    let options = AgentOptions::new_simple(
        "system prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")])),
    );
    Agent::new(options)
}

#[test]
fn register_and_get() {
    let registry = AgentRegistry::new();
    let agent = make_agent();
    let agent_ref = registry.register("alpha", agent);

    let fetched = registry.get("alpha").expect("agent should be registered");
    assert!(Arc::ptr_eq(&agent_ref, &fetched));
}

#[test]
fn get_unknown_returns_none() {
    let registry = AgentRegistry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn remove_agent() {
    let registry = AgentRegistry::new();
    registry.register("alpha", make_agent());

    let removed = registry.remove("alpha");
    assert!(removed.is_some());
    assert!(registry.get("alpha").is_none());
}

#[test]
fn duplicate_name_overwrites() {
    let registry = AgentRegistry::new();
    let first = registry.register("alpha", make_agent());
    let second = registry.register("alpha", make_agent());

    assert!(!Arc::ptr_eq(&first, &second));

    let fetched = registry.get("alpha").unwrap();
    assert!(Arc::ptr_eq(&second, &fetched));
}

#[test]
fn names_lists_all() {
    let registry = AgentRegistry::new();
    registry.register("alpha", make_agent());
    registry.register("beta", make_agent());
    registry.register("gamma", make_agent());

    let mut names = registry.names();
    names.sort();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn len_and_is_empty() {
    let registry = AgentRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    registry.register("alpha", make_agent());
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);

    registry.register("beta", make_agent());
    assert_eq!(registry.len(), 2);

    registry.remove("alpha");
    assert_eq!(registry.len(), 1);
}

#[test]
fn registry_is_clone() {
    let registry = AgentRegistry::new();
    registry.register("alpha", make_agent());

    let cloned = registry.clone();
    let from_original = registry.get("alpha").unwrap();
    let from_clone = cloned.get("alpha").unwrap();
    assert!(Arc::ptr_eq(&from_original, &from_clone));

    // Mutation through clone is visible in original.
    cloned.register("beta", make_agent());
    assert!(registry.get("beta").is_some());
}
