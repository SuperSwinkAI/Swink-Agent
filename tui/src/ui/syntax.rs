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
                    Style::default()
                        .fg(mono)
                        .add_modifier(Modifier::DIM),
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
