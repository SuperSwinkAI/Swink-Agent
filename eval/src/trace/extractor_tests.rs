//! Tests for `extractor`.
#![cfg(test)]

use super::*;

#[test]
fn evaluation_level_serde_round_trip() {
    let yaml_like = serde_json::to_string(&EvaluationLevel::Trace).unwrap();
    assert_eq!(yaml_like, "\"trace\"");
    let back: EvaluationLevel = serde_json::from_str(&yaml_like).unwrap();
    assert_eq!(back, EvaluationLevel::Trace);
}

#[test]
fn extracted_input_level_matches_variant() {
    let call = RecordedToolCall {
        id: "id".into(),
        name: "n".into(),
        arguments: serde_json::Value::Null,
    };
    assert_eq!(
        ExtractedInput::Tool {
            turn_index: 0,
            call
        }
        .level(),
        EvaluationLevel::Tool
    );
    assert_eq!(
        ExtractedInput::Session { turns: vec![] }.level(),
        EvaluationLevel::Session
    );
}
