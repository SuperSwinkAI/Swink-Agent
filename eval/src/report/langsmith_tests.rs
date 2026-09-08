//! Tests for `langsmith`.
#![cfg(test)]

use super::*;

#[test]
fn metric_details_prefers_structured_feedback_key_and_note() {
    let details = concat!(
        "{\"kind\":\"prompt_version\",\"version\":\"correctness_v0\"}\n",
        "{\"kind\":\"feedback_key\",\"key\":\"quality.correctness\"}\n",
        "{\"kind\":\"note\",\"text\":\"judge note\"}"
    );

    let parsed = MetricDetails::parse(Some(details));
    assert_eq!(parsed.feedback_key.as_deref(), Some("quality.correctness"));
    assert_eq!(parsed.comment.as_deref(), Some("judge note"));
}

#[test]
fn langsmith_exporter_debug_redacts_api_token() {
    let exporter = LangSmithExporter::new("ls-secret-token");

    let debug = format!("{exporter:?}");

    assert!(
        !debug.contains("ls-secret-token"),
        "Debug leaks LangSmith token"
    );
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("api.smith.langchain.com"));
}

#[test]
fn metric_details_falls_back_to_raw_text() {
    let parsed = MetricDetails::parse(Some("plain error text"));
    assert_eq!(parsed.feedback_key, None);
    assert_eq!(parsed.comment.as_deref(), Some("plain error text"));
}
