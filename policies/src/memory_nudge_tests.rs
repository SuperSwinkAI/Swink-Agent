//! Tests for `memory_nudge`.
#![cfg(test)]

use super::*;

use swink_agent::{
    AssistantMessage, ContentBlock, Cost, LlmMessage, PolicyContext, PolicyVerdict, StopReason,
    ToolResultMessage, TurnPolicyContext, Usage, UserMessage,
};

// ── Test helpers ─────────────────────────────────────────────────────

fn make_assistant(text: &str) -> AssistantMessage {
    AssistantMessage::new(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        String::new(),
        String::new(),
    )
    .with_timestamp(0)
}

fn make_turn_ctx<'a>(
    assistant: &'a AssistantMessage,
    tool_results: &'a [ToolResultMessage],
    context_messages: &'a [AgentMessage],
) -> TurnPolicyContext<'a> {
    static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
        std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
    TurnPolicyContext::new(
        assistant,
        tool_results,
        StopReason::Stop,
        "",
        &MODEL,
        context_messages,
    )
}

fn make_policy_ctx<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(3, usage, cost, 10, false, &[], state)
}

fn evaluate_text(text: &str, sensitivity: NudgeSensitivity) -> PolicyVerdict {
    let policy = MemoryNudgePolicy::new().with_sensitivity(sensitivity);
    let assistant = make_assistant(text);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_policy_ctx(&usage, &cost, &state);
    let turn = make_turn_ctx(&assistant, &[], &[]);
    PostTurnPolicy::evaluate(&policy, &ctx, &turn)
}

fn expect_inject_category(verdict: PolicyVerdict, expected: &str) {
    match verdict {
        PolicyVerdict::Inject(msgs) => {
            assert!(!msgs.is_empty(), "expected at least one injected message");
            let msg = &msgs[0];
            if let AgentMessage::Llm(LlmMessage::User(UserMessage { content, .. })) = msg {
                if let Some(ContentBlock::Extension { type_name, data }) = content.first() {
                    assert_eq!(type_name, "memory_nudge");
                    let category = data["category"].as_str().expect("category must be string");
                    assert_eq!(
                        category, expected,
                        "expected category {expected}, got {category}"
                    );
                    let confidence = data["confidence"].as_f64().expect("confidence required");
                    assert!(
                        confidence > 0.0 && confidence <= 1.0,
                        "confidence out of range: {confidence}"
                    );
                } else {
                    panic!("expected Extension content block, got: {content:?}");
                }
            } else {
                panic!("expected User message, got: {msg:?}");
            }
        }
        other => panic!("expected Inject verdict, got: {other:?}"),
    }
}

// ── T094 unit tests ───────────────────────────────────────────────────

