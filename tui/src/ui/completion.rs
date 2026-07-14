//! `@path` completion popup, floated above the input editor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::PathCompletion;
use crate::theme;

/// Candidate rows shown at once before the list scrolls.
const MAX_VISIBLE_CANDIDATES: usize = 8;
/// Narrowest the popup is allowed to be, so short paths still read as a list.
const MIN_POPUP_WIDTH: u16 = 24;
/// Borders plus the gap between a path and its detail text.
const CHROME_WIDTH: u16 = 4;

/// Popup height, including borders, for `count` candidates.
fn popup_height(count: usize) -> u16 {
    let rows = count.clamp(1, MAX_VISIBLE_CANDIDATES);
    u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2)
}

/// Width needed to show the widest candidate without truncation.
fn popup_width(completion: &PathCompletion) -> u16 {
    let widest = completion
        .candidates
        .iter()
        .map(|candidate| {
            candidate.path.chars().count()
                + candidate
                    .detail
                    .as_ref()
                    .map_or(0, |detail| detail.chars().count() + 2)
        })
        .max()
        .unwrap_or(0);

    u16::try_from(widest)
        .unwrap_or(u16::MAX)
        .saturating_add(CHROME_WIDTH)
        .max(MIN_POPUP_WIDTH)
}

/// Render the popup directly above `input_area`.
///
/// Draws nothing when there are no candidates or when the input sits too close
/// to the top of the terminal for the popup to fit above it.
pub fn render(frame: &mut Frame, input_area: Rect, completion: &PathCompletion) {
    if completion.candidates.is_empty() {
        return;
    }

    let height = popup_height(completion.candidates.len());
    if input_area.y < height {
        return;
    }

    let area = Rect {
        x: input_area.x,
        y: input_area.y - height,
        width: popup_width(completion).min(input_area.width),
        height,
    };

    let items: Vec<ListItem> = completion
        .candidates
        .iter()
        .map(|candidate| {
            let mut spans = vec![Span::raw(candidate.path.clone())];
            if let Some(detail) = &candidate.detail {
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

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Files (Tab to insert) ")
                .border_style(Style::default().fg(theme::assistant_color())),
        )
        .highlight_style(
            Style::default()
                .fg(theme::assistant_color())
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        );

    // Clear first: the popup floats over the conversation view.
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        list,
        area,
        &mut ListState::default().with_selected(Some(completion.selected)),
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::extensions::PathCandidate;

    fn completion(paths: &[&str], selected: usize) -> PathCompletion {
        PathCompletion {
            candidates: paths.iter().map(|path| PathCandidate::new(*path)).collect(),
            selected,
            start: 0,
        }
    }

    /// Render the popup above a 3-row input at the bottom of a 20x10 terminal
    /// and return the rendered text, one string per row.
    fn render_rows(completion: &PathCompletion, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let input_area = Rect::new(0, height - 3, width, 3);
        terminal
            .draw(|frame| render(frame, input_area, completion))
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|row| {
                (0..width)
                    .map(|col| buffer[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect()
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
        assert_eq!(popup_height(1), 3);
        assert_eq!(popup_height(4), 6);
        assert_eq!(popup_height(MAX_VISIBLE_CANDIDATES), 10);
        assert_eq!(popup_height(500), 10, "long lists cap and scroll");
    }

    #[test]
    fn popup_height_of_an_empty_list_still_has_borders() {
        assert_eq!(popup_height(0), 3);
    }

    #[test]
    fn popup_width_has_a_floor_for_short_paths() {
        assert_eq!(popup_width(&completion(&["a"], 0)), MIN_POPUP_WIDTH);
    }

    #[test]
    fn popup_width_grows_for_long_paths() {
        let long = "a/very/deeply/nested/path/to/some/file.rs";
        let expected = u16::try_from(long.chars().count()).unwrap() + CHROME_WIDTH;
        assert_eq!(popup_width(&completion(&[long], 0)), expected);
    }

    #[test]
    fn popup_width_accounts_for_detail_text() {
        let mut state = completion(&["a.rs"], 0);
        state.candidates[0].detail = Some("some detail text here".to_string());
        assert!(popup_width(&state) > popup_width(&completion(&["a.rs"], 0)));
    }

    #[test]
    fn popup_never_renders_wider_than_the_input() {
        let long = "a/very/deeply/nested/path/that/exceeds/the/terminal/width.rs";
        let rows = render_rows(&completion(&[long], 0), 20, 12);
        assert!(rows.iter().all(|row| row.chars().count() == 20));
    }
}
