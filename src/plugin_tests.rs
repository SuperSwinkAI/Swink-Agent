//! Tests for `plugin`.
#![cfg(test)]

use super::*;

use crate::testing::MockPlugin;

// ─── PluginRegistry tests ───────────────────────────────────────────

#[test]
fn registry_register_and_get() {
    let mut reg = PluginRegistry::new();
    assert!(reg.is_empty());
    assert_eq!(reg.len(), 0);

    reg.register(Arc::new(MockPlugin::new("alpha")));
    assert!(!reg.is_empty());
    assert_eq!(reg.len(), 1);
    assert!(reg.get("alpha").is_some());
    assert!(reg.get("beta").is_none());
}

#[test]
fn registry_duplicate_replaces() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("alpha").with_priority(1)));
    reg.register(Arc::new(MockPlugin::new("alpha").with_priority(5)));
    assert_eq!(reg.len(), 1);
    assert_eq!(reg.get("alpha").unwrap().priority(), 5);
}

#[test]
fn registry_unregister() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("alpha")));
    reg.register(Arc::new(MockPlugin::new("beta")));
    assert_eq!(reg.len(), 2);

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
fn registry_list_sorted_by_priority_desc() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("low").with_priority(1)));
    reg.register(Arc::new(MockPlugin::new("high").with_priority(10)));
    reg.register(Arc::new(MockPlugin::new("mid").with_priority(5)));

    let list = reg.list();
    let names: Vec<&str> = list.iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["high", "mid", "low"]);
}

// ─── Tool-name sanitization tests ───────────────────────────────────

#[test]
fn compose_namespaced_name_dot_becomes_underscore() {
    assert_eq!(compose_namespaced_name("web", "search"), "web_search");
    assert_eq!(compose_namespaced_name("web", "fetch"), "web_fetch");
}

#[test]
fn compose_namespaced_name_replaces_dashes_and_dots() {
    assert_eq!(compose_namespaced_name("my-web", "search"), "my_web_search");
    assert_eq!(compose_namespaced_name("web", "read.file"), "web_read_file");
    assert_eq!(compose_namespaced_name("my.ns", "x.y.z"), "my_ns_x_y_z");
}

#[test]
fn compose_namespaced_name_prepends_letter_when_leading_non_alpha() {
    // Plugin starting with a digit would otherwise produce "1plugin_foo" —
    // Bedrock requires a leading letter, so we prepend "t_".
    assert_eq!(compose_namespaced_name("1plugin", "foo"), "t_1plugin_foo");
    // Same for a leading underscore (valid for Gemini, rejected by Bedrock).
    assert_eq!(compose_namespaced_name("_plugin", "foo"), "t__plugin_foo");
}

#[test]
fn compose_namespaced_name_replaces_non_ascii() {
    assert_eq!(compose_namespaced_name("plugin", "naïve"), "plugin_na_ve");
}

#[test]
fn compose_namespaced_name_truncates_to_max_length() {
    let long_plugin = "a".repeat(40);
    let long_tool = "b".repeat(40);
    let result = compose_namespaced_name(&long_plugin, &long_tool);
    assert_eq!(result.len(), MAX_TOOL_NAME_LEN);
    // Prefix is preserved (plugin name survives at the front).
    assert!(result.starts_with(&long_plugin));
    assert_eq!(result.chars().filter(|c| *c == '_').count(), 2);
    assert_eq!(
        result.rsplit_once('_').unwrap().1.len(),
        TOOL_NAME_HASH_HEX_LEN
    );
}

#[test]
fn compose_namespaced_name_long_collisions_get_distinct_hash_suffixes() {
    let long_plugin = "a".repeat(40);
    let first_tool = format!("{}x", "b".repeat(40));
    let second_tool = format!("{}y", "b".repeat(40));

    let first = compose_namespaced_name(&long_plugin, &first_tool);
    let second = compose_namespaced_name(&long_plugin, &second_tool);

    assert_eq!(first.len(), MAX_TOOL_NAME_LEN);
    assert_eq!(second.len(), MAX_TOOL_NAME_LEN);
    assert_ne!(first, second);
    assert_eq!(
        first[..MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1],
        second[..MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1]
    );
    assert_ne!(
        first.rsplit_once('_').unwrap().1,
        second.rsplit_once('_').unwrap().1
    );
}

#[test]
fn compose_namespaced_name_empty_components() {
    // Empty plugin name collapses to "_", then the leading-letter rule kicks in.
    assert_eq!(compose_namespaced_name("", "foo"), "t___foo");
    assert_eq!(compose_namespaced_name("foo", ""), "foo__");
    // "" → "_", "" → "_", joined "___" (3 underscores), leading non-alpha → prepend "t_".
    assert_eq!(compose_namespaced_name("", ""), "t____");
}

#[test]
fn compose_namespaced_name_satisfies_strictest_grammar() {
    // Regex equivalent to Bedrock's ^[a-zA-Z][a-zA-Z0-9_]*$ (the strictest
    // provider pattern). Every output from this function must match.
    let is_valid = |s: &str| {
        s.len() <= MAX_TOOL_NAME_LEN
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    for (plugin, tool) in [
        ("web", "search"),
        ("my-web", "search"),
        ("web", "read.file"),
        ("1plugin", "foo"),
        ("_plugin", "foo"),
        ("plugin", "naïve"),
        ("", ""),
        (&"a".repeat(100), &"b".repeat(100)),
    ] {
        let name = compose_namespaced_name(plugin, tool);
        assert!(
            is_valid(&name),
            "composed name {name:?} (from {plugin:?} + {tool:?}) violates the strictest grammar"
        );
    }
}

#[test]
fn namespaced_tool_preserves_unsanitized_plugin_name_in_metadata() {
    use crate::testing::MockTool;
    let tool: Arc<dyn AgentTool> = Arc::new(MockTool::new("search"));
    let wrapped = NamespacedTool::new("my-web", tool);
    assert_eq!(wrapped.name(), "my_web_search");
    // Metadata namespace keeps the original plugin name for introspection.
    let meta = wrapped.metadata().expect("metadata present");
    assert_eq!(meta.namespace.as_deref(), Some("my-web"));
}

#[test]
fn registry_list_stable_sort_for_equal_priority() {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(MockPlugin::new("first").with_priority(0)));
    reg.register(Arc::new(MockPlugin::new("second").with_priority(0)));
    reg.register(Arc::new(MockPlugin::new("third").with_priority(0)));

    let list = reg.list();
    let names: Vec<&str> = list.iter().map(|p| p.name()).collect();
    assert_eq!(names, vec!["first", "second", "third"]);
}
