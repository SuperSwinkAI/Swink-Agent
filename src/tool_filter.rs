//! Pattern-based tool filtering at registration time.
//!
//! [`ToolFilter`] uses exact, glob, and regex patterns to restrict which tools
//! are available to the agent. Patterns are applied at registration time so that
//! filtered tools never appear in the LLM prompt.
//!
//! # Example
//!
//! ```
//! use swink_agent::tool_filter::{ToolFilter, ToolPattern};
//!
//! let filter = ToolFilter::new()
//!     .with_allowed(vec![ToolPattern::parse("read_*")])
//!     .with_rejected(vec![ToolPattern::parse("read_secret")]);
//!
//! assert!(filter.is_allowed("read_file"));
//! assert!(!filter.is_allowed("read_secret"));
//! assert!(!filter.is_allowed("bash"));
//! ```

use std::sync::Arc;

use regex::Regex;

use crate::tool::AgentTool;

// ─── ToolPattern ────────────────────────────────────────────────────────────

/// A pattern for matching tool names.
///
/// Auto-detected by [`parse()`](ToolPattern::parse):
/// - Strings starting with `^` or ending with `$` → [`Regex`](ToolPattern::Regex)
/// - Strings containing `*` or `?` → [`Glob`](ToolPattern::Glob)
/// - Everything else → [`Exact`](ToolPattern::Exact)
#[derive(Debug, Clone)]
pub enum ToolPattern {
    /// Match the tool name exactly.
    Exact(String),
    /// Match using glob syntax (`*` = any chars, `?` = single char).
    Glob(String),
    /// Match using a regular expression.
    Regex(Regex),
}

impl ToolPattern {
    /// Parse a pattern string, auto-detecting the pattern type.
    #[must_use]
    pub fn parse(pattern: &str) -> Self {
        if pattern.starts_with('^') || pattern.ends_with('$') {
            Regex::new(pattern).map_or_else(
                |_| Self::Exact(pattern.to_string()),
                Self::Regex,
            )
        } else if pattern.contains('*') || pattern.contains('?') {
            Self::Glob(pattern.to_string())
        } else {
            Self::Exact(pattern.to_string())
        }
    }

    /// Test whether this pattern matches the given tool name.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Self::Exact(pat) => name == pat,
            Self::Glob(pat) => glob_matches(pat, name),
            Self::Regex(re) => re.is_match(name),
        }
    }
}

/// Simple glob matching: `*` matches any sequence, `?` matches one char.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut regex_str = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex_str.push_str(".*"),
            '?' => regex_str.push('.'),
            c => {
                let escaped = regex::escape(&c.to_string());
                regex_str.push_str(&escaped);
            }
        }
    }
    regex_str.push('$');
    Regex::new(&regex_str).is_ok_and(|re| re.is_match(text))
}

// ─── ToolFilter ─────────────────────────────────────────────────────────────

/// Filters tools at registration time using pattern-based allow/reject lists.
///
/// When both `allowed` and `rejected` match a tool name, `rejected` takes
/// precedence — the tool is excluded.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    /// Patterns that a tool name must match to be included. Empty = allow all.
    allowed: Vec<ToolPattern>,
    /// Patterns that exclude a tool name. Takes precedence over `allowed`.
    rejected: Vec<ToolPattern>,
}

impl ToolFilter {
    /// Create a new empty filter (allows all tools).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the allowed patterns.
    #[must_use]
    pub fn with_allowed(mut self, patterns: Vec<ToolPattern>) -> Self {
        self.allowed = patterns;
        self
    }

    /// Set the rejected patterns.
    #[must_use]
    pub fn with_rejected(mut self, patterns: Vec<ToolPattern>) -> Self {
        self.rejected = patterns;
        self
    }

    /// Test whether a tool name passes through this filter.
    #[must_use]
    pub fn is_allowed(&self, name: &str) -> bool {
        // Rejected takes precedence.
        if self.rejected.iter().any(|p| p.matches(name)) {
            return false;
        }
        // If no allowed patterns, everything passes. Otherwise must match at least one.
        if self.allowed.is_empty() {
            return true;
        }
        self.allowed.iter().any(|p| p.matches(name))
    }

    /// Filter a list of tools, returning only those that pass the filter.
    #[must_use]
    pub fn filter_tools(&self, tools: Vec<Arc<dyn AgentTool>>) -> Vec<Arc<dyn AgentTool>> {
        tools
            .into_iter()
            .filter(|t| self.is_allowed(t.name()))
            .collect()
    }
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolFilter>();
    assert_send_sync::<ToolPattern>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_pattern_matches() {
        let pat = ToolPattern::parse("bash");
        assert!(pat.matches("bash"));
        assert!(!pat.matches("read_file"));
    }

    #[test]
    fn glob_pattern_matches() {
        let pat = ToolPattern::parse("read_*");
        assert!(pat.matches("read_file"));
        assert!(pat.matches("read_secret"));
        assert!(!pat.matches("write_file"));
    }

    #[test]
    fn glob_question_mark_matches_single_char() {
        let pat = ToolPattern::parse("tool_?");
        assert!(pat.matches("tool_a"));
        assert!(!pat.matches("tool_ab"));
    }

    #[test]
    fn regex_pattern_matches() {
        let pat = ToolPattern::parse("^file_.*$");
        assert!(pat.matches("file_read"));
        assert!(pat.matches("file_write"));
        assert!(!pat.matches("bash"));
    }

    #[test]
    fn rejected_takes_precedence() {
        let filter = ToolFilter::new()
            .with_allowed(vec![ToolPattern::parse("read_*")])
            .with_rejected(vec![ToolPattern::parse("read_secret")]);

        assert!(filter.is_allowed("read_file"));
        assert!(!filter.is_allowed("read_secret"));
    }

    #[test]
    fn empty_filter_allows_all() {
        let filter = ToolFilter::new();
        assert!(filter.is_allowed("anything"));
        assert!(filter.is_allowed("bash"));
    }

    #[test]
    fn allowed_only_restricts_to_matching() {
        let filter = ToolFilter::new()
            .with_allowed(vec![ToolPattern::parse("bash")]);
        assert!(filter.is_allowed("bash"));
        assert!(!filter.is_allowed("read_file"));
    }

    #[test]
    fn rejected_only_excludes_matching() {
        let filter = ToolFilter::new()
            .with_rejected(vec![ToolPattern::parse("bash")]);
        assert!(!filter.is_allowed("bash"));
        assert!(filter.is_allowed("read_file"));
    }

    #[test]
    fn invalid_regex_falls_back_to_exact() {
        let pat = ToolPattern::parse("^[invalid");
        // Falls back to exact match since regex is invalid
        assert!(pat.matches("^[invalid"));
    }

    #[test]
    fn parse_detects_pattern_type() {
        assert!(matches!(ToolPattern::parse("exact"), ToolPattern::Exact(_)));
        assert!(matches!(ToolPattern::parse("glob_*"), ToolPattern::Glob(_)));
        assert!(matches!(ToolPattern::parse("^regex$"), ToolPattern::Regex(_)));
    }
}
