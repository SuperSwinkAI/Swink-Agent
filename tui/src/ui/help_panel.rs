//! Help side panel toggled with F1.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme;

/// Fixed width of the help panel (including borders).
pub const HELP_PANEL_WIDTH: u16 = 34;

/// Minimum width the conversation area must retain when the help panel is visible.
pub const MIN_CONV_WIDTH: u16 = 40;

/// Side panel displaying key bindings and commands.
#[derive(Debug, Clone)]
pub struct HelpPanel {
    /// Whether the panel is currently visible.
    pub visible: bool,
}

impl HelpPanel {
    /// Create a new hidden help panel.
    #[must_use]
    pub const fn new() -> Self {
        Self { visible: false }
    }

    /// Toggle visibility.
    pub const fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Width consumed by the panel (0 when hidden).
    #[must_use]
    pub const fn width(&self) -> u16 {
        if self.visible { HELP_PANEL_WIDTH } else { 0 }
    }

    /// Render the help panel into the given area.
    #[allow(clippy::unused_self)]
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_focused_color()))
            .title(" Help (F1) ");

        let lines = help_lines();
        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }
}

/// Build the styled help content lines.
#[must_use]
pub fn help_lines() -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme::border_color());
    let normal = Style::default();

    let divider = Line::from(Span::styled(" ──────────────────────────────", dim));

    vec![
        Line::from(Span::styled(" Key Bindings", bold)),
        divider.clone(),
        key_line("Enter", "Submit", normal),
        key_line("Shift+Enter", "New line", normal),
        key_line("Ctrl+Q", "Quit", normal),
        key_line("Ctrl+C", "Quit / Abort", normal),
        key_line("Tab", "Toggle focus", normal),
        key_line("Shift+Tab", "Plan mode", normal),
        key_line("Up/Down", "Scroll / History", normal),
        key_line("PgUp/PgDn", "Page scroll", normal),
        key_line("Mouse wheel", "Chat scroll", normal),
        key_line("Click+drag", "Select / copy", normal),
        key_line("Esc", "Clear selection", normal),
        key_line("F1", "Toggle help", normal),
        key_line("F2", "Collapse tool", normal),
        key_line("F3", "Color mode", normal),
        key_line("F4", "Cycle model", normal),
        key_line("Shift+\u{2190}/\u{2192}", "Cycle tools", normal),
        Line::from(""),
        Line::from(Span::styled(" # Commands", bold)),
        divider.clone(),
        Line::from(Span::styled(" #clear #info #copy #copy all", normal)),
        Line::from(Span::styled(" #copy code #sessions #save", normal)),
        Line::from(Span::styled(" #load <id> #keys #key <p> <k>", normal)),
        Line::from(Span::styled(" #approve [on|off|smart]", normal)),
        Line::from(""),
        Line::from(Span::styled(" / Commands", bold)),
        divider,
        Line::from(Span::styled(" /quit /thinking /system", normal)),
        Line::from(Span::styled(" /reset /editor /plan", normal)),
    ]
}

/// Format a single key-binding line with aligned columns.
fn key_line<'a>(key: &'a str, desc: &'a str, style: Style) -> Line<'a> {
    Line::from(Span::styled(format!(" {key:<14}{desc}"), style))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_is_hidden() {
        let panel = HelpPanel::new();
        assert!(!panel.visible);
        assert_eq!(panel.width(), 0);
    }

    #[test]
    fn toggle_makes_visible() {
        let mut panel = HelpPanel::new();
        panel.toggle();
        assert!(panel.visible);
        assert_eq!(panel.width(), HELP_PANEL_WIDTH);
    }

    #[test]
    fn toggle_twice_hides() {
        let mut panel = HelpPanel::new();
        panel.toggle();
        panel.toggle();
        assert!(!panel.visible);
        assert_eq!(panel.width(), 0);
    }

    #[test]
    fn help_lines_not_empty() {
        let lines = help_lines();
        assert!(!lines.is_empty());
    }
}
