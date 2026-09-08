//! Tests for `stream`.
#![cfg(test)]

use super::*;

#[test]
fn default_serving_options_leave_reasoning_effort_unset() {
    let serving = ServingOptions::default();
    assert!(serving.reasoning_effort.is_none());
    assert!(serving.is_default());
}

#[test]
fn setting_reasoning_effort_is_no_longer_default() {
    let serving = ServingOptions::default().with_reasoning_effort(ReasoningEffort::High);
    assert_eq!(serving.reasoning_effort, Some(ReasoningEffort::High));
    assert!(!serving.is_default());
}

#[test]
fn unsupported_reasoning_effort_is_reported() {
    let serving = ServingOptions::default().with_reasoning_effort(ReasoningEffort::XHigh);
    let dropped = serving.unsupported_fields(ServingOptionSupport::none());
    assert_eq!(dropped, vec!["reasoning_effort"]);

    let honored =
        serving.unsupported_fields(ServingOptionSupport::none().with_reasoning_effort(true));
    assert!(honored.is_empty());
}

#[test]
fn all_and_none_cover_reasoning_effort() {
    assert!(ServingOptionSupport::all().reasoning_effort);
    assert!(!ServingOptionSupport::none().reasoning_effort);
}

#[test]
fn reasoning_effort_round_trips_through_serde_as_snake_case() {
    for (variant, wire) in [
        (ReasoningEffort::Off, "\"off\""),
        (ReasoningEffort::Minimal, "\"minimal\""),
        (ReasoningEffort::Low, "\"low\""),
        (ReasoningEffort::Medium, "\"medium\""),
        (ReasoningEffort::High, "\"high\""),
        (ReasoningEffort::XHigh, "\"xhigh\""),
        (ReasoningEffort::Max, "\"max\""),
    ] {
        let encoded = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(encoded, wire, "wire form for {variant:?}");
        let decoded: ReasoningEffort = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, variant);
    }
}

#[test]
fn absent_reasoning_effort_is_omitted_from_the_wire() {
    let json = serde_json::to_string(&ServingOptions::default()).expect("serialize");
    assert!(
        !json.contains("reasoning_effort"),
        "default must stay byte-identical: {json}"
    );
}
