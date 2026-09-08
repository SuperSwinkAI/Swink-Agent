//! Tests for `meta`.
#![cfg(test)]

use super::*;

#[test]
fn serialization_roundtrip() {
    let meta = SessionMeta {
        id: "20250315_120000".to_string(),
        title: "Test session".to_string(),
        created_at: DateTime::from_timestamp(1_710_500_000, 0).unwrap().to_utc(),
        updated_at: DateTime::from_timestamp(1_710_500_100, 0).unwrap().to_utc(),
        version: 1,
        sequence: 0,
    };

    let json = serde_json::to_string(&meta).unwrap();
    let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized, meta);
}

#[test]
fn partial_eq_works() {
    let a = SessionMeta {
        id: "a".to_string(),
        title: "Session A".to_string(),
        created_at: DateTime::from_timestamp(100, 0).unwrap().to_utc(),
        updated_at: DateTime::from_timestamp(200, 0).unwrap().to_utc(),
        version: 1,
        sequence: 0,
    };
    let b = a.clone();
    assert_eq!(a, b);
}
