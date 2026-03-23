//! Syntax highlighting for code blocks using syntect.

use std::sync::OnceLock;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::theme;
use crate::theme::ColorMode;

/// Cached syntax set.
fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Cached theme set.
fn theme_set() -> &'static ThemeSet {
    static TS: OnceLock<ThemeSet> = OnceLock::new();
    TS.get_or_init(ThemeSet::load_defaults)
}

/// Convert a syntect RGBA color to a ratatui Color.
const fn to_ratatui_color(c: syntect::highlighting::Color) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.r, c.g, c.b)
}

/// Highlight a code block with syntax highlighting.
///
/// Falls back to plain dimmed text if the language isn't recognized.
/// In monochrome mode, skips syntect entirely and renders plain DIM text.
pub fn highlight_code(code: &str, language: &str) -> Vec<Line<'static>> {
    // Monochrome: skip syntect, render plain text with DIM + mono color
    if theme::color_mode() != ColorMode::Custom {
        let mono = theme::user_color();
        return code
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(mono).add_modifier(Modifier::DIM),
                ))
            })
            .collect();
    }

    let ss = syntax_set();
    let ts = theme_set();

    let syntax = if language.is_empty() {
        None
    } else {
        ss.find_syntax_by_token(language)
    };

    let Some(syntax) = syntax else {
        // Fallback: plain code with dim styling
        return code
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default()
                        .fg(theme::border_focused_color())
                        .add_modifier(Modifier::DIM),
                ))
            })
            .collect();
    };

    let syntect_theme = &ts.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, syntect_theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = highlighter.highlight_line(line, ss) else {
            lines.push(Line::from(Span::styled(
                format!("  {}", line.trim_end()),
                Style::default()
                    .fg(theme::border_focused_color())
                    .add_modifier(Modifier::DIM),
            )));
            continue;
        };

        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")]; // indent
        for (style, text) in ranges {
            let mut ratatui_style = Style::default().fg(to_ratatui_color(style.foreground));
            if style.font_style.contains(FontStyle::BOLD) {
                ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
            }
            if style.font_style.contains(FontStyle::ITALIC) {
                ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
            }
            spans.push(Span::styled(
                text.trim_end_matches('\n').to_string(),
                ratatui_style,
            ));
        }
        lines.push(Line::from(spans));
    }

    lines
}

#[cfg(test)]
mod tests {
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
            assert!(!line.spans.is_empty(), "each line should have at least one span");
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
}
