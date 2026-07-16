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
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::extensions::{PathCandidate, SkillCandidate};

    fn completion(paths: &[&str], selected: usize) -> PathCompletion {
        PathCompletion {
            candidates: paths.iter().map(|path| PathCandidate::new(*path)).collect(),
            selected,
            start: 0,
        }
    }

    fn skill_completion(names: &[&str], selected: usize) -> SkillCompletion {
        SkillCompletion {
            candidates: names
                .iter()
                .map(|name| SkillCandidate::new(*name))
                .collect(),
            selected,
            start: 0,
            details: std::collections::HashMap::new(),
        }
    }

    fn rows_of(completion: &PathCompletion) -> Vec<Row<'_>> {
        completion
            .candidates
            .iter()
            .map(|candidate| Row {
                primary: &candidate.path,
                detail: candidate.detail.as_deref(),
            })
            .collect()
    }

    /// Render a popup above a 3-row input at the bottom of the terminal and
    /// return the rendered text, one string per row.
    fn render_rows_with(draw: impl Fn(&mut Frame, Rect), width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let input_area = Rect::new(0, height - 3, width, 3);
        terminal.draw(|frame| draw(frame, input_area)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|row| {
                (0..width)
                    .map(|col| buffer[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn render_rows(completion: &PathCompletion, width: u16, height: u16) -> Vec<String> {
        render_rows_with(
            |frame, input_area| render(frame, input_area, completion),
            width,
            height,
        )
    }

    fn render_skill_rows(completion: &SkillCompletion, width: u16, height: u16) -> Vec<String> {
        render_rows_with(
            |frame, input_area| render_skills(frame, input_area, completion),
            width,
            height,
        )
    }

    #[test]
    fn candidate_paths_are_rendered() {
        let rows = render_rows(&completion(&["src/lib.rs", "src/main.rs"], 0), 40, 12);
        let rendered = rows.join("\n");
        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("src/main.rs"));
    }

    #[test]
    fn detail_text_is_rendered_beside_the_path() {
        let mut state = completion(&["src/lib.rs"], 0);
        state.candidates[0].detail = Some("4.2 kB".to_string());
        let rendered = render_rows(&state, 40, 12).join("\n");
        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("4.2 kB"));
    }

    #[test]
    fn empty_candidate_list_renders_nothing() {
        let state = PathCompletion {
            candidates: Vec::new(),
            selected: 0,
            start: 0,
        };
        let rendered = render_rows(&state, 40, 12).join("");
        assert!(rendered.trim().is_empty());
    }

    #[test]
    fn popup_is_skipped_when_it_cannot_fit_above_the_input() {
        // 4 rows total, input occupies the bottom 3 — only one row above.
        let rendered = render_rows(&completion(&["src/lib.rs"], 0), 40, 4).join("");
        assert!(rendered.trim().is_empty());
    }

    #[test]
    fn popup_sits_directly_above_the_input() {
        let rows = render_rows(&completion(&["a.rs"], 0), 40, 12);
        // Input occupies rows 9..12, popup is 3 tall, so it lands on rows 6..9.
        assert!(rows[6].contains('┌'), "top border on row 6: {:?}", rows[6]);
        assert!(
            rows[7].contains("a.rs"),
            "candidate on row 7: {:?}",
            rows[7]
        );
        assert!(
            rows[8].contains('└'),
            "bottom border on row 8: {:?}",
            rows[8]
        );
        assert!(rows[5].trim().is_empty(), "row 5 untouched: {:?}", rows[5]);
    }

    #[test]
    fn popup_height_grows_with_candidates_then_caps() {
        assert_eq!(popup_height(1, 0), 3);
        assert_eq!(popup_height(4, 0), 6);
        assert_eq!(popup_height(MAX_VISIBLE_CANDIDATES, 0), 10);
        assert_eq!(popup_height(500, 0), 10, "long lists cap and scroll");
    }

    #[test]
    fn popup_height_of_an_empty_list_still_has_borders() {
        assert_eq!(popup_height(0, 0), 3);
    }

    #[test]
    fn popup_width_has_a_floor_for_short_paths() {
        assert_eq!(
            popup_width(&rows_of(&completion(&["a"], 0))),
            MIN_POPUP_WIDTH
        );
    }

    #[test]
    fn popup_width_grows_for_long_paths() {
        let long = "a/very/deeply/nested/path/to/some/file.rs";
        let expected = u16::try_from(long.chars().count()).unwrap() + CHROME_WIDTH;
        assert_eq!(popup_width(&rows_of(&completion(&[long], 0))), expected);
    }

    #[test]
    fn popup_width_accounts_for_detail_text() {
        let mut state = completion(&["a.rs"], 0);
        state.candidates[0].detail = Some("some detail text here".to_string());
        assert!(popup_width(&rows_of(&state)) > popup_width(&rows_of(&completion(&["a.rs"], 0))));
    }

    #[test]
    fn popup_never_renders_wider_than_the_input() {
        let long = "a/very/deeply/nested/path/that/exceeds/the/terminal/width.rs";
        let rows = render_rows(&completion(&[long], 0), 20, 12);
        assert!(rows.iter().all(|row| row.chars().count() == 20));
    }

    // ─── skills popup ─────────────────────────────────────────────────────

    #[test]
    fn skill_names_and_descriptions_are_rendered() {
        let mut state = skill_completion(&["deploy", "review"], 0);
        state.candidates[0].description = Some("Ship a release".to_string());
        let rendered = render_skill_rows(&state, 60, 12).join("\n");
        assert!(rendered.contains("deploy"));
        assert!(rendered.contains("Ship a release"));
        assert!(rendered.contains("review"));
        assert!(rendered.contains("Skills"));
    }

    #[test]
    fn cached_details_render_as_a_preview_below_the_list() {
        let mut state = skill_completion(&["deploy"], 0);
        state
            .details
            .insert("deploy".to_string(), Some("step one\nstep two".to_string()));

        let rows = render_skill_rows(&state, 60, 14);
        let rendered = rows.join("\n");
        assert!(rendered.contains("step one"));
        assert!(rendered.contains("step two"));

        // The preview sits below the candidate row, inside the same popup.
        let deploy_row = rows.iter().position(|row| row.contains("deploy")).unwrap();
        let preview_row = rows
            .iter()
            .position(|row| row.contains("step one"))
            .unwrap();
        let bottom_border = rows.iter().position(|row| row.contains('└')).unwrap();
        assert!(deploy_row < preview_row, "preview below the list");
        assert!(preview_row < bottom_border, "preview inside the popup");
    }

    #[test]
    fn a_preview_grows_the_popup_height() {
        assert_eq!(popup_height(1, 0), 3);
        assert_eq!(popup_height(1, 2), 5);
        assert_eq!(popup_height(1, MAX_PREVIEW_LINES), 9);
    }

    #[test]
    fn preview_lines_are_clamped() {
        let mut state = skill_completion(&["deploy"], 0);
        let body = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.details.insert("deploy".to_string(), Some(body));

        let rendered = render_skill_rows(&state, 60, 20).join("\n");
        assert!(rendered.contains("line 1"));
        assert!(rendered.contains(&format!("line {MAX_PREVIEW_LINES}")));
        assert!(!rendered.contains(&format!("line {}", MAX_PREVIEW_LINES + 1)));
    }

    #[test]
    fn a_skill_popup_that_cannot_fit_with_its_preview_is_skipped() {
        // Without the preview a 1-candidate popup (3 rows) fits above a 3-row
        // input in a 7-row terminal; with a preview it no longer does.
        let mut state = skill_completion(&["deploy"], 0);
        let fits = render_skill_rows(&state, 40, 7).join("");
        assert!(fits.contains("deploy"));

        state
            .details
            .insert("deploy".to_string(), Some("one\ntwo\nthree".to_string()));
        let skipped = render_skill_rows(&state, 40, 7).join("");
        assert!(skipped.trim().is_empty());
    }

    #[test]
    fn details_of_unselected_candidates_are_not_previewed() {
        let mut state = skill_completion(&["deploy", "review"], 0);
        state
            .details
            .insert("review".to_string(), Some("review body".to_string()));
        let rendered = render_skill_rows(&state, 60, 14).join("\n");
        assert!(!rendered.contains("review body"));
    }

    #[test]
    fn skill_popup_never_renders_wider_than_the_input() {
        let mut state = skill_completion(&["deploy"], 0);
        state.candidates[0].description =
            Some("a very long description that exceeds the terminal width".to_string());
        state.details.insert(
            "deploy".to_string(),
            Some("a very long preview line that also exceeds the width".to_string()),
        );
        let rows = render_skill_rows(&state, 20, 14);
        assert!(rows.iter().all(|row| row.chars().count() == 20));
    }
}
