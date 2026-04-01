//! Keyword/regex blocklist policy for assistant output.

use std::collections::HashSet;

use regex::Regex;

use swink_agent::{ContentBlock, PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext};

// ─── Types ──────────────────────────────────────────────────────────────────

/// A single filter rule consisting of a compiled regex and metadata.
#[derive(Debug)]
pub struct FilterRule {
    /// The compiled regex pattern.
    pub pattern: Regex,
    /// Human-readable name shown when the rule triggers.
    pub display_name: String,
    /// Optional category for selective filtering.
    pub category: Option<String>,
}

/// Errors returned when constructing a [`ContentFilter`].
#[derive(Debug)]
pub enum ContentFilterError {
    /// The supplied regex pattern failed to compile.
    InvalidRegex {
        /// The original pattern string.
        pattern: String,
        /// The underlying regex compilation error.
        source: regex::Error,
    },
}

impl std::fmt::Display for ContentFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRegex { pattern, source } => {
                write!(f, "invalid regex pattern `{pattern}`: {source}")
            }
        }
    }
}

impl std::error::Error for ContentFilterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRegex { source, .. } => Some(source),
        }
    }
}

// ─── ContentFilter ──────────────────────────────────────────────────────────

/// Keyword/regex blocklist for assistant output.
///
/// Rules can optionally be organized into categories, and only active
/// categories are checked when `enabled_categories` is set.
#[derive(Debug)]
pub struct ContentFilter {
    rules: Vec<FilterRule>,
    enabled_categories: Option<HashSet<String>>,
    case_insensitive: bool,
    whole_word: bool,
}

impl ContentFilter {
    /// Creates an empty filter with case-insensitive matching enabled and
    /// whole-word matching disabled.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            rules: Vec::new(),
            enabled_categories: None,
            case_insensitive: true,
            whole_word: false,
        }
    }

    /// Adds a keyword rule. The keyword is regex-escaped and modified according
    /// to the current `case_insensitive` and `whole_word` settings.
    ///
    /// # Panics
    ///
    /// Panics if the escaped keyword somehow produces an invalid regex (should
    /// never happen in practice).
    #[must_use]
    pub fn with_keyword(mut self, word: impl Into<String>) -> Self {
        let word = word.into();
        let escaped = regex::escape(&word);
        let pattern_str = self.build_pattern(&escaped);
        // Safety: escaped keyword with optional flags is always valid regex.
        let pattern = Regex::new(&pattern_str)
            .expect("escaped keyword should always produce valid regex");
        self.rules.push(FilterRule {
            pattern,
            display_name: word,
            category: None,
        });
        self
    }

    /// Adds a raw regex rule.
    ///
    /// # Errors
    ///
    /// Returns [`ContentFilterError::InvalidRegex`] if the pattern fails to
    /// compile.
    pub fn with_regex(mut self, pattern: &str) -> Result<Self, ContentFilterError> {
        let compiled = Regex::new(pattern).map_err(|source| ContentFilterError::InvalidRegex {
            pattern: pattern.to_string(),
            source,
        })?;
        self.rules.push(FilterRule {
            pattern: compiled,
            display_name: pattern.to_string(),
            category: None,
        });
        Ok(self)
    }

    /// Adds a keyword rule with a category tag.
    ///
    /// # Panics
    ///
    /// Panics if the escaped keyword somehow produces an invalid regex (should
    /// never happen in practice).
    #[must_use]
    pub fn with_category_keyword(
        mut self,
        category: impl Into<String>,
        word: impl Into<String>,
    ) -> Self {
        let word = word.into();
        let category = category.into();
        let escaped = regex::escape(&word);
        let pattern_str = self.build_pattern(&escaped);
        let pattern = Regex::new(&pattern_str)
            .expect("escaped keyword should always produce valid regex");
        self.rules.push(FilterRule {
            pattern,
            display_name: word,
            category: Some(category),
        });
        self
    }

    /// Adds a raw regex rule with a category tag.
    ///
    /// # Errors
    ///
    /// Returns [`ContentFilterError::InvalidRegex`] if the pattern fails to
    /// compile.
    pub fn with_category_regex(
        mut self,
        category: impl Into<String>,
        pattern: &str,
    ) -> Result<Self, ContentFilterError> {
        let compiled = Regex::new(pattern).map_err(|source| ContentFilterError::InvalidRegex {
            pattern: pattern.to_string(),
            source,
        })?;
        self.rules.push(FilterRule {
            pattern: compiled,
            display_name: pattern.to_string(),
            category: Some(category.into()),
        });
        Ok(self)
    }

    /// Sets whether future keyword additions are case-insensitive.
    #[must_use]
    pub const fn with_case_insensitive(mut self, enabled: bool) -> Self {
        self.case_insensitive = enabled;
        self
    }

    /// Sets whether future keyword additions require whole-word matches.
    #[must_use]
    pub const fn with_whole_word(mut self, enabled: bool) -> Self {
        self.whole_word = enabled;
        self
    }

    /// Restricts which categories are checked. Only rules whose category is in
    /// the given set (or rules with no category) are evaluated. When not set,
    /// all rules are evaluated.
    #[must_use]
    pub fn with_enabled_categories(
        mut self,
        categories: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.enabled_categories = Some(categories.into_iter().map(Into::into).collect());
        self
    }

    /// Builds a regex pattern string from an already-escaped keyword, applying
    /// the current `case_insensitive` and `whole_word` flags.
    fn build_pattern(&self, escaped: &str) -> String {
        let mut pat = String::new();
        if self.case_insensitive {
            pat.push_str("(?i)");
        }
        if self.whole_word {
            pat.push_str(r"\b");
        }
        pat.push_str(escaped);
        if self.whole_word {
            pat.push_str(r"\b");
        }
        pat
    }

    /// Returns whether the given rule should be evaluated given the current
    /// category configuration.
    fn should_evaluate_rule(&self, rule: &FilterRule) -> bool {
        match (&rule.category, &self.enabled_categories) {
            (Some(cat), Some(enabled)) => enabled.contains(cat),
            _ => true,
        }
    }
}

impl Default for ContentFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl PostTurnPolicy for ContentFilter {
    fn name(&self) -> &'static str {
        "ContentFilter"
    }

    fn evaluate(
        &self,
        _ctx: &PolicyContext<'_>,
        turn: &TurnPolicyContext<'_>,
    ) -> PolicyVerdict {
        let text = ContentBlock::extract_text(&turn.assistant_message.content);
        for rule in &self.rules {
            if !self.should_evaluate_rule(rule) {
                continue;
            }
            if rule.pattern.is_match(&text) {
                return PolicyVerdict::Stop(format!(
                    "Content filter triggered: {}",
                    rule.display_name
                ));
            }
        }
        PolicyVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use swink_agent::{AssistantMessage, Cost, StopReason, Usage};

    use super::*;

    fn make_policy_ctx<'a>(usage: &'a Usage, cost: &'a Cost, state: &'a swink_agent::SessionState) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    fn make_turn_ctx(msg: &AssistantMessage) -> TurnPolicyContext<'_> {
        TurnPolicyContext {
            assistant_message: msg,
            tool_results: &[],
            stop_reason: StopReason::Stop,
        }
    }

    fn make_msg(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
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
        assert!(
            matches!(verdict, PolicyVerdict::Stop(reason) if reason.contains("secret-project"))
        );
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
        assert!(
            matches!(verdict, PolicyVerdict::Stop(reason) if reason.contains("restricted"))
        );
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
}
