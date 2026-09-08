//! PII redaction policy — strips personally identifiable information from assistant responses.

use regex::Regex;

use crate::patterns::{compile_named_regexes, compile_regex};

use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict,
    PostTurnPolicy, TurnPolicyContext,
};

/// Behaviour when PII is detected.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PiiMode {
    /// Replace matched text with a placeholder (default).
    #[default]
    Redact,
    /// Stop the loop immediately, reporting which pattern matched.
    Stop,
}

/// A named regex pattern used for PII detection.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PiiPattern {
    pub name: String,
    pub regex: Regex,
}

impl PiiPattern {
    /// Compile `pattern` into a standalone named pattern.
    ///
    /// This is the direct counterpart to [`PiiRedactor::with_pattern`] for
    /// constructing patterns outside a redactor (e.g. in unit tests).
    ///
    /// # Errors
    ///
    /// Returns [`regex::Error`] when `pattern` is not a valid regular
    /// expression.
    pub fn new(name: impl Into<String>, pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            name: name.into(),
            regex: compile_regex(pattern)?,
        })
    }
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
        self.patterns.push(PiiPattern::new(name, pattern)?);
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
                let mut msg = AssistantMessage::new(
                    vec![ContentBlock::Text { text: redacted }],
                    orig.provider.clone(),
                    orig.model_id.clone(),
                )
                .with_usage(orig.usage.clone())
                .with_cost(orig.cost.clone())
                .with_stop_reason(orig.stop_reason)
                .with_timestamp(orig.timestamp);
                if let Some(error_kind) = orig.error_kind {
                    msg = msg.with_error_kind(error_kind);
                }
                if let Some(error_message) = orig.error_message.clone() {
                    msg = msg.with_error_message(error_message);
                }

                PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::Assistant(msg))])
            }
        }
    }
}

#[cfg(test)]
#[path = "pii_redactor_tests.rs"]
mod tests;
