//! Tests for `tool_name`.
#![cfg(test)]

use super::*;

#[test]
fn namespaced_name_replaces_dashes_and_dots() {
    assert_eq!(
        compose_provider_safe_tool_name(Some("my-web"), "read.file"),
        "my_web_read_file"
    );
    assert_eq!(
        compose_provider_safe_tool_name(Some("my.ns"), "x.y.z"),
        "my_ns_x_y_z"
    );
}

#[test]
fn namespaced_name_prepends_letter_when_leading_non_alpha() {
    assert_eq!(
        compose_provider_safe_tool_name(Some("1plugin"), "foo"),
        "t_1plugin_foo"
    );
    assert_eq!(
        compose_provider_safe_tool_name(Some("_plugin"), "foo"),
        "t__plugin_foo"
    );
}

#[test]
fn bare_tool_name_is_sanitized_without_namespace_separators() {
    assert_eq!(compose_provider_safe_tool_name(None, "echo"), "echo");
    assert_eq!(
        compose_provider_safe_tool_name(None, "read.file"),
        "read_file"
    );
    assert_eq!(
        compose_provider_safe_tool_name(None, "1invalid"),
        "t_1invalid"
    );
}

#[test]
fn provider_safe_name_truncates_to_max_length() {
    let result = compose_provider_safe_tool_name(Some(&"a".repeat(40)), &"b".repeat(40));
    assert_eq!(result.len(), MAX_TOOL_NAME_LEN);
    assert_eq!(
        result.rsplit_once('_').expect("hash suffix").1.len(),
        TOOL_NAME_HASH_HEX_LEN
    );
}

#[test]
fn provider_safe_name_long_collisions_get_distinct_hash_suffixes() {
    let prefix = "a".repeat(40);
    let first = compose_provider_safe_tool_name(Some(&prefix), &format!("{}x", "b".repeat(40)));
    let second = compose_provider_safe_tool_name(Some(&prefix), &format!("{}y", "b".repeat(40)));

    assert_ne!(first, second);
    assert_eq!(first.len(), MAX_TOOL_NAME_LEN);
    assert_eq!(second.len(), MAX_TOOL_NAME_LEN);
}

#[cfg(feature = "plugins")]
#[test]
fn disambiguated_provider_safe_name_preserves_grammar_and_length() {
    let base = compose_provider_safe_tool_name(Some("my-web"), "search");
    let disambiguated =
        disambiguate_provider_safe_tool_name(&base, "my-web\0search\0my.web\0search");

    assert!(disambiguated.len() <= MAX_TOOL_NAME_LEN);
    assert!(disambiguated.starts_with("my_web_search"));
    assert!(disambiguated.ends_with(&format!(
        "_{}",
        stable_name_hash_hex("my-web\0search\0my.web\0search")
    )));
    assert!(
        disambiguated
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    );
    assert!(
        disambiguated
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
}

#[test]
fn provider_safe_name_satisfies_strictest_grammar() {
    let is_valid = |s: &str| {
        s.len() <= MAX_TOOL_NAME_LEN
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };

    for (namespace, tool) in [
        (Some("web"), "search"),
        (Some("my-web"), "search"),
        (Some("web"), "read.file"),
        (Some("1plugin"), "foo"),
        (None, "1invalid"),
        (None, "naïve"),
        (None, ""),
        (Some(&"a".repeat(100)), &"b".repeat(100)),
    ] {
        let name = compose_provider_safe_tool_name(namespace, tool);
        assert!(
            is_valid(&name),
            "composed name {name:?} (from {namespace:?} + {tool:?}) violates the strictest grammar"
        );
    }
}
