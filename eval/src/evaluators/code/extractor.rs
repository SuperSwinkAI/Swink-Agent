//! Code extraction strategies (T078).
//!
//! A [`CodeExtractor`] lifts code out of an assistant response using one of
//! three built-in strategies. This is the single choke-point used by every
//! code-family evaluator so extraction logic isn't re-implemented per
//! evaluator.

use regex::Regex;
use std::sync::Arc;

use crate::judge::JudgeClient;

/// Strategy selector for [`CodeExtractor`].
#[derive(Clone)]
pub enum CodeExtractorStrategy {
    /// Match fenced markdown code blocks. When `language` is `Some`, only
    /// fences tagged with the given language are returned.
    MarkdownFence {
        /// Optional language tag to require on the opening fence.
        language: Option<String>,
    },
    /// Arbitrary regex whose first capture group is the extracted code.
    Regex { pattern: Regex },
    /// Ask a judge to return the code block most relevant to the response.
    ///
    /// The supplied prompt is rendered as-is with the response appended; the
    /// judge's `reason` field is returned as the extracted code to keep the
    /// contract free of schema assumptions.
    Llm {
        /// Prompt prepended to the response before the judge dispatch.
        prompt: String,
        /// Judge client used for the dispatch.
        judge: Arc<dyn JudgeClient>,
    },
}

impl std::fmt::Debug for CodeExtractorStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MarkdownFence { language } => f
                .debug_struct("MarkdownFence")
                .field("language", language)
                .finish(),
            Self::Regex { pattern } => f
                .debug_struct("Regex")
                .field("pattern", &pattern.as_str())
                .finish(),
            Self::Llm { prompt, .. } => f
                .debug_struct("Llm")
                .field("prompt_len", &prompt.len())
                .finish(),
        }
    }
}

/// Strategy object that extracts code from an assistant response.
///
/// Each call to [`Self::extract`] returns the first matching snippet found,
/// or `None` if no code was extracted. The extractor is deterministic for
/// `MarkdownFence` and `Regex` strategies; the `Llm` strategy is inherently
/// judge-backed and therefore non-deterministic.
pub struct CodeExtractor {
    strategy: CodeExtractorStrategy,
}

impl CodeExtractor {
    /// Create an extractor with the given strategy.
    #[must_use]
    pub const fn new(strategy: CodeExtractorStrategy) -> Self {
        Self { strategy }
    }

    /// Convenience: markdown-fence extractor with no language requirement.
    #[must_use]
    pub const fn markdown_fence() -> Self {
        Self::new(CodeExtractorStrategy::MarkdownFence { language: None })
    }

    /// Extract the first matching code snippet from `response`.
    pub async fn extract(&self, response: &str) -> Option<String> {
        match &self.strategy {
            CodeExtractorStrategy::MarkdownFence { language } => {
                extract_markdown_fence(response, language.as_deref())
            }
            CodeExtractorStrategy::Regex { pattern } => {
                pattern.captures(response).and_then(|caps| {
                    caps.get(1)
                        .or_else(|| caps.get(0))
                        .map(|m| m.as_str().to_string())
                })
            }
            CodeExtractorStrategy::Llm { prompt, judge } => {
                let rendered = format!("{prompt}\n\n---\n{response}");
                match judge.judge(&rendered).await {
                    Ok(verdict) => verdict.reason,
                    Err(_) => None,
                }
            }
        }
    }
}

fn extract_markdown_fence(response: &str, required_language: Option<&str>) -> Option<String> {
    let mut lines = response.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("```") else {
            continue;
        };
        let tag = rest.trim();
        if let Some(expected) = required_language {
            if !tag.eq_ignore_ascii_case(expected) {
                continue;
            }
        }
        let mut body = String::new();
        for inner in lines.by_ref() {
            let inner_trimmed = inner.trim_start();
            if inner_trimmed.starts_with("```") {
                return Some(body.trim_end_matches('\n').to_string());
            }
            body.push_str(inner);
            body.push('\n');
        }
        // Unterminated fence — return whatever we collected.
        return Some(body.trim_end_matches('\n').to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_fence_extracts_first_block() {
        let response =
            "Here is the code:\n\n```rust\nfn add(a: i32, b: i32) -> i32 { a + b }\n```\n";
        let out = extract_markdown_fence(response, Some("rust"));
        assert_eq!(
            out.as_deref(),
            Some("fn add(a: i32, b: i32) -> i32 { a + b }")
        );
    }

    #[test]
    fn markdown_fence_skips_non_matching_language() {
        let response = "```python\nprint('hi')\n```\n\n```rust\nfn a() {}\n```\n";
        let out = extract_markdown_fence(response, Some("rust"));
        assert_eq!(out.as_deref(), Some("fn a() {}"));
    }

    #[test]
    fn markdown_fence_ignores_language_when_none() {
        let response = "```\nanything\n```";
        let out = extract_markdown_fence(response, None);
        assert_eq!(out.as_deref(), Some("anything"));
    }
}
