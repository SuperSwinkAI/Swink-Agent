//! Parsing for `@path` file mentions.
//!
//! This module knows how to *find* mentions in prompt text. It deliberately
//! knows nothing about the filesystem: which paths exist, what the working
//! directory is, and what the ignore rules are all belong to the host (see
//! [`TuiExtensions::with_path_completions`](crate::TuiExtensions::with_path_completions)
//! and
//! [`TuiExtensions::with_mention_resolver`](crate::TuiExtensions::with_mention_resolver)).
//!
//! A mention is an `@` that starts the text or follows whitespace, plus the
//! run of non-whitespace characters after it. Requiring whitespace before the
//! `@` is what keeps `user@example.com` from parsing as a mention of
//! `example.com`.

/// Trailing characters trimmed off a mention.
///
/// These read as sentence punctuation rather than path characters, so
/// `see @src/lib.rs.` mentions `src/lib.rs`. Only *trailing* characters are
/// trimmed, so the `.` in `src/lib.rs` survives.
const TRAILING_PUNCTUATION: &[char] = &[',', ';', ':', '!', '?', ')', ']', '}', '.'];

/// A `@path` mention located in prompt text.
///
/// `start`/`end` are byte offsets into the text the mention was parsed from,
/// so a host can splice replacements in without re-scanning:
/// `&text[mention.start..mention.end]` is the mention including its `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMention {
    /// The mentioned path, without the leading `@` and without trailing
    /// sentence punctuation. Interpreted by the host — the TUI never resolves
    /// it against a filesystem.
    pub path: String,
    /// Byte offset of the `@`.
    pub start: usize,
    /// Byte offset one past the end of the mention.
    pub end: usize,
}

/// Find every `@path` mention in `text`, in source order.
///
/// # Example
/// ```rust
/// # use swink_agent_tui::parse_mentions;
/// let mentions = parse_mentions("compare @src/lib.rs with @src/main.rs please");
/// let paths: Vec<&str> = mentions.iter().map(|m| m.path.as_str()).collect();
/// assert_eq!(paths, ["src/lib.rs", "src/main.rs"]);
///
/// // An email address is not a mention: the `@` does not follow whitespace.
/// assert!(parse_mentions("mail me at a@b.com").is_empty());
/// ```
#[must_use]
pub fn parse_mentions(text: &str) -> Vec<PathMention> {
    let mut mentions = Vec::new();

    for (start, ch) in text.char_indices() {
        if ch != '@' {
            continue;
        }

        // Must start the text or follow whitespace, so email addresses and
        // `foo@bar` identifiers do not parse as mentions.
        let follows_whitespace = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if !follows_whitespace {
            continue;
        }

        let rest = &text[start + ch.len_utf8()..];
        let token_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let path = rest[..token_len].trim_end_matches(TRAILING_PUNCTUATION);

        // A bare `@`, or `@@foo`, is not a path mention.
        if path.is_empty() || path.starts_with('@') {
            continue;
        }

        mentions.push(PathMention {
            path: path.to_string(),
            start,
            end: start + ch.len_utf8() + path.len(),
        });
    }

    mentions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<String> {
        parse_mentions(text).into_iter().map(|m| m.path).collect()
    }

    #[test]
    fn plain_text_has_no_mentions() {
        assert!(parse_mentions("just a normal prompt").is_empty());
    }

    #[test]
    fn mention_at_start_of_text_parses() {
        assert_eq!(paths("@src/lib.rs explain this"), ["src/lib.rs"]);
    }

    #[test]
    fn mention_after_whitespace_parses() {
        assert_eq!(paths("look at @src/lib.rs"), ["src/lib.rs"]);
    }

    #[test]
    fn multiple_mentions_parse_in_source_order() {
        assert_eq!(paths("@b.rs and @a.rs"), ["b.rs", "a.rs"]);
    }

    #[test]
    fn email_address_is_not_a_mention() {
        assert!(parse_mentions("ping wes@example.com about it").is_empty());
    }

    #[test]
    fn bare_at_sign_is_not_a_mention() {
        assert!(parse_mentions("what does @ mean").is_empty());
    }

    #[test]
    fn double_at_sign_is_not_a_mention() {
        assert!(parse_mentions("@@handle").is_empty());
    }

    #[test]
    fn trailing_sentence_punctuation_is_trimmed() {
        assert_eq!(paths("read @src/lib.rs."), ["src/lib.rs"]);
        assert_eq!(paths("read @src/lib.rs, then stop"), ["src/lib.rs"]);
        assert_eq!(paths("read @src/lib.rs?"), ["src/lib.rs"]);
    }

    #[test]
    fn interior_dots_survive_trimming() {
        assert_eq!(paths("@a.b.c.rs"), ["a.b.c.rs"]);
    }

    #[test]
    fn mention_spans_cover_the_at_sign_and_path() {
        let mentions = parse_mentions("see @src/lib.rs now");
        assert_eq!(mentions.len(), 1);
        let mention = &mentions[0];
        assert_eq!(
            &"see @src/lib.rs now"[mention.start..mention.end],
            "@src/lib.rs"
        );
    }

    #[test]
    fn spans_are_correct_after_multibyte_text() {
        let text = "héllo @src/lib.rs";
        let mentions = parse_mentions(text);
        assert_eq!(mentions.len(), 1);
        assert_eq!(&text[mentions[0].start..mentions[0].end], "@src/lib.rs");
    }

    #[test]
    fn mention_after_newline_parses() {
        assert_eq!(paths("line one\n@src/lib.rs"), ["src/lib.rs"]);
    }

    #[test]
    fn mention_with_multibyte_path_parses() {
        assert_eq!(paths("@src/café.rs"), ["src/café.rs"]);
    }

    #[test]
    fn nested_paths_parse_whole() {
        assert_eq!(paths("@a/b/c/d.rs"), ["a/b/c/d.rs"]);
    }
}
