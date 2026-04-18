#![cfg(feature = "plugins")]

//! Integration tests for PluginRegistry and NamespacedTool.

use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentTool, NamespacedTool, PluginRegistry};

mod common;
use common::{MockPlugin, MockTool};

// ─── PluginRegistry CRUD tests (T005) ──────────────────────────────────────

#[test]
fn registry_register_and_lookup() {
    let mut reg = PluginRegistry::new();
    assert!(reg.is_empty());
    assert_eq!(reg.len(), 0);

    reg.register(Arc::new(MockPlugin::new("alpha")));
    reg.register(Arc::new(MockPlugin::new("beta")));

    assert_eq!(reg.len(), 2);
    assert!(!reg.is_empty());
    assert!(reg.get("alpha").is_some());
    assert!(reg.get("beta").is_some());
    assert!(reg.get("gamma").is_none());
}

#[test]
fn registry_duplicate_replaces_and_preserves_count() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("alpha").with_priority(1)));
    reg.register(Arc::new(MockPlugin::new("alpha").with_priority(99)));

    assert_eq!(reg.len(), 1);
    assert_eq!(reg.get("alpha").unwrap().priority(), 99);
}

#[test]
fn registry_unregister_removes_plugin() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("alpha")));
    reg.register(Arc::new(MockPlugin::new("beta")));

    reg.unregister("alpha");
    assert_eq!(reg.len(), 1);
    assert!(reg.get("alpha").is_none());
    assert!(reg.get("beta").is_some());
}

#[test]
fn registry_unregister_nonexistent_is_noop() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("alpha")));
    reg.unregister("nonexistent");
    assert_eq!(reg.len(), 1);
}

#[test]
fn registry_list_sorted_by_priority_descending() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("low").with_priority(1)));
    reg.register(Arc::new(MockPlugin::new("high").with_priority(10)));
    reg.register(Arc::new(MockPlugin::new("mid").with_priority(5)));

    let names: Vec<&str> = reg.list().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["high", "mid", "low"]);
}

#[test]
fn registry_list_stable_sort_on_equal_priority() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("first")));
    reg.register(Arc::new(MockPlugin::new("second")));
    reg.register(Arc::new(MockPlugin::new("third")));

    let names: Vec<&str> = reg.list().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["first", "second", "third"]);
}

// ─── NamespacedTool tests (T007) ───────────────────────────────────────────

#[test]
fn namespaced_tool_prefixes_name() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner);

    assert_eq!(namespaced.name(), "artifacts_save");
}

#[test]
fn namespaced_tool_delegates_description() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner.clone());

    assert_eq!(namespaced.description(), inner.description());
    assert_eq!(namespaced.label(), inner.label());
}

#[test]
fn namespaced_tool_delegates_schema() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner.clone());

    assert_eq!(namespaced.parameters_schema(), inner.parameters_schema());
}

#[test]
fn namespaced_tool_metadata_has_plugin_namespace() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner);

    let meta = namespaced.metadata().expect("should have metadata");
    assert_eq!(meta.namespace.as_deref(), Some("artifacts"));
}

#[test]
fn namespaced_tool_delegates_requires_approval() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner.clone());

    assert_eq!(namespaced.requires_approval(), inner.requires_approval());
}

#[tokio::test]
async fn namespaced_tool_delegates_execute() {
    let inner = Arc::new(MockTool::new("save")) as Arc<dyn AgentTool>;
    let namespaced = NamespacedTool::new("artifacts", inner);

    let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new()));
    let result = namespaced
        .execute(
            "call-1",
            json!({}),
            CancellationToken::new(),
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error);
}

#[test]
fn two_namespaced_tools_from_different_plugins_are_distinct() {
    let tool = Arc::new(MockTool::new("run")) as Arc<dyn AgentTool>;
    let ns1 = NamespacedTool::new("plugin_a", tool.clone());
    let ns2 = NamespacedTool::new("plugin_b", tool);

    assert_eq!(ns1.name(), "plugin_a_run");
    assert_eq!(ns2.name(), "plugin_b_run");
    assert_ne!(ns1.name(), ns2.name());
}
