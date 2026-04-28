use crate::mutate::{Candidate, MutationContext, MutationError, MutationStrategy};

/// Mutation strategy that tests prompt sections by removing or simplifying them.
///
/// Produces up to two candidates:
/// 1. Full removal — replaces the section with an empty string.
/// 2. First-sentence simplification — keeps only the first sentence.
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
        context: &MutationContext,
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

        // Candidate 2: first-sentence simplification
        let first_sentence = extract_first_sentence(target);
        if first_sentence != target {
            candidates.push(Candidate::new(
                component,
                target.to_string(),
                first_sentence,
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
