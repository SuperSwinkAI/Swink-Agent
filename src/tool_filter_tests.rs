//! Tests for `tool_filter`.
#![cfg(test)]

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
fn glob_star_backtracks_without_regex() {
    let pat = ToolPattern::parse("read_*_file");
    assert!(pat.matches("read_secret_file"));
    assert!(pat.matches("read_very_secret_file"));
    assert!(!pat.matches("read_secret_dir"));
}

#[test]
fn glob_handles_unicode_chars() {
    let pat = ToolPattern::parse("t?ol_*");
    assert!(pat.matches("t🦀ol_alpha"));
    assert!(!pat.matches("tool"));
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
    let filter = ToolFilter::new().with_allowed(vec![ToolPattern::parse("bash")]);
    assert!(filter.is_allowed("bash"));
    assert!(!filter.is_allowed("read_file"));
}

#[test]
fn rejected_only_excludes_matching() {
    let filter = ToolFilter::new().with_rejected(vec![ToolPattern::parse("bash")]);
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
    assert!(matches!(
        ToolPattern::parse("^regex$"),
        ToolPattern::Regex(_)
    ));
}
