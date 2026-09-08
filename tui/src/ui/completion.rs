//! Completion popups (`@path` files, `/skill` skills), floated above the
//! input editor.
//!
//! One generic renderer draws both: a titled list of rows — a primary span
//! plus an optional dimmed detail span — and, for skills, a clamped preview
//! block below the list showing the highlighted skill's documentation.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::{PathCompletion, SkillCompletion};
use crate::theme;

/// Candidate rows shown at once before the list scrolls.
const MAX_VISIBLE_CANDIDATES: usize = 8;
/// Narrowest the popup is allowed to be, so short entries still read as a list.
const MIN_POPUP_WIDTH: u16 = 24;
/// Borders plus the gap between a primary span and its detail text.
const CHROME_WIDTH: u16 = 4;
/// Preview lines shown below the list before the details are clamped.
const MAX_PREVIEW_LINES: usize = 6;

/// One popup row: what to render, independent of candidate type.
struct Row<'a> {
    /// Text inserted on accept, shown in the list.
    primary: &'a str,
    /// Optional dimmed text beside it (size, kind, one-line summary).
    detail: Option<&'a str>,
}

/// Popup height, including borders, for `count` candidates plus a preview.
fn popup_height(count: usize, preview_lines: usize) -> u16 {
    let rows = count.clamp(1, MAX_VISIBLE_CANDIDATES) + preview_lines;
    u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2)
}

/// Width needed to show the widest row without truncation.
fn popup_width(rows: &[Row<'_>]) -> u16 {
    let widest = rows
        .iter()
        .map(|row| {
            row.primary.chars().count() + row.detail.map_or(0, |detail| detail.chars().count() + 2)
        })
        .max()
        .unwrap_or(0);

    u16::try_from(widest)
        .unwrap_or(u16::MAX)
        .saturating_add(CHROME_WIDTH)
        .max(MIN_POPUP_WIDTH)
}

/// Render the `@path` popup directly above `input_area`.
///
/// Draws nothing when there are no candidates or when the input sits too close
/// to the top of the terminal for the popup to fit above it.
pub fn render(frame: &mut Frame, input_area: Rect, completion: &PathCompletion) {
    let rows: Vec<Row<'_>> = completion
        .candidates
        .iter()
        .map(|candidate| Row {
            primary: &candidate.path,
            detail: candidate.detail.as_deref(),
        })
        .collect();
    render_popup(
        frame,
        input_area,
        " Files (Tab to insert) ",
        &rows,
        completion.selected,
        None,
    );
}

/// Render the `/skill` popup directly above `input_area`.
///
/// Same geometry as [`render`], plus the highlighted skill's cached tier-2
/// documentation as a clamped preview block below the list.
pub fn render_skills(frame: &mut Frame, input_area: Rect, completion: &SkillCompletion) {
    let rows: Vec<Row<'_>> = completion
        .candidates
        .iter()
        .map(|candidate| Row {
            primary: &candidate.name,
            detail: candidate.description.as_deref(),
        })
        .collect();
    render_popup(
        frame,
        input_area,
        " Skills (Tab to insert) ",
        &rows,
        completion.selected,
        completion.selected_details(),
    );
}

/// Shared popup body: bordered title, candidate list, optional preview block.
fn render_popup(
    frame: &mut Frame,
    input_area: Rect,
    title: &str,
    rows: &[Row<'_>],
    selected: usize,
    preview: Option<&str>,
) {
    if rows.is_empty() {
        return;
    }

    let preview_lines: Vec<&str> = preview
        .map(|preview| preview.lines().take(MAX_PREVIEW_LINES).collect())
        .unwrap_or_default();

    let height = popup_height(rows.len(), preview_lines.len());
    if input_area.y < height {
        return;
    }

    let area = Rect {
        x: input_area.x,
        y: input_area.y - height,
        width: popup_width(rows).min(input_area.width),
        height,
    };

    // Clear first: the popup floats over the conversation view.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme::assistant_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let preview_height = u16::try_from(preview_lines.len()).unwrap_or(u16::MAX);
    let list_area = Rect {
        height: inner.height.saturating_sub(preview_height),
        ..inner
    };

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let mut spans = vec![Span::raw(row.primary.to_string())];
            if let Some(detail) = row.detail {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default()
                        .fg(theme::border_color())
                        .add_modifier(Modifier::DIM),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .fg(theme::assistant_color())
            .add_modifier(Modifier::REVERSED | Modifier::BOLD),
    );
    frame.render_stateful_widget(
        list,
        list_area,
        &mut ListState::default().with_selected(Some(selected)),
    );

    if !preview_lines.is_empty() {
        let preview_area = Rect {
            y: inner.y + list_area.height,
            height: preview_height.min(inner.height),
            ..inner
        };
        let lines: Vec<Line> = preview_lines
            .iter()
            .map(|line| {
                Line::from(Span::styled(
                    (*line).to_string(),
                    Style::default().add_modifier(Modifier::DIM),
                ))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), preview_area);
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
