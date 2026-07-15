//! Per-turn usage accounting behind the status bar and the `/usage` command.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use swink_agent::AssistantMessage;

use super::state::{App, TurnUsage};
use crate::format;

impl TurnUsage {
    /// Record the usage and (already-priced) cost of one assistant response.
    ///
    /// By the time the TUI sees an [`AssistantMessage`], the agent loop has
    /// filled in `cost` from operator-declared rates or the model catalog — see
    /// [`swink_agent::price_assistant_message_with`]. The TUI never prices
    /// anything itself; it only totals what the loop reports.
    pub(super) fn from_message(message: &AssistantMessage) -> Self {
        Self {
            model_id: message.model_id.clone(),
            input_tokens: message.usage.input,
            output_tokens: message.usage.output,
            cache_read_tokens: message.usage.cache_read,
            cache_write_tokens: message.usage.cache_write,
            cost: message.cost.total,
        }
    }
}

/// Totals for one model across the session.
#[derive(Debug, Default, Clone, Copy)]
struct ModelTotals {
    turns: usize,
    input: u64,
    output: u64,
    cost: f64,
}

impl App {
    /// Render the `/usage` report: a per-turn breakdown, per-model subtotals,
    /// and session totals.
    ///
    /// Costs come from the agent loop, which prices each assistant message
    /// before the TUI ever sees it. A model with no catalog entry and no
    /// operator-declared `[pricing]` rates reports `$0.0000` — the report says
    /// so explicitly rather than implying the turns were free.
    pub(crate) fn usage_report(&self) -> String {
        if self.turn_usage.is_empty() {
            return "No usage recorded yet — send a prompt first.".to_string();
        }

        let mut out = String::new();
        let turns = self.turn_usage.len();
        let plural = if turns == 1 { "" } else { "s" };
        let _ = writeln!(out, "Usage — {turns} turn{plural}");
        out.push('\n');

        for (index, turn) in self.turn_usage.iter().enumerate() {
            let _ = writeln!(
                out,
                "  {:>3}  {:<28}  ↓{:>7} ↑{:>7}  ${:.4}",
                index + 1,
                truncate(&turn.model_id, 28),
                format::format_tokens(turn.input_tokens),
                format::format_tokens(turn.output_tokens),
                turn.cost,
            );
        }

        let by_model = self.usage_by_model();
        if by_model.len() > 1 {
            out.push_str("\nBy model\n");
            for (model_id, totals) in &by_model {
                let _ = writeln!(
                    out,
                    "  {:<28}  {:>3} turn(s)  ↓{:>7} ↑{:>7}  ${:.4}",
                    truncate(model_id, 28),
                    totals.turns,
                    format::format_tokens(totals.input),
                    format::format_tokens(totals.output),
                    totals.cost,
                );
            }
        }

        let cache_read: u64 = self.turn_usage.iter().map(|t| t.cache_read_tokens).sum();
        let cache_write: u64 = self.turn_usage.iter().map(|t| t.cache_write_tokens).sum();

        out.push('\n');
        let _ = writeln!(
            out,
            "  Total  ↓{} ↑{}  cache ↓{} ↑{}  ${:.4}",
            format::format_tokens(self.total_input_tokens),
            format::format_tokens(self.total_output_tokens),
            format::format_tokens(cache_read),
            format::format_tokens(cache_write),
            self.total_cost,
        );

        if self.total_cost == 0.0 {
            let unpriced: Vec<&str> = by_model
                .iter()
                .filter(|(_, totals)| totals.cost == 0.0)
                .map(|(model_id, _)| model_id.as_str())
                .collect();
            let _ = write!(
                out,
                "\n  No pricing for {}. Declare rates under [pricing] in tui.toml.",
                unpriced.join(", ")
            );
        }

        out
    }

    /// Session totals grouped by model ID, ordered by model ID for a stable
    /// report across renders.
    fn usage_by_model(&self) -> BTreeMap<String, ModelTotals> {
        let mut by_model: BTreeMap<String, ModelTotals> = BTreeMap::new();
        for turn in &self.turn_usage {
            let totals = by_model.entry(turn.model_id.clone()).or_default();
            totals.turns += 1;
            totals.input += turn.input_tokens;
            totals.output += turn.output_tokens;
            totals.cost += turn.cost;
        }
        by_model
    }
}

