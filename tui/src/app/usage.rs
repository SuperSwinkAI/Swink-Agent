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
        Self::new(
            message.model_id.clone(),
            message.usage.input,
            message.usage.output,
            message.usage.cache_read,
            message.usage.cache_write,
            message.cost.total,
        )
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
        if self.usage.turn_usage.is_empty() {
            return "No usage recorded yet — send a prompt first.".to_string();
        }

        let mut out = String::new();
        let turns = self.usage.turn_usage.len();
        let plural = if turns == 1 { "" } else { "s" };
        let _ = writeln!(out, "Usage — {turns} turn{plural}");
        out.push('\n');

        for (index, turn) in self.usage.turn_usage.iter().enumerate() {
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

        let cache_read: u64 = self
            .usage
            .turn_usage
            .iter()
            .map(|t| t.cache_read_tokens)
            .sum();
        let cache_write: u64 = self
            .usage
            .turn_usage
            .iter()
            .map(|t| t.cache_write_tokens)
            .sum();

        out.push('\n');
        let _ = writeln!(
            out,
            "  Total  ↓{} ↑{}  cache ↓{} ↑{}  ${:.4}",
            format::format_tokens(self.usage.total_input_tokens),
            format::format_tokens(self.usage.total_output_tokens),
            format::format_tokens(cache_read),
            format::format_tokens(cache_write),
            self.usage.total_cost,
        );

        if self.usage.total_cost == 0.0 {
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
        for turn in &self.usage.turn_usage {
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
#[path = "usage_tests.rs"]
mod tests;
