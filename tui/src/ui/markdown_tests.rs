//! Tests for `markdown`.
#![cfg(test)]

use super::*;
use ratatui::style::Color;

// --- parse_inline ---

#[test]
fn parse_inline_plain_text() {
    let spans = parse_inline("hello world", Style::default());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "hello world");
}

#[test]
fn parse_inline_backtick_code() {
    let spans = parse_inline("use `foo` here", Style::default());
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].content.as_ref(), "use ");
    assert_eq!(spans[1].content.as_ref(), "foo");
    assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(spans[1].style.fg, Some(Color::Yellow));
    assert_eq!(spans[2].content.as_ref(), " here");
}

#[test]
fn parse_inline_bold() {
    let spans = parse_inline("a **bold** b", Style::default());
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[1].content.as_ref(), "bold");
    assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn parse_inline_italic() {
    let spans = parse_inline("a *italic* b", Style::default());
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[1].content.as_ref(), "italic");
    assert!(spans[1].style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn parse_inline_empty_string() {
    let spans = parse_inline("", Style::default());
    assert!(spans.is_empty());
}

#[test]
fn parse_inline_only_code() {
    let spans = parse_inline("`code`", Style::default());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "code");
}

// --- split_preserving_spaces ---

#[test]
fn split_preserving_spaces_basic() {
    let result = split_preserving_spaces("hello world");
    assert_eq!(result, vec!["hello", " world"]);
}

#[test]
fn split_preserving_spaces_multiple_spaces() {
    let result = split_preserving_spaces("a  b");
    // "a", " ", " b"
    assert_eq!(result, vec!["a", " ", " b"]);
}

#[test]
fn split_preserving_spaces_empty() {
    let result = split_preserving_spaces("");
    assert!(result.is_empty());
}

#[test]
fn split_preserving_spaces_single_word() {
    let result = split_preserving_spaces("hello");
    assert_eq!(result, vec!["hello"]);
}

#[test]
fn split_preserving_spaces_leading_space() {
    let result = split_preserving_spaces(" hello");
    assert_eq!(result, vec![" hello"]);
}

// --- wrap_spans ---

#[test]
fn wrap_spans_zero_width_returns_single_line() {
    let spans = vec![Span::raw("hello world")];
    let lines = wrap_spans(spans, 0);
    assert_eq!(lines.len(), 1);
}

#[test]
fn wrap_spans_fits_in_width() {
    let spans = vec![Span::raw("short")];
    let lines = wrap_spans(spans, 80);
    assert_eq!(lines.len(), 1);
}

#[test]
fn wrap_spans_wraps_long_text() {
    let spans = vec![Span::raw("hello world foo bar")];
    let lines = wrap_spans(spans, 12);
    assert!(lines.len() > 1, "should wrap into multiple lines");
}

// --- markdown_to_lines (integration) ---

#[test]
fn markdown_to_lines_empty_input() {
    let lines = markdown_to_lines("", 80);
    assert!(lines.is_empty());
}

#[test]
fn markdown_to_lines_plain_paragraph() {
    let lines = markdown_to_lines("Hello world.", 80);
    assert_eq!(lines.len(), 1);
}

#[test]
fn markdown_to_lines_header_levels() {
    let lines = markdown_to_lines("# H1\n## H2\n### H3", 80);
    assert_eq!(lines.len(), 3);
    // H1 should have underline modifier
    let h1_span = &lines[0].spans[0];
    assert!(h1_span.style.add_modifier.contains(Modifier::UNDERLINED));
    assert!(h1_span.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn markdown_to_lines_bullet_list() {
    let lines = markdown_to_lines("- item one\n- item two", 80);
    assert_eq!(lines.len(), 2);
    // The bullet character should appear somewhere in the first line's spans
    let full_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        full_text.contains('\u{2022}'),
        "should contain bullet character, got: {full_text:?}"
    );
}

#[test]
fn markdown_to_lines_numbered_list() {
    let lines = markdown_to_lines("1. first\n2. second", 80);
    assert_eq!(lines.len(), 2);
    let full_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        full_text.contains("1."),
        "should contain '1.', got: {full_text:?}"
    );
}

#[test]
fn markdown_to_lines_empty_line_preserved() {
    let lines = markdown_to_lines("a\n\nb", 80);
    assert_eq!(lines.len(), 3);
    assert!(lines[1].spans.is_empty() || lines[1].spans[0].content.as_ref().is_empty());
}

#[test]
fn markdown_to_lines_code_block() {
    let input = "```rust\nlet x = 1;\n```";
    let lines = markdown_to_lines(input, 80);
    // Should produce at least one line from the code block
    assert!(!lines.is_empty());
}

#[test]
fn markdown_to_lines_unclosed_code_block_flushed() {
    // Simulates streaming where code block hasn't closed yet
    let input = "```python\nprint('hello')";
    let lines = markdown_to_lines(input, 80);
    assert!(
        !lines.is_empty(),
        "unclosed code block should still produce output"
    );
}
