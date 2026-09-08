// Prompt injection guard policy.
//
// Scans user messages (PreTurn) and tool results (PostTurn) for patterns
// commonly used in prompt injection attacks. Matches trigger an immediate
// `Stop` verdict, halting the agent loop.

use regex::Regex;

use crate::patterns::compile_case_insensitive_regex;

use swink_agent::{
    AgentMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict, PostTurnPolicy,
    PreTurnPolicy, TurnPolicyContext,
};

/// A single named, pre-compiled regex pattern used internally by [`PromptInjectionGuard`].
struct NamedPattern {
    name: String,
    regex: Regex,
}

impl NamedPattern {
    fn new(name: impl Into<String>, pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            name: name.into(),
            regex: compile_case_insensitive_regex(pattern)?,
        })
    }
}

/// A policy that detects prompt injection attempts in user messages and tool results.
///
/// Ships with ~10 default patterns targeting common injection phrases. Custom
/// patterns can be added via [`with_pattern`](Self::with_pattern). Each pattern
/// is compiled as a case-insensitive regex.
///
/// # Slots
///
/// - **`PreTurn`**: scans new user messages for direct injection.
/// - **`PostTurn`**: scans tool results for indirect injection.
pub struct PromptInjectionGuard {
    patterns: Vec<NamedPattern>,
}

impl PromptInjectionGuard {
    /// Creates a guard loaded with the default injection patterns.
    ///
    /// # Panics
    ///
    /// Panics if a built-in default pattern fails to compile (indicates a bug).
    #[must_use]
    pub fn new() -> Self {
        let defaults = default_patterns();
        let patterns = defaults
            .iter()
            .map(|(name, pat)| NamedPattern::new(*name, pat).expect("default pattern must compile"))
            .collect();

        Self { patterns }
    }

    /// Creates an empty guard with no patterns. Use [`with_pattern`](Self::with_pattern)
    /// to add custom patterns.
    #[must_use]
    pub const fn without_defaults() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    /// Adds a custom case-insensitive pattern to the guard.
    ///
    /// # Errors
    ///
    /// Returns `regex::Error` if `pattern` is not a valid regular expression.
    pub fn with_pattern(
        mut self,
        name: impl Into<String>,
        pattern: &str,
    ) -> Result<Self, regex::Error> {
        self.patterns.push(NamedPattern::new(name, pattern)?);
        Ok(self)
    }

    /// Checks `text` against all patterns. Returns the name of the first match, if any.
    fn check(&self, text: &str) -> Option<&str> {
        for pattern in &self.patterns {
            if pattern.regex.is_match(text) {
                return Some(&pattern.name);
            }
        }
        None
    }
}

impl Default for PromptInjectionGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl PreTurnPolicy for PromptInjectionGuard {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "PromptInjectionGuard"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        for msg in ctx.new_messages {
            if let AgentMessage::Llm(LlmMessage::User(user_msg)) = msg {
                let text = ContentBlock::extract_text(&user_msg.content);
                if let Some(pattern_name) = self.check(&text) {
                    return PolicyVerdict::Stop(format!(
                        "Prompt injection detected: {pattern_name}"
                    ));
                }
            }
        }
        PolicyVerdict::Continue
    }
}

impl PostTurnPolicy for PromptInjectionGuard {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "PromptInjectionGuard"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        for result in turn.tool_results {
            let text = ContentBlock::extract_text(&result.content);
            if let Some(pattern_name) = self.check(&text) {
                return PolicyVerdict::Stop(format!(
                    "Indirect prompt injection detected in tool result: {pattern_name}"
                ));
            }
        }
        PolicyVerdict::Continue
    }
}

/// Returns the default set of (name, pattern) pairs.
///
/// Patterns are crafted to be specific enough to avoid false positives on
/// benign phrases like "please ignore the previous error".
const fn default_patterns() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "ignore_all_previous_instructions",
            r"ignore\s+all\s+previous\s+instructions",
        ),
        (
            "disregard_system_prompt",
            r"disregard\s+your\s+system\s+prompt",
        ),
        ("you_are_now_a", r"you\s+are\s+now\s+a\b"),
        ("forget_your_instructions", r"forget\s+your\s+instructions"),
        (
            "override_your_programming",
            r"override\s+your\s+programming",
        ),
        ("new_persona", r"new\s+persona"),
        ("jailbreak", r"\bjailbreak\b"),
        ("pretend_you_are", r"pretend\s+you\s+are\b"),
        (
            "act_as_no_restrictions",
            r"act\s+as\s+if\s+you\s+have\s+no\s+restrictions",
        ),
        ("ignore_the_above", r"ignore\s+the\s+above\b"),
    ]
}

#[cfg(test)]
#[path = "prompt_guard_tests.rs"]
mod tests;
