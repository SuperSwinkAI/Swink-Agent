//! Tests for `ablation`.
#![cfg(test)]

use super::*;
use crate::diagnose::{CaseFailure, TargetComponent, WeakPoint};
use swink_agent_eval::Score;

fn context(max_candidates: usize) -> MutationContext<'static> {
    MutationContext {
        weak_point: WeakPoint {
            component: TargetComponent::FullPrompt,
            affected_cases: vec![CaseFailure {
                case_id: "c1".to_string(),
                evaluator_name: "response".to_string(),
                score: Score::new(0.2, 0.5),
                details: None,
            }],
            mean_score_gap: 0.3,
            severity: 0.3,
        },
        failing_traces: vec![],
        eval_criteria: "response quality".to_string(),
        seed: None,
        max_candidates,
        budget: None,
    }
}

#[test]
fn simplification_caps_at_50_words_when_first_sentence_is_longer() {
    // A 60-word run-on sentence with no sentence-ending punctuation until
    // the very end, so `extract_first_sentence` would otherwise return
    // all 60 words.
    let words: Vec<String> = (0..60).map(|i| format!("word{i}")).collect();
    let target = format!("{}.", words.join(" "));

    let strategy = Ablation::new();
    let candidates = strategy.mutate(&target, &context(10)).unwrap();

    let simplified = candidates
        .iter()
        .find(|c| !c.mutated_value.is_empty())
        .expect("expected a non-empty simplification candidate");
    assert_eq!(simplified.mutated_value.split_whitespace().count(), 50);
    assert!(simplified.mutated_value.starts_with("word0 word1"));
}

#[test]
fn simplification_prefers_first_sentence_when_shorter_than_cap() {
    let target = "Short sentence. Followed by a lot more unrelated text that goes on.";
    let strategy = Ablation::new();
    let candidates = strategy.mutate(target, &context(10)).unwrap();

    let simplified = candidates
        .iter()
        .find(|c| !c.mutated_value.is_empty())
        .expect("expected a non-empty simplification candidate");
    assert_eq!(simplified.mutated_value, "Short sentence.");
}
