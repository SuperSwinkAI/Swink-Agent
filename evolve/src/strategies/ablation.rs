use crate::mutate::{Candidate, MutationContext, MutationError, MutationStrategy};

/// FR-011: the simplification candidate is capped at this many words when the
/// first sentence is longer.
const MAX_SIMPLIFIED_WORDS: usize = 50;

/// Mutation strategy that tests prompt sections by removing or simplifying them.
///
/// Produces up to two candidates:
/// 1. Full removal — replaces the section with an empty string.
/// 2. Simplification — keeps only the first sentence, or the first 50 words
///    (FR-011), whichever is shorter.
pub struct Ablation;

impl Ablation {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Ablation {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationStrategy for Ablation {
    fn name(&self) -> &str {
        "ablation"
    }

    fn mutate(
        &self,
        target: &str,
        context: &MutationContext<'_>,
    ) -> Result<Vec<Candidate>, MutationError> {
        let component = context.weak_point.component.clone();
        let mut candidates = Vec::new();

        // Candidate 1: full removal
        candidates.push(Candidate::new(
            component.clone(),
            target.to_string(),
            String::new(),
            "ablation".to_string(),
        ));

        // Candidate 2: first-sentence or first-50-words simplification,
        // whichever is shorter (FR-011).
        let simplified = simplify_to_sentence_or_word_cap(target, MAX_SIMPLIFIED_WORDS);
        if simplified != target {
            candidates.push(Candidate::new(
                component,
                target.to_string(),
                simplified,
                "ablation".to_string(),
            ));
        }

        candidates.truncate(context.max_candidates);
        Ok(candidates)
    }
}

/// Extracts text up to and including the first sentence-ending punctuation.
fn extract_first_sentence(text: &str) -> String {
    for (i, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            return text[..i + ch.len_utf8()].to_string();
        }
    }
    text.to_string()
}

/// Keeps only the first `max_words` whitespace-separated words of `text`.
fn truncate_to_words(text: &str, max_words: usize) -> String {
    text.split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
}

/// FR-011: simplify to the first sentence, or the first `max_words` words,
/// whichever is shorter (by word count).
fn simplify_to_sentence_or_word_cap(text: &str, max_words: usize) -> String {
    let sentence = extract_first_sentence(text);
    let word_capped = truncate_to_words(text, max_words);
    if sentence.split_whitespace().count() <= word_capped.split_whitespace().count() {
        sentence
    } else {
        word_capped
    }
}

#[cfg(test)]
mod tests {
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
}
