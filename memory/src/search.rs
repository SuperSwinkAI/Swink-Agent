//! Cross-session search types and helpers.
//!
//! The `search` feature gate enables a tantivy-backed full-text index stored
//! alongside the session JSONL files.  Without the feature the module still
//! compiles and provides the public types; `JsonlSessionStore::search()` falls
//! back to the linear scan already implemented in `jsonl.rs`.

use chrono::{DateTime, Utc};
use swink_agent::{ContentBlock, LlmMessage};

use crate::entry::SessionEntry;

#[cfg(feature = "search")]
pub(crate) mod index;

const DEFAULT_MAX_RESULTS: usize = 50;
const SNIPPET_CONTEXT_BYTES: usize = 72;

/// Options for searching persisted sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSearchOptions {
    /// Restrict search to these session IDs.
    pub session_ids: Option<Vec<String>>,
    /// Restrict search to entry discriminator names such as `"message"` or `"label"`.
    pub entry_types: Option<Vec<String>>,
    /// Include entries at or after this timestamp.
    pub start_time: Option<DateTime<Utc>>,
    /// Include entries at or before this timestamp.
    pub end_time: Option<DateTime<Utc>>,
    /// Maximum number of hits to return. Defaults to 50.
    pub max_results: Option<usize>,
}

impl SessionSearchOptions {
    pub(crate) fn limit(&self) -> usize {
        self.max_results.unwrap_or(DEFAULT_MAX_RESULTS)
    }
}

/// A single cross-session search result.
#[derive(Debug, Clone)]
pub struct SessionHit {
    /// ID of the session containing the match.
    pub session_id: String,
    /// Title from the matched session metadata.
    pub session_title: String,
    /// Matched entry from the session log.
    pub entry: SessionEntry,
    /// Simple term-frequency score for result ordering.
    pub score: usize,
    /// Compact preview around the first matched term.
    pub snippet: String,
}

#[cfg(not(feature = "search"))]
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(not(feature = "search"))]
pub(crate) fn entry_matches_time_range(
    entry: &SessionEntry,
    options: &SessionSearchOptions,
) -> bool {
    let Some(timestamp) = entry.timestamp() else {
        return options.start_time.is_none() && options.end_time.is_none();
    };

    if let Some(start) = options.start_time
        && timestamp < start.timestamp().cast_unsigned()
    {
        return false;
    }
    if let Some(end) = options.end_time
        && timestamp > end.timestamp().cast_unsigned()
    {
        return false;
    }
    true
}

#[cfg(not(feature = "search"))]
pub(crate) fn entry_matches_type(entry: &SessionEntry, options: &SessionSearchOptions) -> bool {
    options.entry_types.as_ref().is_none_or(|types| {
        types
            .iter()
            .any(|expected| expected == entry.entry_type_name())
    })
}

#[cfg(not(feature = "search"))]
pub(crate) fn search_entry(entry: &SessionEntry, terms: &[String]) -> Option<(usize, String)> {
    if terms.is_empty() {
        return None;
    }

    let text = searchable_text(entry);
    if text.is_empty() {
        return None;
    }

    let haystack = text.to_lowercase();
    let mut score = 0;
    let mut first_match = None;
    for term in terms {
        let mut matches = haystack.match_indices(term);
        let (idx, _) = matches.next()?;
        first_match = Some(first_match.map_or(idx, |current: usize| current.min(idx)));
        score += 1 + matches.count();
    }

    Some((score, snippet(&text, first_match.unwrap_or(0))))
}

/// Public-crate wrapper used by the tantivy index module.
#[cfg(feature = "search")]
pub(crate) fn searchable_text_pub(entry: &SessionEntry) -> String {
    searchable_text(entry)
}

fn searchable_text(entry: &SessionEntry) -> String {
    match entry {
        SessionEntry::Message(message) => message_text(message),
        SessionEntry::ModelChange {
            from,
            to,
            timestamp: _,
        } => format!(
            "model change {} {} {} {}",
            from.provider, from.model_id, to.provider, to.model_id
        ),
        SessionEntry::ThinkingLevelChange {
            from,
            to,
            timestamp: _,
        } => {
            format!("thinking level change {from} {to}")
        }
        SessionEntry::Compaction {
            dropped_count,
            tokens_before,
            tokens_after,
            timestamp: _,
        } => format!(
            "compaction dropped {dropped_count} tokens before {tokens_before} after {tokens_after}"
        ),
        SessionEntry::Label {
            text,
            message_index,
            timestamp: _,
        } => format!("label {text} message {message_index}"),
        SessionEntry::Custom {
            type_name,
            data,
            timestamp: _,
        } => format!("custom {type_name} {data}"),
    }
}

fn message_text(message: &LlmMessage) -> String {
    match message {
        LlmMessage::User(message) => content_text(&message.content),
        LlmMessage::Assistant(message) => content_text(&message.content),
        LlmMessage::ToolResult(message) => {
            let mut text = content_text(&message.content);
            if !message.details.is_null() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&message.details.to_string());
            }
            text
        }
    }
}

fn content_text(blocks: &[ContentBlock]) -> String {
    let mut text = ContentBlock::extract_text(blocks);
    for block in blocks {
        match block {
            ContentBlock::Thinking {
                thinking,
                signature: _,
            } => push_part(&mut text, thinking),
            ContentBlock::ToolCall {
                id: _,
                name,
                arguments,
                partial_json,
            } => {
                push_part(&mut text, name);
                push_part(&mut text, &arguments.to_string());
                if let Some(partial_json) = partial_json {
                    push_part(&mut text, partial_json);
                }
            }
            ContentBlock::Extension { type_name, data } => {
                push_part(&mut text, type_name);
                push_part(&mut text, &data.to_string());
            }
            _ => {}
        }
    }
    text
}

fn push_part(text: &mut String, part: &str) {
    if part.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(part);
}

/// Public-crate wrapper used by the tantivy index module.
#[cfg(feature = "search")]
pub(crate) fn snippet_from_text(text: &str, match_idx: usize) -> String {
    snippet(text, match_idx)
}

fn snippet(text: &str, match_idx: usize) -> String {
    if text.len() <= SNIPPET_CONTEXT_BYTES * 2 {
        return text.to_string();
    }

    let start = floor_char_boundary(text, match_idx.saturating_sub(SNIPPET_CONTEXT_BYTES));
    let end = floor_char_boundary(text, (match_idx + SNIPPET_CONTEXT_BYTES).min(text.len()));

    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(text[start..end].trim());
    if end < text.len() {
        snippet.push_str("...");
    }
    snippet
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
