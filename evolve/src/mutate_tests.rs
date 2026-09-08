//! Tests for `mutate`.
#![cfg(test)]

use super::*;

#[test]
fn candidate_id_is_deterministic() {
    let c1 = Candidate::new(
        TargetComponent::FullPrompt,
        "original".to_string(),
        "mutated text".to_string(),
        "test".to_string(),
    );
    let c2 = Candidate::new(
        TargetComponent::FullPrompt,
        "different original".to_string(),
        "mutated text".to_string(),
        "other".to_string(),
    );
    // Same mutated value → same id regardless of original or strategy
    assert_eq!(c1.id, c2.id);

    let c3 = Candidate::new(
        TargetComponent::FullPrompt,
        "original".to_string(),
        "different text".to_string(),
        "test".to_string(),
    );
    assert_ne!(c1.id, c3.id);
}

#[test]
fn deduplicate_removes_identity_and_duplicates() {
    let original = "original text";
    let candidates = vec![
        Candidate::new(
            TargetComponent::FullPrompt,
            original.into(),
            "mutated".into(),
            "a".into(),
        ),
        Candidate::new(
            TargetComponent::FullPrompt,
            original.into(),
            "mutated".into(),
            "b".into(),
        ),
        Candidate::new(
            TargetComponent::FullPrompt,
            original.into(),
            original.into(),
            "c".into(),
        ),
    ];
    let result = deduplicate(candidates, original);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].mutated_value, "mutated");
}
