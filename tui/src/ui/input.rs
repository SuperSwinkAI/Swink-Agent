//! Multi-line input editor widget.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme;

/// An in-progress sigil token (`@path` mention or leading `/skill`) sitting
/// under the cursor.
///
/// Produced by [`InputEditor::mention_query`] / [`InputEditor::slash_query`]
/// and consumed by [`InputEditor::replace_mention_query`]. The name predates
/// the `/skill` use — see [`InputEditor::slash_query`] for why it is shared.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionQuery {
    /// Byte offset of the sigil (`@` or `/`) within the cursor's line.
    pub start: usize,
    /// Text between the sigil and the cursor. Empty right after the sigil is
    /// typed.
    pub query: String,
}

impl MentionQuery {
    /// Build a query for the sigil token starting at `start` in the line.
    #[must_use]
    pub fn new(start: usize, query: impl Into<String>) -> Self {
        Self {
            start,
            query: query.into(),
        }
    }
}

/// Multi-line input editor state.
pub struct InputEditor {
    /// Lines of text in the editor.
    lines: Vec<String>,
    /// Current cursor row (0-indexed).
    cursor_row: usize,
    /// Current cursor column (0-indexed).
    cursor_col: usize,
    /// Scroll offset for when content exceeds visible area.
    scroll_offset: usize,
    /// Input history for Up/Down recall.
    history: Vec<Vec<String>>,
    /// Current index into history (None = editing new input).
    history_index: Option<usize>,
    /// Saved in-progress input when browsing history.
    saved_input: Option<Vec<String>>,
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            saved_input: None,
        }
    }

    /// Current cursor row (0-indexed).
    pub const fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    /// Number of lines in the editor.
    pub const fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Read-only access to the lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Clamp cursor to valid position within current lines.
    fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len().saturating_sub(1);
        }
        let char_count = self.lines[self.cursor_row].chars().count();
        if self.cursor_col > char_count {
            self.cursor_col = char_count;
        }
    }

    /// Char count of the current line.
    fn line_char_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
    }

    /// Convert a char index to a byte offset in the given line.
    fn char_to_byte(line: &str, char_idx: usize) -> usize {
        line.char_indices()
            .nth(char_idx)
            .map_or(line.len(), |(byte_idx, _)| byte_idx)
    }

    /// Convert a byte offset to a char index.
    fn byte_to_char(line: &str) -> usize {
        line.chars().count()
    }

    /// Get the dynamic height for the input area.
    /// Grows with content from 3 to a max of 10 lines.
    pub fn height(&self) -> u16 {
        let content_height = self.lines.len() + 2; // +2 for borders
        #[allow(clippy::cast_possible_truncation)]
        {
            content_height.clamp(3, 10) as u16
        }
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        let char_count = self.line_char_len();
        if self.cursor_col > char_count {
            self.cursor_col = char_count;
        }
        let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
        self.lines[self.cursor_row].insert(byte_idx, c);
        self.cursor_col += 1;
    }

    /// Insert a newline at the cursor position (Shift+Enter).
    pub fn insert_newline(&mut self) {
        let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
        let remainder = self.lines[self.cursor_row][byte_idx..].to_string();
        self.lines[self.cursor_row].truncate(byte_idx);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, remainder);
        self.cursor_col = 0;
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        self.clamp_cursor();
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
            // Find the byte range of the char at this position
            if let Some(ch) = self.lines[self.cursor_row][byte_idx..].chars().next() {
                self.lines[self.cursor_row].replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
            }
        } else if self.cursor_row > 0 {
            // Merge with previous line
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        self.clamp_cursor();
        let char_count = self.line_char_len();
        if self.cursor_col < char_count {
            let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
            if let Some(ch) = self.lines[self.cursor_row][byte_idx..].chars().next() {
                self.lines[self.cursor_row].replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
            }
        } else if self.cursor_row + 1 < self.lines.len() {
            // Merge with next line
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let char_count = self.line_char_len();
        if self.cursor_col < char_count {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let char_count = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.cursor_col = self.cursor_col.min(char_count);
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            let char_count = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.cursor_col = self.cursor_col.min(char_count);
        }
    }

    /// Move cursor to start of line (Home / Ctrl+A).
    pub const fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to end of line (End / Ctrl+E).
    pub fn move_end(&mut self) {
        self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
    }

    /// Submit the current input, returning the full text. Clears the editor
    /// and pushes the submitted lines onto the history buffer.
    ///
    /// Returns `None` if the input is empty/whitespace.
    pub fn submit(&mut self) -> Option<String> {
        self.submit_inner(true)
    }

    /// Submit the current input WITHOUT persisting the lines to history.
    ///
    /// Used for submissions that contain user-supplied secrets (e.g.
    /// `#key <provider> <api-key>`) so that the value cannot be recalled
    /// via history navigation. Returns `None` if the input is empty.
    pub fn submit_without_history(&mut self) -> Option<String> {
        self.submit_inner(false)
    }

    fn submit_inner(&mut self, push_to_history: bool) -> Option<String> {
        let text: String = self.lines.join("\n");
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        if push_to_history {
            // Save to history
            self.history.push(self.lines.clone());
        }
        // Reset editor
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
        self.history_index = None;
        self.saved_input = None;
        Some(trimmed)
    }

    /// Navigate to previous history entry.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                // Save current input and go to most recent history
                self.saved_input = Some(self.lines.clone());
                let idx = self.history.len() - 1;
                self.history_index = Some(idx);
                self.lines = self.history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                self.lines = self.history[new_idx].clone();
            }
            _ => return,
        }
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
    }

    /// Navigate to next history entry.
    pub fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.lines = self.history[new_idx].clone();
            } else {
                // Restore saved input
                self.history_index = None;
                if let Some(saved) = self.saved_input.take() {
                    self.lines = saved;
                } else {
                    self.lines = vec![String::new()];
                }
            }
            self.cursor_row = self.lines.len().saturating_sub(1);
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
        }
    }

    /// The `@path` mention the cursor is currently inside, if any.
    ///
    /// Returns `None` unless the cursor sits at the end of an unbroken `@`
    /// token: the `@` must start the line or follow whitespace, and no
    /// whitespace may fall between it and the cursor. That means moving the
    /// cursor away from a mention, or typing a space to finish one, closes the
    /// completion popup without any special-casing at the call site.
    #[must_use]
    pub fn mention_query(&self) -> Option<MentionQuery> {
        self.sigil_query('@', false)
    }

    /// The leading `/skill` invocation the cursor is currently inside, if any.
    ///
    /// Stricter than [`mention_query`](Self::mention_query): the `/` must be
    /// the first non-whitespace character of the **first** line (matching the
    /// command table's single-leading-sigil model), and the cursor must sit at
    /// the end of the unbroken token after it. A mid-sentence `/` is never a
    /// query. Note that `/usr/bin` at line start *does* produce a query — the
    /// popup simply closes when the host returns no candidates for it.
    ///
    /// Returns a [`MentionQuery`] even though nothing here is a mention: the
    /// type is just "sigil offset plus partial token", and renaming it (or
    /// adding a twin) would break the public API for zero structural gain. The
    /// `start` offset points at the `/`.
    #[must_use]
    pub fn slash_query(&self) -> Option<MentionQuery> {
        self.sigil_query('/', true)
    }

    /// Shared sigil-token scanner behind [`mention_query`](Self::mention_query)
    /// and [`slash_query`](Self::slash_query).
    ///
    /// Finds `sigil` before the cursor on the cursor's line with no whitespace
    /// between it and the cursor. With `leading_only`, the sigil must be the
    /// first non-whitespace character of the first line; otherwise it must
    /// start the line or follow whitespace (the `@` rule that keeps
    /// `user@example.com` from becoming a mention).
    fn sigil_query(&self, sigil: char, leading_only: bool) -> Option<MentionQuery> {
        if leading_only && self.cursor_row != 0 {
            return None;
        }
        let line = self.lines.get(self.cursor_row)?;
        let cursor_byte = Self::char_to_byte(line, self.cursor_col);
        let before = line.get(..cursor_byte)?;

        let start = if leading_only {
            let start = before.find(sigil)?;
            if !before[..start].chars().all(char::is_whitespace) {
                return None;
            }
            start
        } else {
            let start = before.rfind(sigil)?;
            if start > 0
                && !before[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
            {
                return None;
            }
            start
        };

        let query = &before[start + sigil.len_utf8()..];
        if query.chars().any(char::is_whitespace) {
            return None;
        }

        Some(MentionQuery::new(start, query))
    }

    /// Replace the sigil token running from `start` to the cursor.
    ///
    /// `start` is a [`MentionQuery::start`] offset and `replacement` is the
    /// full token text including its sigil (`@path` or `/skill`, as produced
    /// by the matching query method). The cursor lands at the end of the
    /// inserted text. A `start` that is no longer valid (stale offset, moved
    /// cursor) is ignored rather than panicking.
    pub fn replace_mention_query(&mut self, start: usize, replacement: &str) {
        let Some(line) = self.lines.get(self.cursor_row) else {
            return;
        };
        let cursor_byte = Self::char_to_byte(line, self.cursor_col);
        if start > cursor_byte || !line.is_char_boundary(start) {
            return;
        }

        self.lines[self.cursor_row].replace_range(start..cursor_byte, replacement);
        self.cursor_col = self.lines[self.cursor_row][..start + replacement.len()]
            .chars()
            .count();
    }

    /// Check if this is a multi-line input.
    pub const fn is_multiline(&self) -> bool {
        self.lines.len() > 1
    }

    /// True if the editor contains no text (all lines are empty).
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(String::is_empty)
    }

    /// Render the input editor.
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool, status_hint: &str) {
        let border_color = if focused {
            theme::assistant_color()
        } else {
            theme::border_color()
        };

        let title = if status_hint.is_empty() {
            " Message ".to_string()
        } else {
            format!(" Message ({status_hint}) ")
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color));

        // Build text lines with optional line numbers for multi-line
        let text_lines: Vec<Line> = if self.is_multiline() {
            self.lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{:>2} ", i + 1),
                            Style::default()
                                .fg(theme::border_color())
                                .add_modifier(Modifier::DIM),
                        ),
                        Span::raw(line.clone()),
                    ])
                })
                .collect()
        } else {
            self.lines.iter().map(|l| Line::from(l.clone())).collect()
        };

        let paragraph = Paragraph::new(text_lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);

        // Position cursor
        if focused {
            #[allow(clippy::cast_possible_truncation)]
            let gutter_width: u16 = if self.is_multiline() { 3 } else { 0 };
            let visible_row = self.cursor_row.saturating_sub(self.scroll_offset);
            #[allow(clippy::cast_possible_truncation)]
            let cursor_x = area.x + 1 + gutter_width + self.cursor_col as u16;
            #[allow(clippy::cast_possible_truncation)]
            let cursor_y = area.y + 1 + visible_row as u16;
            if cursor_y < area.y + area.height - 1 {
                frame.set_cursor_position((cursor_x.min(area.x + area.width - 2), cursor_y));
            }
        }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
