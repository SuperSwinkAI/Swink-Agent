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
mod tests {
    use super::*;

    #[test]
    fn new_editor_is_empty() {
        let editor = InputEditor::new();
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert!(editor.lines.iter().all(String::is_empty));
    }

    #[test]
    fn insert_char_at_start() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        assert_eq!(editor.lines, vec!["a".to_string()]);
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn insert_char_at_end() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        assert_eq!(editor.lines, vec!["ab".to_string()]);
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn backspace_at_start_does_nothing() {
        let mut editor = InputEditor::new();
        editor.backspace();
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn backspace_merges_lines() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        assert_eq!(editor.lines.len(), 2);
        // Move cursor to start of line 2
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.lines, vec!["ab".to_string()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn delete_at_end_does_nothing() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.delete();
        assert_eq!(editor.lines, vec!["a".to_string()]);
    }

    #[test]
    fn delete_merges_with_next_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // Move cursor to end of first line
        editor.cursor_row = 0;
        editor.cursor_col = 1;
        editor.delete();
        assert_eq!(editor.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn insert_newline_splits_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        // Move cursor between a and b
        editor.cursor_col = 1;
        editor.insert_newline();
        assert_eq!(editor.lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(editor.cursor_row, 1);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn move_left_at_start_wraps_to_previous_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        // Cursor is at (1, 0)
        editor.move_left();
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 1); // end of "a"
    }

    #[test]
    fn move_right_at_end_wraps_to_next_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // Move to end of first line
        editor.cursor_row = 0;
        editor.cursor_col = 1;
        editor.move_right();
        assert_eq!(editor.cursor_row, 1);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn move_up_at_top_stays() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.move_up();
        assert_eq!(editor.cursor_row, 0);
    }

