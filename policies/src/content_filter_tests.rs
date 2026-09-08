//! Tests for `content_filter`.
#![cfg(test)]

use swink_agent::{AssistantMessage, Cost, StopReason, Usage};

use super::*;

fn make_policy_ctx<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(0, usage, cost, 0, false, &[], state)
}

fn make_turn_ctx(msg: &AssistantMessage) -> TurnPolicyContext<'_> {
    static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
        std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
    TurnPolicyContext::new(msg, &[], StopReason::Stop, "", &MODEL, &[])
}

fn make_msg(text: &str) -> AssistantMessage {
    AssistantMessage::new(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        String::new(),
        String::new(),
    )
    .with_timestamp(0)
}

#[test]
fn blocks_keyword() {
    let filter = ContentFilter::new().with_keyword("secret-project");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("The secret-project is underway.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Stop(reason) if reason.contains("secret-project")));
}

#[test]
fn case_insensitive_match() {
    let filter = ContentFilter::new().with_keyword("secret");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("This is a SECRET document.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Stop(_)));
}

#[test]
fn whole_word_no_substring_match() {
    let filter = ContentFilter::new()
        .with_whole_word(true)
        .with_keyword("ass");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("The assembly line is running.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Continue));
}

#[test]
fn regex_pattern_blocks() {
    let filter = ContentFilter::new()
        .with_regex(r"(?i)internal\s+use\s+only")
        .expect("valid regex");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("This document is for Internal Use Only.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Stop(_)));
}

#[test]
fn category_filtering_active() {
    let filter = ContentFilter::new()
        .with_enabled_categories(["compliance"])
        .with_category_keyword("profanity", "badword");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("This contains a badword.");
    let turn = make_turn_ctx(&msg);

    // "profanity" category is not in enabled set, so rule is skipped.
    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Continue));
}

#[test]
fn category_filtering_inactive_passes() {
    let filter = ContentFilter::new()
        .with_enabled_categories(["compliance"])
        .with_category_keyword("compliance", "restricted");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("This is restricted information.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Stop(reason) if reason.contains("restricted")));
}

#[test]
fn empty_filter_allows_all() {
    let filter = ContentFilter::new();
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("Anything goes here.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Continue));
}

#[test]
fn invalid_regex_returns_error() {
    let result = ContentFilter::new().with_regex("[invalid");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ContentFilterError::InvalidRegex { .. }));
}

#[test]
fn filter_rule_new_compiles_standalone_rule() {
    let rule = FilterRule::new(r"(?i)top\s+secret").expect("valid regex");
    assert!(rule.pattern.is_match("This is Top Secret material."));
    assert_eq!(rule.display_name, r"(?i)top\s+secret");
    assert!(rule.category.is_none());
}

#[test]
fn filter_rule_new_rejects_invalid_regex() {
    let err = FilterRule::new("[invalid").unwrap_err();
    assert!(matches!(err, ContentFilterError::InvalidRegex { .. }));
}

#[test]
fn no_match_returns_continue() {
    let filter = ContentFilter::new()
        .with_keyword("forbidden")
        .with_regex(r"(?i)classified")
        .expect("valid regex");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let msg = make_msg("This is a perfectly normal message.");
    let turn = make_turn_ctx(&msg);

    let verdict = filter.evaluate(&ctx, &turn);
    assert!(matches!(verdict, PolicyVerdict::Continue));
}
