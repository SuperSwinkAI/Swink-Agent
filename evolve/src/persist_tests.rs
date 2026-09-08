//! Tests for `persist`.
#![cfg(test)]

use super::*;

#[test]
fn manifest_entry_jsonl_roundtrip() {
    let entry = ManifestEntry {
        cycle_id: 1,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        target_component: "PromptSection(Persona)".to_string(),
        original_value: "You are a helpful assistant.".to_string(),
        mutated_value: "You must be a helpful assistant.".to_string(),
        strategy: "template".to_string(),
        baseline_score: 0.7,
        candidate_score: 0.82,
        verdict: "Accepted".to_string(),
        rejection_reason: None,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let restored: ManifestEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.cycle_id, entry.cycle_id);
    assert_eq!(restored.verdict, entry.verdict);
    // Exact JSON round-trip, not a computed value — bit-for-bit equality is the point.
    #[allow(clippy::float_cmp)]
    {
        assert_eq!(restored.candidate_score, entry.candidate_score);
    }
    assert_eq!(restored.rejection_reason, entry.rejection_reason);
}
