//! Tests for `usage`.
#![cfg(test)]

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
    app.usage.total_input_tokens = turns.iter().map(|t| t.input_tokens).sum();
    app.usage.total_output_tokens = turns.iter().map(|t| t.output_tokens).sum();
    app.usage.total_cost = turns.iter().map(|t| t.cost).sum();
    app.usage.turn_usage = turns;
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
    let message = swink_agent::AssistantMessage::new(vec![], "anthropic", "claude-sonnet-4-6")
        .with_usage(
            swink_agent::Usage::default()
                .with_input(10)
                .with_output(20)
                .with_cache_read(30)
                .with_cache_write(40),
        )
        .with_cost(swink_agent::Cost::default().with_total(1.25))
        .with_stop_reason(swink_agent::StopReason::Stop)
        .with_timestamp(0);
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
