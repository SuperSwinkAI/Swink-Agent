//! Tests for `otlp`.
#![cfg(test)]

use super::*;

#[test]
fn decode_hex_16_round_trips() {
    let bytes: [u8; 16] = decode_hex("0123456789abcdef0123456789abcdef").unwrap();
    assert_eq!(
        bytes,
        [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ]
    );
}

#[test]
fn otlp_provider_debug_redacts_bearer_token() {
    let provider = OtlpHttpTraceProvider::new("https://otlp.example")
        .expect("provider builds")
        .with_bearer_token("otlp-secret-token");

    let debug = format!("{provider:?}");

    assert!(
        !debug.contains("otlp-secret-token"),
        "Debug leaks OTLP bearer"
    );
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("https://otlp.example"));
}

#[test]
fn decode_hex_rejects_oversize() {
    let r: Option<[u8; 8]> = decode_hex("0123456789abcdef00");
    assert!(r.is_none());
}

#[test]
fn decode_hex_rejects_non_hex() {
    let r: Option<[u8; 8]> = decode_hex("zzzz567890abcdef");
    assert!(r.is_none());
}

#[test]
fn nanos_to_systime_monotonic() {
    let a = nanos_to_systime(1_700_000_000_000_000_000);
    let b = nanos_to_systime(1_700_000_000_000_001_000);
    assert!(b > a);
}
