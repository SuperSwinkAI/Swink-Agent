//! Hand-rolled markdown to ratatui `Line` converter.
//!
//! Supports a useful subset of Markdown:
//! - Inline: **bold**, *italic*, `code`
//! - Block: ATX headers, fenced code blocks, bullet/numbered lists
//! - Word-wrapping to a given width

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::syntax;

/// Parse inline markdown within a single line of text, producing styled spans.
fn parse_inline(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut buf = String::new();

    while let Some(&(i, ch)) = chars.peek() {
        match ch {
            '`' => {
                // Flush buffer
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), base_style));
                }
                chars.next();
                // Collect until closing backtick
                let mut code = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c == '`' {
                        chars.next();
                        break;
                    }
                    code.push(c);
                    chars.next();
                }
                let code_style = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(code, code_style));
            }
            '*' => {
                // Check for ** (bold) vs * (italic)
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), base_style));
                }
                let rest = &text[i..];
                // Consume the first '*'
                chars.next();
                if rest.starts_with("**") {
                    // Consume the second '*'
                    chars.next();
                    let mut bold_text = String::new();
                    while let Some(&(j, c)) = chars.peek() {
                        if text[j..].starts_with("**") {
                            chars.next();
                            chars.next();
                            break;
                        }
                        bold_text.push(c);
                        chars.next();
                    }
                    spans.push(Span::styled(
                        bold_text,
                        base_style.add_modifier(Modifier::BOLD),
                    ));
                } else {
                    let mut italic_text = String::new();
                    while let Some(&(_, c)) = chars.peek() {
                        if c == '*' {
                            chars.next();
                            break;
                        }
                        italic_text.push(c);
                        chars.next();
                    }
                    spans.push(Span::styled(
                        italic_text,
                        base_style.add_modifier(Modifier::ITALIC),
                    ));
                }
            }
            _ => {
                buf.push(ch);
                chars.next();
            }
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, base_style));
    }
    spans
}

/// Simple word-wrap: split spans into lines that fit within `width` columns.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![spans];
    }

    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut col = 0;

    for span in spans {
        let style = span.style;
        let text = span.content.to_string();

        for word in split_preserving_spaces(&text) {
            let wlen = word.len();
            if col + wlen > width && col > 0 {
                lines.push(Vec::new());
                col = 0;
                // Skip leading space on new line
                let trimmed = word.trim_start();
                if !trimmed.is_empty() {
                    lines.last_mut().unwrap().push(Span::styled(trimmed.to_string(), style));
                    col = trimmed.len();
                }
            } else {
                lines.last_mut().unwrap().push(Span::styled(word.clone(), style));
                col += wlen;
            }
        }
    }

    lines
}

/// Split text into words preserving spaces attached to the following word.
fn split_preserving_spaces(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == ' ' {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            current.push(' ');
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Render markdown text into styled `Line`s for ratatui, word-wrapped to `width`.
///
/// This is a line-by-line state machine supporting headers, fenced code blocks,
/// bullet/numbered lists, and inline formatting.
pub fn markdown_to_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut output: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block — flush with syntax highlighting
                let code_text = code_buffer.join("\n");
                output.extend(syntax::highlight_code(&code_text, &code_lang));
                code_buffer.clear();
                in_code_block = false;
                code_lang.clear();
            } else {
                // Start code block
                in_code_block = true;
                code_lang = line.trim_start_matches('`').trim().to_string();
            }
            continue;
        }

        if in_code_block {
            code_buffer.push(line.to_string());
            continue;
        }

        let trimmed = line.trim();

        // ATX headers (check longer prefixes first)
        if let Some((header_text, extra)) = trimmed
            .strip_prefix("### ")
            .map(|t| (t, Modifier::empty()))
            .or_else(|| trimmed.strip_prefix("## ").map(|t| (t, Modifier::empty())))
            .or_else(|| {
                trimmed
                    .strip_prefix("# ")
                    .map(|t| (t, Modifier::UNDERLINED))
            })
        {
            let style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | extra);
            output.push(Line::from(Span::styled(header_text.to_string(), style)));
            continue;
        }

        // Bullet list
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let mut spans = vec![Span::styled("  • ".to_string(), Style::default().fg(Color::Cyan))];
            spans.extend(parse_inline(rest, Style::default()));
            for wrapped in wrap_spans(spans, width.saturating_sub(4)) {
                output.push(Line::from(wrapped));
            }
            continue;
        }

        // Numbered list
        if let Some(pos) = trimmed.find(". ") {
            let prefix = &trimmed[..pos];
            if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                let rest = &trimmed[pos + 2..];
                let mut spans = vec![Span::styled(
                    format!("  {prefix}. "),
                    Style::default().fg(Color::Cyan),
                )];
                spans.extend(parse_inline(rest, Style::default()));
                for wrapped in wrap_spans(spans, width.saturating_sub(4)) {
                    output.push(Line::from(wrapped));
                }
                continue;
            }
        }

        // Empty line
        if trimmed.is_empty() {
            output.push(Line::from(""));
            continue;
        }

        // Regular paragraph with inline formatting
        let spans = parse_inline(line, Style::default());
        for wrapped in wrap_spans(spans, width) {
            output.push(Line::from(wrapped));
        }
    }

    // Flush any unclosed code block (e.g. during streaming)
    if in_code_block && !code_buffer.is_empty() {
        let code_text = code_buffer.join("\n");
        output.extend(syntax::highlight_code(&code_text, &code_lang));
    }

    output
}