    #[test]
    fn move_down_at_bottom_stays() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.move_down();
        assert_eq!(editor.cursor_row, 0);
    }

    #[test]
    fn history_prev_and_next() {
        let mut editor = InputEditor::new();
        // Submit "first"
        editor.insert_char('f');
        editor.insert_char('i');
        editor.insert_char('r');
        editor.insert_char('s');
        editor.insert_char('t');
        editor.submit();
        // Submit "second"
        editor.insert_char('s');
        editor.insert_char('e');
        editor.insert_char('c');
        editor.insert_char('o');
        editor.insert_char('n');
        editor.insert_char('d');
        editor.submit();
        // Navigate backwards
        editor.history_prev();
        assert_eq!(editor.lines, vec!["second".to_string()]);
        editor.history_prev();
        assert_eq!(editor.lines, vec!["first".to_string()]);
        // Navigate forward
        editor.history_next();
        assert_eq!(editor.lines, vec!["second".to_string()]);
        editor.history_next();
        // Should restore to empty (saved input)
        assert_eq!(editor.lines, vec![String::new()]);
    }

    fn editor_with(text: &str) -> InputEditor {
        let mut editor = InputEditor::new();
        for ch in text.chars() {
            if ch == '\n' {
                editor.insert_newline();
            } else {
                editor.insert_char(ch);
            }
        }
        editor
    }

    #[test]
    fn no_mention_query_in_plain_text() {
        assert!(editor_with("hello world").mention_query().is_none());
    }

    #[test]
    fn mention_query_is_empty_right_after_the_at_sign() {
        let query = editor_with("look at @").mention_query().unwrap();
        assert_eq!(query.query, "");
        assert_eq!(query.start, 8);
    }

    #[test]
    fn mention_query_grows_as_the_path_is_typed() {
        assert_eq!(
            editor_with("@src/li").mention_query().unwrap().query,
            "src/li"
        );
    }

    #[test]
    fn whitespace_after_the_mention_closes_the_query() {
        assert!(editor_with("@src/lib.rs ").mention_query().is_none());
    }

    #[test]
    fn at_sign_inside_a_word_is_not_a_mention_query() {
        assert!(editor_with("wes@example").mention_query().is_none());
    }

    #[test]
    fn mention_query_tracks_the_cursor_not_the_line_end() {
        let mut editor = editor_with("@src/lib.rs");
        editor.move_left();
        editor.move_left();
        // Cursor sits between "." and "rs" — the query is the prefix only.
        assert_eq!(editor.mention_query().unwrap().query, "src/lib.");
    }

    #[test]
    fn mention_query_found_on_a_later_line() {
        let editor = editor_with("first line\n@src/li");
        let query = editor.mention_query().unwrap();
        assert_eq!(query.query, "src/li");
        assert_eq!(query.start, 0);
    }

    #[test]
    fn replace_mention_query_swaps_in_the_accepted_path() {
        let mut editor = editor_with("look at @src/li");
        let start = editor.mention_query().unwrap().start;
        editor.replace_mention_query(start, "@src/lib.rs ");
        assert_eq!(editor.lines(), ["look at @src/lib.rs "]);
    }

    #[test]
    fn replace_mention_query_leaves_the_cursor_after_the_insertion() {
        let mut editor = editor_with("@src/li");
        editor.replace_mention_query(0, "@src/lib.rs ");
        assert_eq!(editor.cursor_col, 12);
        editor.insert_char('x');
        assert_eq!(editor.lines(), ["@src/lib.rs x"]);
    }

    #[test]
    fn replace_mention_query_preserves_text_after_the_cursor() {
        let mut editor = editor_with("@src/li tail");
        for _ in 0..5 {
            editor.move_left();
        }
        let start = editor.mention_query().unwrap().start;
        editor.replace_mention_query(start, "@src/lib.rs");
        assert_eq!(editor.lines(), ["@src/lib.rs tail"]);
    }

    #[test]
    fn replace_mention_query_ignores_a_stale_start_offset() {
        let mut editor = editor_with("@a");
        editor.replace_mention_query(99, "@should-not-apply");
        assert_eq!(editor.lines(), ["@a"]);
    }

    #[test]
    fn replace_mention_query_handles_multibyte_prefixes() {
        let mut editor = editor_with("héllo @src/li");
        let start = editor.mention_query().unwrap().start;
        editor.replace_mention_query(start, "@src/lib.rs");
        assert_eq!(editor.lines(), ["héllo @src/lib.rs"]);
    }

    #[test]
    fn no_slash_query_in_plain_text() {
        assert!(editor_with("hello world").slash_query().is_none());
    }

    #[test]
    fn slash_query_is_empty_right_after_the_slash() {
        let query = editor_with("/").slash_query().unwrap();
        assert_eq!(query.query, "");
        assert_eq!(query.start, 0);
    }

    #[test]
    fn slash_query_grows_as_the_name_is_typed() {
        assert_eq!(editor_with("/depl").slash_query().unwrap().query, "depl");
    }

    #[test]
    fn slash_query_allows_leading_whitespace() {
        let query = editor_with("  /dep").slash_query().unwrap();
        assert_eq!(query.query, "dep");
        assert_eq!(query.start, 2);
    }

    #[test]
    fn whitespace_after_the_name_closes_the_slash_query() {
        assert!(editor_with("/deploy ").slash_query().is_none());
        assert!(editor_with("/deploy prod").slash_query().is_none());
    }

    #[test]
    fn a_mid_text_slash_is_not_a_slash_query() {
        assert!(editor_with("see /dep").slash_query().is_none());
        assert!(editor_with("either/or").slash_query().is_none());
    }

    #[test]
    fn a_path_at_line_start_does_produce_a_slash_query() {
        // The popup closes because the host returns no candidates for it —
        // the query itself is legitimate.
        assert_eq!(
            editor_with("/usr/bin").slash_query().unwrap().query,
            "usr/bin"
        );
    }

    #[test]
    fn a_slash_on_a_later_line_is_not_a_slash_query() {
        assert!(editor_with("first line\n/dep").slash_query().is_none());
    }

    #[test]
    fn slash_query_tracks_the_cursor_not_the_line_end() {
        let mut editor = editor_with("/deploy");
        editor.move_left();
        editor.move_left();
        // Cursor sits between "depl" and "oy" — the query is the prefix only.
        assert_eq!(editor.slash_query().unwrap().query, "depl");
    }

    #[test]
    fn slash_query_and_mention_query_are_mutually_exclusive() {
        // A leading slash token is not a mention...
        let slash = editor_with("/dep");
        assert!(slash.slash_query().is_some());
        assert!(slash.mention_query().is_none());

        // ...and a mention is not a leading slash token.
        let mention = editor_with("look at @src/li");
        assert!(mention.mention_query().is_some());
        assert!(mention.slash_query().is_none());

        // Even a mention typed after a leading command: the cursor is in the
        // mention, so only the mention query fires.
        let both = editor_with("/deploy @src/li");
        assert!(both.slash_query().is_none(), "whitespace closed the token");
        assert!(both.mention_query().is_some());
    }

    #[test]
    fn replace_mention_query_splices_an_accepted_skill() {
        let mut editor = editor_with("/dep");
        let start = editor.slash_query().unwrap().start;
        editor.replace_mention_query(start, "/deploy ");
        assert_eq!(editor.lines(), ["/deploy "]);
    }

    #[test]
    fn submit_clears_and_returns_text() {
        let mut editor = InputEditor::new();
        editor.insert_char('h');
        editor.insert_char('i');
        let result = editor.submit();
        assert_eq!(result, Some("hi".to_string()));
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert_eq!(editor.history.len(), 1);
    }

    #[test]
    fn submit_without_history_clears_but_skips_history() {
        let mut editor = InputEditor::new();
        for c in "#key openai sk-leak-sentinel".chars() {
            editor.insert_char(c);
        }
        let result = editor.submit_without_history();
        assert_eq!(result.as_deref(), Some("#key openai sk-leak-sentinel"));
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert!(
            editor.history.is_empty(),
            "submit_without_history must not push to history"
        );
    }

    #[test]
    fn submit_without_history_on_empty_returns_none() {
        let mut editor = InputEditor::new();
        assert_eq!(editor.submit_without_history(), None);
        assert!(editor.history.is_empty());
    }

    #[test]
    fn history_navigation_after_submit_without_history_is_empty() {
        let mut editor = InputEditor::new();
        for c in "#key openai sk-leak-sentinel-xyz".chars() {
            editor.insert_char(c);
        }
        let submitted = editor.submit_without_history();
        assert!(submitted.is_some());

        // History is empty, so navigating backwards must not recall the key.
        editor.history_prev();
        assert_eq!(
            editor.lines,
            vec![String::new()],
            "sensitive submission must not be recallable via history"
        );
        for line in &editor.lines {
            assert!(
                !line.contains("sk-leak-sentinel-xyz"),
                "secret value leaked into history: {line}"
            );
        }
    }

    #[test]
    fn multiline_sensitive_submission_does_not_enter_history() {
        let mut editor = InputEditor::new();
        editor.insert_char('p');
        editor.insert_char('r');
        editor.insert_char('e');
        editor.insert_newline();
        for c in "#key anthropic sk-ant-top-secret".chars() {
            editor.insert_char(c);
        }
        editor.insert_newline();
        editor.insert_char('p');
        editor.insert_char('o');
        editor.insert_char('s');
        editor.insert_char('t');
        let submitted = editor.submit_without_history();
        assert!(submitted.is_some());

        // Nothing should be recallable — the ENTIRE multi-line entry is
        // withheld, not just the key line.
        editor.history_prev();
        for line in &editor.lines {
            assert!(
                !line.contains("sk-ant-top-secret"),
                "multi-line sensitive entry leaked secret into history: {line}"
            );
        }
        assert_eq!(editor.lines, vec![String::new()]);
    }

    #[test]
    fn height_clamps_between_min_max() {
        let editor = InputEditor::new();
        // 1 line + 2 borders = 3, clamped to min 3
        assert_eq!(editor.height(), 3);

        let mut editor = InputEditor::new();
        // Add 20 lines to exceed max
        for _ in 0..20 {
            editor.insert_newline();
        }
        assert_eq!(editor.height(), 10);
    }

    #[test]
    fn backspace_with_cursor_past_end_clamps_instead_of_panic() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        // Artificially put cursor past end of line
        editor.cursor_col = 100;
        // Should not panic — clamp_cursor brings it in range
        editor.backspace();
        assert_eq!(editor.lines, vec!["a".to_string()]);
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn delete_with_cursor_past_end_clamps_instead_of_panic() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        // Artificially put cursor past end of line
        editor.cursor_col = 100;
        // Should not panic — clamps to end, then merges or no-ops
        editor.delete();
        // Cursor clamped to 1 (end of "a"), nothing to delete
        assert_eq!(editor.lines, vec!["a".to_string()]);
    }

    #[test]
    fn backspace_with_cursor_row_past_end_clamps() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        // Artificially put cursor on non-existent row
        editor.cursor_row = 50;
        editor.cursor_col = 10;
        // Should not panic
        editor.backspace();
        // Clamped to row 0, col clamped, then backspace operates normally
        assert!(!editor.lines.is_empty());
    }

    #[test]
    fn fields_are_private() {
        // This test documents that fields are not directly accessible
        // from outside the module. If this compiles, the struct API is correct.
        let editor = InputEditor::new();
        // Only public getters available:
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn insert_emoji_and_cursor_advances() {
        let mut editor = InputEditor::new();
        editor.insert_char('🎉');
        assert_eq!(editor.lines[0], "🎉");
        assert_eq!(editor.cursor_col, 1);
        editor.insert_char('x');
        assert_eq!(editor.lines[0], "🎉x");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn insert_cjk_characters() {
        let mut editor = InputEditor::new();
        editor.insert_char('你');
        editor.insert_char('好');
        assert_eq!(editor.lines[0], "你好");
        assert_eq!(editor.cursor_col, 2);
        editor.move_left();
        assert_eq!(editor.cursor_col, 1);
        editor.backspace();
        assert_eq!(editor.lines[0], "好");
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn insert_combining_characters() {
        let mut editor = InputEditor::new();
        // e followed by combining acute accent (two chars, one grapheme)
        editor.insert_char('e');
        editor.insert_char('\u{0301}');
        assert_eq!(editor.lines[0], "e\u{0301}");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn large_paste_does_not_panic() {
        let mut editor = InputEditor::new();
        let large_text: String = "a".repeat(10_000);
        for c in large_text.chars() {
            editor.insert_char(c);
        }
        assert_eq!(editor.lines[0].len(), 10_000);
        assert_eq!(editor.cursor_col, 10_000);
        // Verify submit works with large content
        let result = editor.submit();
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 10_000);
    }

    #[test]
    fn is_empty_true_for_new_editor() {
        let editor = InputEditor::new();
        assert!(editor.is_empty());
    }

    #[test]
    fn is_empty_false_after_insert() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        assert!(!editor.is_empty());
    }

    #[test]
    fn is_empty_false_with_blank_second_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        assert!(!editor.is_empty());
    }

    #[test]
    fn is_empty_true_after_submit_clears_editor() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.submit();
        assert!(editor.is_empty());
    }
}
