//! Tests for `langfuse`.
#![cfg(test)]

use super::*;

#[test]
fn hash_to_span_id_is_deterministic() {
    let a = hash_to_span_id("obs-1");
    let b = hash_to_span_id("obs-1");
    assert_eq!(a, b);
    let c = hash_to_span_id("obs-2");
    assert_ne!(a, c);
}

#[test]
fn langfuse_provider_debug_redacts_auth_keys() {
    let provider = LangfuseTraceProvider::new("https://langfuse.example", "pk-secret", "sk-secret")
        .expect("provider builds");

    let debug = format!("{provider:?}");

    assert!(
        !debug.contains("pk-secret"),
        "Debug leaks Langfuse public key"
    );
    assert!(
        !debug.contains("sk-secret"),
        "Debug leaks Langfuse secret key"
    );
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("https://langfuse.example"));
}

#[test]
fn parse_rfc3339_roundtrips() {
    let t = parse_rfc3339("2026-04-23T10:00:00Z").expect("rfc3339");
    let later = parse_rfc3339("2026-04-23T10:00:01Z").expect("rfc3339");
    assert!(later > t);
}

#[test]
fn json_to_otel_value_maps_primitives() {
    assert!(matches!(
        json_to_otel_value(&serde_json::json!("hi")),
        Some(Value::String(_))
    ));
    assert!(matches!(
        json_to_otel_value(&serde_json::json!(true)),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        json_to_otel_value(&serde_json::json!(42)),
        Some(Value::I64(42))
    ));
    assert!(matches!(
        json_to_otel_value(&serde_json::json!(2.5_f64)),
        Some(Value::F64(_))
    ));
    assert!(json_to_otel_value(&serde_json::Value::Null).is_none());
}
