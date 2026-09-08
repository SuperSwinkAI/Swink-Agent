//! Tests for `mentions`.
#![cfg(test)]

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
