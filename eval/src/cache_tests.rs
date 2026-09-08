//! Tests for `cache`.
#![cfg(test)]

use super::*;
use crate::types::{Attachment, EvalCase};

fn fp(id: &str) -> CacheFingerprint {
    CacheFingerprint {
        case_id: id.into(),
        system_prompt: "sp".into(),
        user_messages: vec!["hi".into()],
    }
}

/// Full `EvalCase` matching the `fp()` helper above, for tests that need
/// to go through `EvalCase::cache_fingerprint()` rather than constructing
/// a `CacheFingerprint` directly.
fn case(id: &str) -> EvalCase {
    EvalCase {
        id: id.into(),
        name: id.into(),
        description: None,
        system_prompt: "sp".into(),
        user_messages: vec!["hi".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

#[test]
fn cache_key_deterministic_and_context_sensitive() {
    let f = fp("c1");
    let empty = FingerprintContext::default();
    let a = CacheKey::from_fingerprint(&f, &empty);
    assert_eq!(a, CacheKey::from_fingerprint(&f, &empty));
    assert_eq!(a.as_hex().len(), 64);
    let b = CacheKey::from_fingerprint(
        &f,
        &FingerprintContext {
            initial_session: Some(serde_json::json!({"k": 1})),
            ..Default::default()
        },
    );
    assert_ne!(a, b);
}

/// FR-038: the cache key MUST be derived from exactly `case_id`,
/// `system_prompt`, `user_messages` (case-derived) plus `initial_session`,
/// tool-set hash, and agent model (context-derived). A change to any
/// *other* case field (budget, evaluators, attachments, expected
/// criteria, ...) must NOT change the cache key — those fields affect
/// scoring, not what the agent sees.
#[test]
fn cache_key_ignores_non_key_case_fields() {
    let mut left = case("c1");
    let mut right = case("c1");
    left.budget = Some(crate::types::BudgetConstraints {
        max_cost: Some(1.0),
        max_input: None,
        max_output: None,
        max_turns: None,
    });
    left.evaluators = vec!["trajectory".into()];
    left.attachments = vec![Attachment::Url("https://example.com/a.png".into())];
    right.budget = None;
    right.evaluators = vec![];
    right.attachments = vec![];

    let empty = FingerprintContext::default();
    let key_left = CacheKey::from_fingerprint(&left.cache_fingerprint(), &empty);
    let key_right = CacheKey::from_fingerprint(&right.cache_fingerprint(), &empty);
    assert_eq!(
        key_left, key_right,
        "non-key case fields must not affect the cache key"
    );
}

/// Complementary to the above: changing a field FR-038 *does* name
/// (`system_prompt`) must change the cache key.
#[test]
fn cache_key_changes_with_key_case_field() {
    let mut other = case("c1");
    other.system_prompt = "different system prompt".into();

    let empty = FingerprintContext::default();
    let key_a = CacheKey::from_fingerprint(&case("c1").cache_fingerprint(), &empty);
    let key_b = CacheKey::from_fingerprint(&other.cache_fingerprint(), &empty);
    assert_ne!(key_a, key_b);
}

#[test]
fn tool_set_hash_is_order_independent() {
    assert_eq!(
        tool_set_hash([("a", "{}"), ("b", "{}")]),
        tool_set_hash([("b", "{}"), ("a", "{}")])
    );
    assert_ne!(
        tool_set_hash([("a", "{}")]),
        tool_set_hash([("a", "{}"), ("b", "{}")])
    );
}

#[test]
fn validate_identifier_rejects_path_traversal() {
    assert!(validate_identifier("../evil").is_err());
    assert!(validate_identifier("a/b").is_err());
    assert!(validate_identifier("").is_err());
    assert!(validate_identifier("ok-id_1.0").is_ok());
}
