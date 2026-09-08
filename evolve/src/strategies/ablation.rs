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
#[non_exhaustive]
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
#[path = "ablation_tests.rs"]
mod tests;
