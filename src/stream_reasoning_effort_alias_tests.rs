//! Tests for `stream`.
#![cfg(test)]

use super::ReasoningEffort;

#[test]
fn every_xhigh_spelling_deserializes() {
    for wire in ["\"xhigh\"", "\"x_high\"", "\"extra_high\""] {
        let decoded: ReasoningEffort = serde_json::from_str(wire).expect(wire);
        assert_eq!(decoded, ReasoningEffort::XHigh, "spelling {wire}");
    }
}
