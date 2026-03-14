//! Tool execution panel — shows active and recently completed tool calls.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use swink_agent::redact_sensitive_values;

use crate::theme;

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

/// A tool call awaiting user approval.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub name: String,
    pub arguments_summary: String,
    id: String,
}

/// A recently resolved approval decision.
#[derive(Debug, Clone)]
pub struct ResolvedApproval {
    pub approved: bool,
    pub resolved_at: Instant,
}

/// Tool panel state.
pub struct ToolPanel {
    /// Currently executing tools.
    pub active: Vec<ToolExecution>,
    /// Recently completed tools.
    pub completed: Vec<ToolExecution>,
    /// Tools awaiting user approval.
    pub pending_approvals: Vec<PendingApproval>,
    /// Recently resolved approvals (shown briefly).
    pub resolved_approvals: Vec<ResolvedApproval>,
    /// Spinner frame counter.
    pub spinner_frame: usize,
}

impl ToolPanel {
    pub const fn new() -> Self {
        Self {
            active: Vec::new(),
            completed: Vec::new(),
            pending_approvals: Vec::new(),
            resolved_approvals: Vec::new(),
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

    /// Mark a tool as awaiting approval.
    pub fn set_awaiting_approval(&mut self, id: &str, name: &str, arguments: &Value) {
        let summary = summarize_arguments(arguments);
        self.pending_approvals.push(PendingApproval {
            id: id.to_string(),
            name: name.to_string(),
            arguments_summary: summary,
        });
    }

    /// Resolve a pending approval.
    pub fn resolve_approval(&mut self, id: &str, approved: bool) {
        self.pending_approvals.retain(|p| p.id != id);
        let _ = id; // id used for matching only
        self.resolved_approvals.push(ResolvedApproval {
            approved,
            resolved_at: Instant::now(),
        });
    }

    /// Whether there are tools pending user approval.
    #[allow(dead_code)]
    pub const fn has_pending_approval(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    /// Advance the spinner and prune old completed tools (>3s) and resolved approvals (>2s).
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
        self.completed
            .retain(|t| t.completed_at.is_none_or(|at| at.elapsed().as_secs() < 3));
        self.resolved_approvals
            .retain(|r| r.resolved_at.elapsed().as_secs() < 2);
    }

    /// Whether there's anything to display.
    pub const fn is_visible(&self) -> bool {
        !self.active.is_empty()
            || !self.completed.is_empty()
            || !self.pending_approvals.is_empty()
            || !self.resolved_approvals.is_empty()
    }

    /// Height needed for the panel.
    pub fn height(&self) -> u16 {
        if !self.is_visible() {
            return 0;
        }
        // 2 for borders + 1 per tool/approval entry
        let total = self.active.len()
            + self.completed.len()
            + self.pending_approvals.len()
            + self.resolved_approvals.len()
            + 2;
        #[allow(clippy::cast_possible_truncation)]
        {
            total.min(10) as u16
        }
    }

    /// Render the tool panel.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Tools ")
            .border_style(Style::default().fg(theme::tool_color()));

        let mut lines: Vec<Line> = Vec::new();

        // Pending approvals (highest priority — shown first)
        for approval in &self.pending_approvals {
            lines.push(Line::from(vec![
                Span::styled(
                    " \u{26a0} ",
                    Style::default()
                        .fg(theme::tool_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    approval.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(": {}", approval.arguments_summary),
                    Style::default().fg(theme::border_focused_color()),
                ),
                Span::styled(
                    " \u{2014} Approve? [Y/n]",
                    Style::default()
                        .fg(theme::tool_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Recently resolved approvals
        for resolved in &self.resolved_approvals {
            let (icon, label, color) = if resolved.approved {
                ("\u{2713}", "Approved", theme::success_color())
            } else {
                ("\u{2717}", "Rejected", theme::failure_color())
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), Style::default().fg(color)),
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::DIM),
                ),
            ]));
        }

        // Active tools with spinner
        for tool in &self.active {
            let elapsed = tool.started_at.elapsed().as_secs();
            let spinner = SPINNER[self.spinner_frame % SPINNER.len()];
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(theme::tool_color()),
                ),
                Span::styled(
                    tool.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {elapsed}s"),
                    Style::default()
                        .fg(theme::border_color())
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
                ("\u{2717}", theme::failure_color())
            } else {
                ("\u{2713}", theme::success_color())
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
                        .fg(theme::border_color())
                        .add_modifier(Modifier::DIM),
                ),
            ]));
        }

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }
}

/// Summarize tool arguments for display in the approval prompt.
///
/// Sensitive values are redacted before summarization so that secrets
/// never appear in the terminal UI.
fn summarize_arguments(args: &Value) -> String {
    let args = &redact_sensitive_values(args);
    match args {
        Value::Object(map) if map.len() == 1 => {
            if let Some((key, val)) = map.iter().next() {
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let truncated = if val_str.len() > 60 {
                    format!("{}...", &val_str[..57])
                } else {
                    val_str
                };
                return format!("{key}={truncated}");
            }
            String::new()
        }
        Value::Object(map) => {
            let keys: Vec<&String> = map.keys().take(3).collect();
            let summary = keys
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if map.len() > 3 {
                format!("{{{summary}, ...}}")
            } else {
                format!("{{{summary}}}")
            }
        }
        other => {
            let s = other.to_string();
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_tool_adds_to_active() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());
        assert_eq!(panel.active.len(), 1);
        assert_eq!(panel.active[0].id, "t1");
        assert_eq!(panel.active[0].name, "bash");
    }

    #[test]
    fn end_tool_moves_to_completed() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());
        panel.end_tool("t1", false);
        assert!(panel.active.is_empty());
        assert_eq!(panel.completed.len(), 1);
        assert_eq!(panel.completed[0].id, "t1");
        assert!(!panel.completed[0].is_error);
    }

    #[test]
    fn set_awaiting_approval_adds_to_pending() {
        let mut panel = ToolPanel::new();
        let args = serde_json::json!({"command": "rm -rf /"});
        panel.set_awaiting_approval("t1", "bash", &args);
        assert_eq!(panel.pending_approvals.len(), 1);
        assert_eq!(panel.pending_approvals[0].name, "bash");
    }

    #[test]
    fn resolve_approval_moves_to_resolved() {
        let mut panel = ToolPanel::new();
        let args = serde_json::json!({"command": "ls"});
        panel.set_awaiting_approval("t1", "bash", &args);
        panel.resolve_approval("t1", true);
        assert!(panel.pending_approvals.is_empty());
        assert_eq!(panel.resolved_approvals.len(), 1);
        assert!(panel.resolved_approvals[0].approved);
    }

    #[test]
    fn is_visible_when_has_active_tools() {
        let mut panel = ToolPanel::new();
        assert!(!panel.is_visible());
        panel.start_tool("t1".into(), "bash".into());
        assert!(panel.is_visible());
    }

    #[test]
    fn is_visible_when_has_completed_tools() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());
        panel.end_tool("t1", false);
        assert!(panel.is_visible());
    }

    #[test]
    fn not_visible_when_empty() {
        let panel = ToolPanel::new();
        assert!(!panel.is_visible());
    }

    #[test]
    fn height_capped_at_max() {
        let mut panel = ToolPanel::new();
        for i in 0..20 {
            panel.start_tool(format!("t{i}"), "tool".into());
        }
        assert!(panel.height() <= 10);
    }
}
