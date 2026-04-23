//! Helpers for composing provider-safe tool names.

/// Maximum length for a composed tool name (the tightest cap across providers:
/// `OpenAI`, Bedrock, and Gemini all cap at 64; Anthropic allows 128).
pub const MAX_TOOL_NAME_LEN: usize = 64;
/// Hex characters preserved from the stable hash when truncation is required.
pub const TOOL_NAME_HASH_HEX_LEN: usize = 16;

/// Compose a tool name that is safe across every supported provider.
///
/// `namespace` is optional. When present, the namespace and tool name are
/// joined with `_` after sanitization. When absent, the sanitized tool name is
/// used directly. The final name always starts with an ASCII letter, contains
/// only ASCII alphanumerics / `_`, and is bounded to the strictest provider
/// length limit.
#[must_use]
pub fn compose_provider_safe_tool_name(namespace: Option<&str>, tool_name: &str) -> String {
    let sanitized_tool = sanitize_tool_name_component(tool_name);
    let joined = match namespace {
        Some(namespace) => {
            let sanitized_namespace = sanitize_tool_name_component(namespace);
            format!("{sanitized_namespace}_{sanitized_tool}")
        }
        None => sanitized_tool,
    };
    let with_leading_letter = match joined.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => joined,
        _ => format!("t_{joined}"),
    };

    if with_leading_letter.len() <= MAX_TOOL_NAME_LEN {
        with_leading_letter
    } else {
        let hash_suffix = stable_name_hash_hex(&with_leading_letter);
        let prefix_len = MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1;
        let prefix: String = with_leading_letter.chars().take(prefix_len).collect();
        format!("{prefix}_{hash_suffix}")
    }
}

/// Derive a deterministic provider-safe fallback name for a colliding tool.
///
/// `base_name` must already satisfy the provider-safe grammar. The returned
/// name preserves that grammar while appending a stable hash derived from the
/// raw pre-sanitized identity so distinct inputs that collapse onto the same
/// sanitized wire name can still coexist.
#[must_use]
pub fn disambiguate_provider_safe_tool_name(base_name: &str, raw_identity: &str) -> String {
    let hash_suffix = stable_name_hash_hex(raw_identity);
    let prefix_len = MAX_TOOL_NAME_LEN - TOOL_NAME_HASH_HEX_LEN - 1;
    let prefix: String = base_name.chars().take(prefix_len).collect();
    format!("{prefix}_{hash_suffix}")
}

fn sanitize_tool_name_component(input: &str) -> String {
    if input.is_empty() {
        return "_".to_owned();
    }

    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn stable_name_hash_hex(input: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:0TOOL_NAME_HASH_HEX_LEN$x}")
}

#[cfg(test)]
mod tests {
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
        let second =
            compose_provider_safe_tool_name(Some(&prefix), &format!("{}y", "b".repeat(40)));

        assert_ne!(first, second);
        assert_eq!(first.len(), MAX_TOOL_NAME_LEN);
        assert_eq!(second.len(), MAX_TOOL_NAME_LEN);
    }

    #[test]
    fn disambiguated_provider_safe_name_preserves_grammar_and_length() {
        let base = compose_provider_safe_tool_name(Some("my-web"), "search");
        let disambiguated =
            disambiguate_provider_safe_tool_name(&base, "my-web\0search\0my.web\0search");

        assert_eq!(disambiguated.len(), MAX_TOOL_NAME_LEN);
        assert!(disambiguated.starts_with("my_web_search"));
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
}
