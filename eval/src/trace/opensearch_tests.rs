//! Tests for `opensearch`.
#![cfg(test)]

use super::*;

#[test]
fn hash_to_span_id_is_deterministic() {
    let a = hash_to_span_id("span-1");
    let b = hash_to_span_id("span-1");
    assert_eq!(a, b);
    assert_ne!(hash_to_span_id("span-1"), hash_to_span_id("span-2"));
}

#[test]
fn opensearch_provider_debug_redacts_bearer_token() {
    let provider = OpenSearchTraceProvider::new("https://search.example", "spans")
        .expect("provider builds")
        .with_bearer("os-secret-token");

    let debug = format!("{provider:?}");

    assert!(
        !debug.contains("os-secret-token"),
        "Debug leaks OpenSearch bearer"
    );
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("spans"));
}

#[test]
fn parse_rfc3339_handles_utc_suffix() {
    let t = parse_rfc3339("2026-04-23T10:00:00Z").expect("rfc3339");
    let later = parse_rfc3339("2026-04-23T10:00:01Z").expect("rfc3339");
    assert!(later > t);
}