/// Truncate to `max` characters, marking the cut with an ellipsis.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TuiConfig;

    fn turn(model_id: &str, input: u64, output: u64, cost: f64) -> TurnUsage {
        TurnUsage {
            model_id: model_id.to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost,
        }
    }

    fn app_with(turns: Vec<TurnUsage>) -> App {
        let mut app = App::new(TuiConfig::default());
        app.total_input_tokens = turns.iter().map(|t| t.input_tokens).sum();
        app.total_output_tokens = turns.iter().map(|t| t.output_tokens).sum();
        app.total_cost = turns.iter().map(|t| t.cost).sum();
        app.turn_usage = turns;
        app
    }

    #[test]
    fn empty_report_says_so() {
        let report = App::new(TuiConfig::default()).usage_report();
        assert!(report.contains("No usage recorded yet"));
    }

    #[test]
    fn report_lists_one_line_per_turn() {
        let app = app_with(vec![
            turn("model-a", 100, 50, 0.01),
            turn("model-a", 200, 60, 0.02),
        ]);
        let report = app.usage_report();
        assert!(report.contains("Usage — 2 turns"), "{report}");
        assert!(report.contains("  1  model-a"), "{report}");
        assert!(report.contains("  2  model-a"), "{report}");
    }

    #[test]
    fn single_turn_is_not_pluralized() {
        let report = app_with(vec![turn("m", 1, 1, 0.0)]).usage_report();
        assert!(report.contains("Usage — 1 turn\n"), "{report}");
    }

    #[test]
    fn report_totals_tokens_and_cost() {
        let app = app_with(vec![
            turn("model-a", 100, 50, 0.01),
            turn("model-a", 200, 60, 0.02),
        ]);
        let report = app.usage_report();
        assert!(report.contains("↓300 ↑110"), "{report}");
        assert!(report.contains("$0.0300"), "{report}");
    }

    #[test]
    fn by_model_section_appears_only_for_multiple_models() {
        let single = app_with(vec![turn("model-a", 1, 1, 0.0)]).usage_report();
        assert!(!single.contains("By model"), "{single}");

        let multi = app_with(vec![
            turn("model-a", 1, 1, 0.01),
            turn("model-b", 1, 1, 0.02),
        ])
        .usage_report();
        assert!(multi.contains("By model"), "{multi}");
        assert!(multi.contains("model-a"), "{multi}");
        assert!(multi.contains("model-b"), "{multi}");
    }

    #[test]
    fn zero_cost_report_names_the_unpriced_models() {
        let report = app_with(vec![turn("my-local-llama", 1000, 500, 0.0)]).usage_report();
        assert!(report.contains("No pricing for my-local-llama"), "{report}");
        assert!(report.contains("[pricing]"), "{report}");
    }

    #[test]
    fn priced_report_omits_the_pricing_hint() {
        let report = app_with(vec![turn("model-a", 1000, 500, 0.5)]).usage_report();
        assert!(!report.contains("No pricing for"), "{report}");
    }

    #[test]
    fn turn_usage_is_built_from_the_loop_priced_message() {
        let message = swink_agent::AssistantMessage {
            content: vec![],
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4-6".to_string(),
            usage: swink_agent::Usage {
                input: 10,
                output: 20,
                cache_read: 30,
                cache_write: 40,
                ..swink_agent::Usage::default()
            },
            cost: swink_agent::Cost {
                total: 1.25,
                ..swink_agent::Cost::default()
            },
            stop_reason: swink_agent::StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        };
        let recorded = TurnUsage::from_message(&message);
        assert_eq!(recorded.model_id, "claude-sonnet-4-6");
        assert_eq!(recorded.input_tokens, 10);
        assert_eq!(recorded.output_tokens, 20);
        assert_eq!(recorded.cache_read_tokens, 30);
        assert_eq!(recorded.cache_write_tokens, 40);
        assert!((recorded.cost - 1.25).abs() < 1e-9);
    }

    /// Long model IDs are truncated in the table so columns stay aligned. The
    /// `[pricing]` hint deliberately prints the full ID — it is meant to be
    /// copied into a config file, so truncating it there would break it.
    #[test]
    fn long_model_ids_are_truncated_in_the_table_but_not_in_the_pricing_hint() {
        let long = "a".repeat(60);
        let report = app_with(vec![turn(&long, 1, 1, 0.0)]).usage_report();

        let table_line = report
            .lines()
            .find(|line| line.trim_start().starts_with('1'))
            .expect("a per-turn line");
        assert!(table_line.contains('…'), "{table_line}");
        assert!(!table_line.contains(&long), "{table_line}");

        let hint = report
            .lines()
            .find(|line| line.contains("No pricing for"))
            .expect("the pricing hint");
        assert!(hint.contains(&long), "{hint}");
    }

    #[test]
    fn truncate_leaves_short_text_alone() {
        assert_eq!(truncate("short", 28), "short");
    }
}