#[test]
fn correction_phrase_triggers_correction_nudge() {
    let verdict = evaluate_text(
        "No, actually you should use serde_json for that.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "correction");
}

#[test]
fn dont_do_that_triggers_correction_nudge() {
    let verdict = evaluate_text(
        "Don't do that, use the builder pattern instead.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "correction");
}

#[test]
fn remember_this_triggers_explicit_save() {
    let verdict = evaluate_text(
        "Remember this: always run cargo fmt before committing.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "explicit_save");
}

#[test]
fn note_that_triggers_explicit_save() {
    let verdict = evaluate_text(
        "Note that the database password is rotated monthly.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "explicit_save");
}

#[test]
fn we_decided_triggers_decision() {
    let verdict = evaluate_text(
        "We decided to use Postgres for the primary datastore.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "decision");
}

#[test]
fn the_plan_is_triggers_decision() {
    let verdict = evaluate_text(
        "The plan is to migrate to async/await across the board.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "decision");
}

#[test]
fn i_prefer_triggers_preference() {
    let verdict = evaluate_text(
        "I prefer dark mode for all my editors.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "preference");
}

#[test]
fn always_use_triggers_preference() {
    let verdict = evaluate_text(
        "Always use snake_case for variable names.",
        NudgeSensitivity::Medium,
    );
    expect_inject_category(verdict, "preference");
}

#[test]
fn no_signal_returns_continue() {
    let verdict = evaluate_text(
        "The function takes two arguments and returns a string.",
        NudgeSensitivity::Medium,
    );
    assert!(
        matches!(verdict, PolicyVerdict::Continue),
        "ordinary text should return Continue, got: {verdict:?}"
    );
}

#[test]
fn empty_text_returns_continue() {
    let verdict = evaluate_text("", NudgeSensitivity::High);
    assert!(
        matches!(verdict, PolicyVerdict::Continue),
        "empty text should return Continue"
    );
}

#[test]
fn below_threshold_returns_continue() {
    // "i use " has base confidence 0.55 — below Low threshold of 0.75
    let verdict = evaluate_text(
        "In this project, I use a custom allocator.",
        NudgeSensitivity::Low,
    );
    assert!(
        matches!(verdict, PolicyVerdict::Continue),
        "low-confidence match should be suppressed at Low sensitivity, got: {verdict:?}"
    );
}

#[test]
fn high_sensitivity_triggers_on_borderline() {
    // "i use " has base confidence 0.55 — above High threshold of 0.35
    let verdict = evaluate_text(
        "In this project, I use a custom allocator.",
        NudgeSensitivity::High,
    );
    expect_inject_category(verdict, "preference");
}

#[test]
fn turn_number_stored_in_nudge() {
    let policy = MemoryNudgePolicy::new();
    let assistant = make_assistant("Remember this: always validate input.");
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = PolicyContext::new(7, &usage, &cost, 20, false, &[], &state);
    let turn = make_turn_ctx(&assistant, &[], &[]);
    match PostTurnPolicy::evaluate(&policy, &ctx, &turn) {
        PolicyVerdict::Inject(msgs) => {
            let msg = &msgs[0];
            if let AgentMessage::Llm(LlmMessage::User(UserMessage { content, .. })) = msg
                && let Some(ContentBlock::Extension { data, .. }) = content.first()
            {
                let turn_number = data["turn_number"].as_u64().expect("turn_number required");
                assert_eq!(turn_number, 7, "turn_number should match ctx.turn_index");
            }
        }
        other => panic!("expected Inject, got: {other:?}"),
    }
}

#[test]
fn summary_truncated_at_200_chars() {
    let long_text = format!("Remember this: {}", "x".repeat(300));
    let truncated = truncate_summary(&long_text, 200);
    // The unicode scalar count should be at most 201 (200 chars + ellipsis char)
    let char_count = truncated.chars().count();
    assert!(
        char_count <= 201,
        "summary should be truncated to ≤201 chars, got {char_count}"
    );
    assert!(
        truncated.ends_with('…'),
        "truncated summary should end with ellipsis"
    );
}

#[test]
fn nudge_sensitivity_thresholds() {
    assert!((NudgeSensitivity::Low.threshold() - 0.75).abs() < f32::EPSILON);
    assert!((NudgeSensitivity::Medium.threshold() - 0.55).abs() < f32::EPSILON);
    assert!((NudgeSensitivity::High.threshold() - 0.35).abs() < f32::EPSILON);
}

#[test]
fn category_as_str_values() {
    assert_eq!(MemoryNudgeCategory::Correction.as_str(), "correction");
    assert_eq!(MemoryNudgeCategory::ExplicitSave.as_str(), "explicit_save");
    assert_eq!(MemoryNudgeCategory::Decision.as_str(), "decision");
    assert_eq!(MemoryNudgeCategory::Preference.as_str(), "preference");
}

#[test]
fn memory_nudge_round_trips_through_extension_data() {
    // The documented contract: the `data` value embedded in the extension
    // block deserializes back into a `MemoryNudge`.
    let nudge = MemoryNudge::new(
        MemoryNudgeCategory::ExplicitSave,
        "Remember this: always validate input.",
        0.95,
        7,
    );
    let data = nudge.to_json();
    assert_eq!(data["category"], "explicit_save");

    let recovered: MemoryNudge =
        serde_json::from_value(data).expect("extension data must deserialize");
    assert_eq!(recovered.category, MemoryNudgeCategory::ExplicitSave);
    assert_eq!(recovered.summary, nudge.summary);
    assert!((recovered.confidence - nudge.confidence).abs() < f32::EPSILON);
    assert_eq!(recovered.turn_number, 7);
}

#[test]
fn category_serde_names_match_as_str() {
    for category in [
        MemoryNudgeCategory::Correction,
        MemoryNudgeCategory::ExplicitSave,
        MemoryNudgeCategory::Decision,
        MemoryNudgeCategory::Preference,
    ] {
        let json = serde_json::to_value(&category).unwrap();
        assert_eq!(json, category.as_str());
    }
}

#[test]
fn multiple_categories_emit_multiple_nudges() {
    // A message that triggers both ExplicitSave and Decision
    let verdict = evaluate_text(
        "Remember this: we decided to use axum for the web layer.",
        NudgeSensitivity::Medium,
    );
    match verdict {
        PolicyVerdict::Inject(msgs) => {
            assert!(
                msgs.len() >= 2,
                "expected nudges for both ExplicitSave and Decision, got {} messages",
                msgs.len()
            );
        }
        other => panic!("expected Inject, got: {other:?}"),
    }
}
