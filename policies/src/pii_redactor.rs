//! PII redaction policy — strips personally identifiable information from assistant responses.

use regex::Regex;

use crate::patterns::{compile_named_regexes, compile_regex};

use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict,
    PostTurnPolicy, TurnPolicyContext,
};

/// Behaviour when PII is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PiiMode {
    /// Replace matched text with a placeholder (default).
    #[default]
    Redact,
    /// Stop the loop immediately, reporting which pattern matched.
    Stop,
}

/// A named regex pattern used for PII detection.
#[derive(Debug, Clone)]
pub struct PiiPattern {
    pub name: String,
    pub regex: Regex,
}

/// Detects and optionally redacts PII in assistant output.
///
/// Operates as a [`PostTurnPolicy`]: after each assistant turn it scans the
/// concatenated text blocks and either replaces matches with a placeholder
/// (`Redact` mode) or stops the loop (`Stop` mode).
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::{PiiRedactor, PiiMode};
///
/// let policy = PiiRedactor::new().with_mode(PiiMode::Stop);
/// ```
#[derive(Debug, Clone)]
pub struct PiiRedactor {
    patterns: Vec<PiiPattern>,
    mode: PiiMode,
    placeholder: String,
}

impl PiiRedactor {
    /// Create a `PiiRedactor` with default US-format PII patterns, `Redact`
    /// mode, and placeholder `[REDACTED]`.
    ///
    /// # Panics
    ///
    /// Panics if a built-in regex pattern fails to compile (should never happen).
    #[must_use]
    pub fn new() -> Self {
        Self {
            patterns: compile_named_regexes(default_patterns(), |name, regex| PiiPattern {
                name,
                regex,
            })
            .expect("default PII pattern must compile"),
            mode: PiiMode::default(),
            placeholder: "[REDACTED]".to_string(),
        }
    }

    /// Set the operating mode.
    #[must_use]
    pub const fn with_mode(mut self, mode: PiiMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set a custom replacement placeholder (only used in `Redact` mode).
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Add a custom named pattern. Returns an error if the regex is invalid.
    ///
    /// # Errors
    ///
    /// Returns [`regex::Error`] when `pattern` is not a valid regular expression.
    pub fn with_pattern(
        mut self,
        name: impl Into<String>,
        pattern: &str,
    ) -> Result<Self, regex::Error> {
        let regex = compile_regex(pattern)?;
        self.patterns.push(PiiPattern {
            name: name.into(),
            regex,
        });
        Ok(self)
    }
}

impl Default for PiiRedactor {
    fn default() -> Self {
        Self::new()
    }
}

const fn default_patterns() -> &'static [(&'static str, &'static str)] {
    &[
        ("email", r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}"),
        (
            "us_phone",
            r"(\+1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}",
        ),
        ("ssn", r"\d{3}-\d{2}-\d{4}"),
        ("credit_card", r"\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}"),
        ("ipv4", r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}"),
    ]
}

impl PostTurnPolicy for PiiRedactor {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "pii-redactor"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let text = ContentBlock::extract_text(&turn.assistant_message.content);

        let first_match = self
            .patterns
            .iter()
            .find(|pattern| pattern.regex.is_match(&text));
        let Some(first_match) = first_match else {
            return PolicyVerdict::Continue;
        };

        match self.mode {
            PiiMode::Stop => PolicyVerdict::Stop(format!("PII detected: {}", first_match.name)),
            PiiMode::Redact => {
                let mut redacted = text;
                for pattern in &self.patterns {
                    redacted = pattern
                        .regex
                        .replace_all(&redacted, self.placeholder.as_str())
                        .into_owned();
                }

                let orig = &turn.assistant_message;
                let msg = AssistantMessage {
                    content: vec![ContentBlock::Text { text: redacted }],
                    provider: orig.provider.clone(),
                    model_id: orig.model_id.clone(),
                    usage: orig.usage.clone(),
                    cost: orig.cost.clone(),
                    stop_reason: orig.stop_reason,
                    error_message: orig.error_message.clone(),
                    timestamp: orig.timestamp,
                    cache_hint: None,
                };

                PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::Assistant(msg))])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use swink_agent::{
        AssistantMessage, ContentBlock, Cost, PolicyContext, StopReason, ToolResultMessage,
        TurnPolicyContext, Usage,
    };

    fn make_turn_ctx(text: &str) -> (AssistantMessage, Vec<ToolResultMessage>) {
        let msg = AssistantMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            provider: "test".into(),
            model_id: "test-model".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 12345,
            cache_hint: None,
        };
        (msg, vec![])
    }

    fn make_policy_ctx() -> (Usage, Cost) {
        (Usage::default(), Cost::default())
    }

    fn evaluate_text(policy: &PiiRedactor, text: &str) -> PolicyVerdict {
        let (msg, results) = make_turn_ctx(text);
        let (usage, cost) = make_policy_ctx();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 0,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 1,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let turn = TurnPolicyContext {
            assistant_message: &msg,
            tool_results: &results,
            stop_reason: StopReason::Stop,
        };
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
}
