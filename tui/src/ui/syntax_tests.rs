//! Tests for `syntax`.
#![cfg(test)]

use super::*;
use crate::theme::{ColorMode, set_color_mode};

fn reset() {
    set_color_mode(ColorMode::Custom);
}

#[test]
fn highlight_recognized_language_returns_styled_lines() {
    reset();
    let code = "fn main() {\n    println!(\"hello\");\n}";
    let lines = highlight_code(code, "rust");
    assert!(!lines.is_empty());
    // Each line should have spans with colors (not just plain text)
    for line in &lines {
        assert!(
            !line.spans.is_empty(),
            "each line should have at least one span"
        );
    }
}

#[test]
fn highlight_unrecognized_language_returns_dim_lines() {
    reset();
    let code = "some code here\nmore code";
    let lines = highlight_code(code, "not_a_real_language_xyz");
    assert_eq!(lines.len(), 2);
    // Should have DIM modifier on the spans
    for line in &lines {
        assert!(!line.spans.is_empty());
        let style = line.spans[0].style;
        assert!(
            style.add_modifier.contains(Modifier::DIM),
            "unrecognized language should produce DIM text"
        );
    }
}

#[test]
fn highlight_empty_language_returns_dim_lines() {
    reset();
    let code = "plain code\nno language";
    let lines = highlight_code(code, "");
    assert_eq!(lines.len(), 2);
    for line in &lines {
        let style = line.spans[0].style;
        assert!(
            style.add_modifier.contains(Modifier::DIM),
            "empty language should produce DIM text"
        );
    }
}

#[test]
fn highlight_lines_have_two_space_indent() {
    reset();
    let code = "x = 1";
    let lines = highlight_code(code, "python");
    assert!(!lines.is_empty());
    // First span of each line should start with 2-space indent
    let first_span_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        first_span_text.starts_with("  "),
        "highlighted lines should have 2-space indent prefix"
    );
}

#[test]
fn monochrome_mode_skips_syntect() {
    set_color_mode(ColorMode::MonoWhite);
    let code = "fn main() {}";
    let lines = highlight_code(code, "rust");
    assert!(!lines.is_empty());
    // In monochrome mode, lines should have DIM modifier
    for line in &lines {
        let style = line.spans[0].style;
        assert!(
            style.add_modifier.contains(Modifier::DIM),
            "monochrome mode should produce DIM text"
        );
    }
    reset();
}
