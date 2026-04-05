use std::collections::HashSet;

use regex::Regex;
use swink_agent::policy::{PolicyContext, PolicyVerdict, PostTurnPolicy, TurnPolicyContext};
use swink_agent::types::ContentBlock;

/// `PostTurnPolicy` that detects known prompt injection patterns in web content.
///
/// After each turn, this policy scans tool results from `web.*` tools for text
/// matching common prompt-injection signatures (e.g. "ignore previous
/// instructions", "you are now", fake `system:` prefixes). Because `PostTurnPolicy`
/// runs after messages are already committed to context, this policy **logs
/// warnings** via `tracing::warn!` rather than modifying content. It always
/// returns [`PolicyVerdict::Continue`].
///
/// This is defense-in-depth: the primary value is detection and auditing.
pub struct ContentSanitizerPolicy {
    patterns: Vec<Regex>,
}

impl ContentSanitizerPolicy {
    /// Create a new sanitizer with the default set of injection-detection patterns.
    pub fn new() -> Self {
        let pattern_strings = [
            r"(?i)ignore\s+(all\s+)?previous\s+instructions",
            r"(?i)you\s+are\s+now\s+",
            r"(?im)^system:\s*",
            r"(?i)IMPORTANT:\s*ignore",
            r"(?i)disregard\s+(all\s+)?(previous|above)",
            r"(?i)forget\s+(all\s+)?(previous|prior|above)\s+(instructions|context)",
            r"(?i)new\s+instructions?:\s*",
            r"(?i)override\s+(all\s+)?previous",
        ];
        let patterns = pattern_strings
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        Self { patterns }
    }

    /// Test whether the given text contains any injection patterns.
    ///
    /// Returns `Some(sanitized)` with matched patterns replaced by `[FILTERED]`
    /// if any pattern matched, or `None` if the text is clean.
    pub fn sanitize_text(&self, text: &str) -> Option<String> {
        let mut result = text.to_string();
        let mut modified = false;
        for pattern in &self.patterns {
            if pattern.is_match(&result) {
                result = pattern.replace_all(&result, "[FILTERED]").to_string();
                modified = true;
            }
        }
        if modified {
            Some(result)
        } else {
            None
        }
    }
}

impl Default for ContentSanitizerPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl PostTurnPolicy for ContentSanitizerPolicy {
    fn name(&self) -> &str {
        "web.sanitizer"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        // Collect tool-call IDs that belong to web.* tools from the assistant message.
        let web_call_ids: HashSet<&str> = turn
            .assistant_message
            .content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolCall {
                    id, name, ..
                } = block
                    && name.starts_with("web.") {
                        return Some(id.as_str());
                    }
                None
            })
            .collect();

        if web_call_ids.is_empty() {
            return PolicyVerdict::Continue;
        }

        // Scan only tool results that correspond to web.* tool calls.
        for result in turn.tool_results {
            if !web_call_ids.contains(result.tool_call_id.as_str()) {
                continue;
            }
            for block in &result.content {
                if let ContentBlock::Text { text } = block {
                    for pattern in &self.patterns {
                        if pattern.is_match(text) {
                            tracing::warn!(
                                tool_call_id = %result.tool_call_id,
                                pattern = %pattern.as_str(),
                                "Potential prompt injection detected in web content"
                            );
                        }
                    }
                }
            }
        }

        PolicyVerdict::Continue
    }
}
