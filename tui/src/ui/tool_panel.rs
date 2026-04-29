//! Tool execution panel — shows active and recently completed tool calls.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use swink_agent::{AgentToolResult, ContentBlock, redact_sensitive_values};

use crate::theme;

/// Braille spinner frames for active tool display.
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A tracked tool execution.
#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub id: String,
    pub name: String,
    pub streamed_output: String,
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
            streamed_output: String::new(),
            started_at: Instant::now(),
            completed_at: None,
            is_error: false,
        });
    }

    /// Append a partial output update for an active tool.
    pub fn update_tool(&mut self, id: &str, name: &str, partial: &AgentToolResult) {
        let update = ContentBlock::extract_text(&partial.content);
        if update.is_empty() {
            return;
        }

        let Some(tool) = self.active.iter_mut().find(|tool| tool.id == id) else {
            self.start_tool(id.to_string(), name.to_string());
            let tool = self
                .active
                .last_mut()
                .expect("start_tool should add the missing tool");
            tool.streamed_output = update;
            return;
        };

        if update.starts_with(&tool.streamed_output) {
            tool.streamed_output = update;
        } else {
            tool.streamed_output.push_str(&update);
        }
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
        self.resolved_approvals.push(ResolvedApproval {
            approved,
            resolved_at: Instant::now(),
        });
    }

    /// Advance the spinner and prune old completed tools (>10s) and resolved approvals (>2s).
    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
        self.completed
            .retain(|t| t.completed_at.is_none_or(|at| at.elapsed().as_secs() < 10));
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

    /// Render the tool panel (no extra prompts).
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_with_prompts(frame, area, false, None);
    }

    /// Render the tool panel with optional plan approval and trust follow-up prompts.
    #[allow(clippy::too_many_lines)]
    pub fn render_with_prompts(
        &self,
        frame: &mut Frame,
        area: Rect,
        pending_plan_approval: bool,
        trust_follow_up: Option<&str>,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Tools ")
            .border_style(Style::default().fg(theme::tool_color()));

        let mut lines: Vec<Line> = Vec::new();

        // Plan approval prompt (highest priority)
        if pending_plan_approval {
            lines.push(Line::from(vec![
                Span::styled(
                    " \u{26a0} ",
                    Style::default()
                        .fg(theme::plan_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Approve plan?",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " [Y/n]",
                    Style::default()
                        .fg(theme::plan_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Trust follow-up prompt
        if let Some(tool_name) = trust_follow_up {
            lines.push(Line::from(vec![
                Span::styled(
                    " \u{2713} ",
                    Style::default()
                        .fg(theme::tool_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("Always approve {tool_name}?"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " [y/N]",
                    Style::default()
                        .fg(theme::tool_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Pending tool approvals
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
            let preview = tool
                .streamed_output
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_else(|| tool.streamed_output.trim());
            let preview = truncate_preview(preview, 60);
            let mut spans = vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(theme::tool_color()),
                ),
                Span::styled(
                    tool.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            if !preview.is_empty() {
                spans.push(Span::styled(
                    format!(": {preview}"),
                    Style::default()
                        .fg(theme::border_focused_color())
                        .add_modifier(Modifier::DIM),
                ));
            }
            spans.push(Span::styled(
                format!("  {elapsed}s"),
                Style::default()
                    .fg(theme::border_color())
                    .add_modifier(Modifier::DIM),
            ));
            lines.push(Line::from(spans));
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
                let truncated = truncate_with_ellipsis(&val_str, 60);
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
            truncate_with_ellipsis(&s, 60)
        }
    }
}

fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().nth(max_chars).is_none() {
        return text.to_string();
    }

    let keep_chars = max_chars.saturating_sub(3);
    let keep_bytes = text
        .char_indices()
        .nth(keep_chars)
        .map_or(text.len(), |(index, _)| index);
    format!("{}...", &text[..keep_bytes])
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let preview: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        format!("{preview}...")
    } else {
        preview
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
        assert!(panel.active[0].streamed_output.is_empty());
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
    fn set_awaiting_approval_truncates_unicode_object_argument() {
        let mut panel = ToolPanel::new();
        let args = serde_json::json!({"command": "é".repeat(61)});

        panel.set_awaiting_approval("t1", "bash", &args);

        let summary = &panel.pending_approvals[0].arguments_summary;
        assert!(summary.starts_with("command="));
        assert!(summary.ends_with("..."));
        assert_eq!(summary.trim_start_matches("command=").chars().count(), 60);
    }

    #[test]
    fn summarize_arguments_truncates_unicode_non_object_argument() {
        let args = serde_json::json!("é".repeat(61));

        let summary = summarize_arguments(&args);

        assert!(summary.ends_with("..."));
        assert_eq!(summary.chars().count(), 60);
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
    fn end_tool_out_of_order_concurrent() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());
        panel.start_tool("t2".into(), "read_file".into());
        panel.start_tool("t3".into(), "write_file".into());

        // Complete in reverse order (t3, t1, t2)
        panel.end_tool("t3", false);
        assert_eq!(panel.active.len(), 2);
        assert_eq!(panel.completed.len(), 1);
        assert_eq!(panel.completed[0].id, "t3");
        assert_eq!(panel.completed[0].name, "write_file");

        panel.end_tool("t1", true);
        assert_eq!(panel.active.len(), 1);
        assert_eq!(panel.completed.len(), 2);
        assert_eq!(panel.completed[1].id, "t1");
        assert!(panel.completed[1].is_error);

        panel.end_tool("t2", false);
        assert!(panel.active.is_empty());
        assert_eq!(panel.completed.len(), 3);
        assert_eq!(panel.completed[2].id, "t2");
    }

    #[test]
    fn end_tool_unknown_id_is_noop() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());
        panel.end_tool("nonexistent", false);
        assert_eq!(panel.active.len(), 1);
        assert!(panel.completed.is_empty());
    }

    #[test]
    fn update_tool_accumulates_incremental_output() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());

        panel.update_tool("t1", "bash", &AgentToolResult::text("line 1\n"));
        panel.update_tool("t1", "bash", &AgentToolResult::text("line 2"));

        assert_eq!(panel.active[0].streamed_output, "line 1\nline 2");
    }

    #[test]
    fn update_tool_replaces_with_latest_snapshot() {
        let mut panel = ToolPanel::new();
        panel.start_tool("t1".into(), "bash".into());

        panel.update_tool("t1", "bash", &AgentToolResult::text("line 1"));
        panel.update_tool("t1", "bash", &AgentToolResult::text("line 1\nline 2"));

        assert_eq!(panel.active[0].streamed_output, "line 1\nline 2");
    }

    #[test]
    fn update_tool_registers_missing_active_tool() {
        let mut panel = ToolPanel::new();

        panel.update_tool("t1", "bash", &AgentToolResult::text("working"));

        assert_eq!(panel.active.len(), 1);
        assert_eq!(panel.active[0].name, "bash");
        assert_eq!(panel.active[0].streamed_output, "working");
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
