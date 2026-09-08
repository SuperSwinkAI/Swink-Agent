//! Tests for `pii_redactor`.
#![cfg(test)]

use super::*;

use swink_agent::{
    AssistantMessage, ContentBlock, Cost, PolicyContext, StopReason, ToolResultMessage,
    TurnPolicyContext, Usage,
};

fn make_turn_ctx(text: &str) -> (AssistantMessage, Vec<ToolResultMessage>) {
    let msg = AssistantMessage::new(
        vec![ContentBlock::Text { text: text.into() }],
        "test",
        "test-model",
    )
    .with_timestamp(12345);
    (msg, vec![])
}

fn make_policy_ctx() -> (Usage, Cost) {
    (Usage::default(), Cost::default())
}

fn evaluate_text(policy: &PiiRedactor, text: &str) -> PolicyVerdict {
    let (msg, results) = make_turn_ctx(text);
    let (usage, cost) = make_policy_ctx();
    let state = swink_agent::SessionState::new();
    let ctx = PolicyContext::new(0, &usage, &cost, 1, false, &[], &state);
    static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
        std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
    let turn = TurnPolicyContext::new(&msg, &results, StopReason::Stop, "", &MODEL, &[]);
    policy.evaluate(&ctx, &turn)
}

fn assert_redacted(verdict: PolicyVerdict, expected_text: &str) {
    match verdict {
        PolicyVerdict::Inject(messages) => {
            assert_eq!(messages.len(), 1);
            if let AgentMessage::Llm(LlmMessage::Assistant(msg)) = &messages[0] {
                let text = ContentBlock::extract_text(&msg.content);
                assert_eq!(text, expected_text);
            } else {
                panic!("expected Llm(Assistant(...))");
            }
        }
        other => panic!("expected Inject, got {other:?}"),
    }
}

#[test]
fn redacts_email() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Contact john@example.com for details");
    assert_redacted(verdict, "Contact [REDACTED] for details");
}

#[test]
fn redacts_phone() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Call 555-123-4567");
    assert_redacted(verdict, "Call [REDACTED]");
}

#[test]
fn redacts_ssn() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "SSN is 123-45-6789");
    assert_redacted(verdict, "SSN is [REDACTED]");
}

#[test]
fn redacts_credit_card() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Card 4111 1111 1111 1111");
    assert_redacted(verdict, "Card [REDACTED]");
}

#[test]
fn redacts_ipv4() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Server at 192.168.1.1");
    assert_redacted(verdict, "Server at [REDACTED]");
}

#[test]
fn redacts_multiple_pii_types() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Email alice@test.org and call 555-123-4567 please");
    assert_redacted(verdict, "Email [REDACTED] and call [REDACTED] please");
}

#[test]
fn overlapping_matches_resolved_left_to_right() {
    // Patterns are applied in order: email first, then phone, etc.
    // This test verifies sequential replacement doesn't corrupt output.
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(
        &policy,
        "user@mail.com called from 555-111-2222 and 555-333-4444",
    );
    assert_redacted(verdict, "[REDACTED] called from [REDACTED] and [REDACTED]");
}

#[test]
fn no_pii_returns_continue() {
    let policy = PiiRedactor::new();
    let verdict = evaluate_text(&policy, "Hello, how can I help you today?");
    assert!(matches!(verdict, PolicyVerdict::Continue));
}

#[test]
fn stop_mode_returns_stop() {
    let policy = PiiRedactor::new().with_mode(PiiMode::Stop);
    let verdict = evaluate_text(&policy, "My email is test@example.com");
    match verdict {
        PolicyVerdict::Stop(reason) => {
            assert!(reason.contains("PII detected"), "reason: {reason}");
            assert!(reason.contains("email"), "reason: {reason}");
        }
        other => panic!("expected Stop, got {other:?}"),
    }
}

#[test]
fn custom_placeholder_used() {
    let policy = PiiRedactor::new().with_placeholder("[REMOVED]");
    let verdict = evaluate_text(&policy, "Email admin@corp.io here");
    assert_redacted(verdict, "Email [REMOVED] here");
}

#[test]
fn custom_pattern_works() {
    let policy = PiiRedactor::new()
        .with_pattern("custom_id", r"ID-\d{6}")
        .expect("valid regex");
    let verdict = evaluate_text(&policy, "Reference ID-123456 noted");
    assert_redacted(verdict, "Reference [REDACTED] noted");
}

#[test]
fn pii_pattern_new_compiles_standalone_pattern() {
    let pattern = PiiPattern::new("badge_id", r"BADGE-\d{4}").expect("valid regex");
    assert_eq!(pattern.name, "badge_id");
    assert!(pattern.regex.is_match("Visitor BADGE-1234 arrived."));
}

#[test]
fn pii_pattern_new_rejects_invalid_regex() {
    assert!(PiiPattern::new("broken", "[invalid").is_err());
}
