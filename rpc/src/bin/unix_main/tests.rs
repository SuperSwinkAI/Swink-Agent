//! Tests for `unix_main` in `swink_agentd`.
#![cfg(test)]

use super::*;

// These tests pass explicit `Option` values instead of mutating the
// process environment, so they cannot race other tests (mirrors the
// TUI's `resolve_system_prompt` tests, spec 025 T044).

#[test]
fn cli_flag_wins_over_env_and_default() {
    let result = resolve(
        Some("from-cli".to_string()),
        Some("from-env".to_string()),
        DEFAULT_MODEL,
    );
    assert_eq!(result, "from-cli");
}

#[test]
fn env_var_wins_over_default() {
    let result = resolve(None, Some("from-env".to_string()), DEFAULT_MODEL);
    assert_eq!(result, "from-env");
}

#[test]
fn default_used_when_nothing_set() {
    assert_eq!(resolve(None, None, DEFAULT_MODEL), DEFAULT_MODEL);
    assert_eq!(
        resolve(None, None, DEFAULT_SYSTEM_PROMPT),
        DEFAULT_SYSTEM_PROMPT
    );
}

#[test]
fn explicit_empty_cli_flag_still_wins() {
    let result = resolve(
        Some(String::new()),
        Some("from-env".to_string()),
        DEFAULT_SYSTEM_PROMPT,
    );
    assert_eq!(result, "", "explicit empty string should still be used");
}

#[test]
fn shared_defaults_are_not_empty() {
    assert!(!DEFAULT_MODEL.is_empty());
    assert!(!DEFAULT_SYSTEM_PROMPT.is_empty());
}
