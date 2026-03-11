//! Tool execution panel — shows active and recently completed tool calls.

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Braille spinner frames for active tool display.
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A tracked tool execution.
#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub id: String,
    pub name: String,
    pub started_at: Instant,
    pub completed_at: Option<Instant>,
    pub is_error: bool,
}

/// Tool panel state.
pub struct ToolPanel {
    /// Currently executing tools.
    pub active: Vec<ToolExecution>,
    /// Recently completed tools.
    pub completed: Vec<ToolExecution>,
    /// Spinner frame counter.
    pub spinner_frame: usize,
}

impl ToolPanel {
    pub const fn new() -> Self {
        Self {
            active: Vec::new(),
            completed: Vec::new(),
            spinner_frame: 0,
        }
    }

    /// Add a new active tool execution.
    pub fn start_tool(&mut self, id: String, name: String) {
        self.active.push(ToolExecution {
            id,
            name,
            started_at: Instant::now(),
            completed_at: None,
            is_error: false,
        });
    }

    /// Mark a tool as completed, moving it from active to completed.
    pub fn end_tool(&mut self, id: &str, is_error: bool) {
        if let Some(pos) = self.active.iter().position(|t| t.id == id) {
            let mut tool = self.active.remove(pos);
            tool.completed_at = Some(Instant::now());
            tool.is_error = is_error;
            self.completed.push(tool);
        }
    }

    /// Advance the spinner and prune old completed tools (>3s).
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
        self.completed
            .retain(|t| t.completed_at.is_none_or(|at| at.elapsed().as_secs() < 3));
    }

    /// Whether there's anything to display.
    pub const fn is_visible(&self) -> bool {
        !self.active.is_empty() || !self.completed.is_empty()
    }

    /// Height needed for the panel.
    pub fn height(&self) -> u16 {
        if !self.is_visible() {
            return 0;
        }
        // 2 for borders + 1 per tool
        #[allow(clippy::cast_possible_truncation)]
        { (self.active.len() + self.completed.len() + 2).min(8) as u16 }
    }

    /// Render the tool panel.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Tools ")
            .border_style(Style::default().fg(Color::Yellow));

        let mut lines: Vec<Line> = Vec::new();

        // Active tools with spinner
        for tool in &self.active {
            let elapsed = tool.started_at.elapsed().as_secs();
            let spinner = SPINNER[self.spinner_frame % SPINNER.len()];
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    tool.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {elapsed}s"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]));
        }

        // Completed tools with check/cross
        for tool in &self.completed {
            let duration = tool
                .completed_at
                .map_or(0, |end| end.duration_since(tool.started_at).as_millis());
            let (icon, color) = if tool.is_error {
                ("✗", Color::Red)
            } else {
                ("✓", Color::Green)
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), Style::default().fg(color)),
                Span::styled(
                    tool.name.clone(),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("  {duration}ms"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]));
        }

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }
}
