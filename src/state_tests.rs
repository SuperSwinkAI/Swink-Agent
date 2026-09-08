//! Tests for `state`.
#![cfg(test)]

use super::*;
use serde_json::json;

// ── StateDelta ──

#[test]
fn delta_default_is_empty() {
    let d = StateDelta::default();
    assert!(d.is_empty());
    assert_eq!(d.len(), 0);
}

#[test]
fn delta_serde_roundtrip() {
    let mut d = StateDelta::default();
    d.changes.insert("a".into(), Some(json!(1)));
    d.changes.insert("b".into(), None);
    let json = serde_json::to_string(&d).unwrap();
    let d2: StateDelta = serde_json::from_str(&json).unwrap();
    assert_eq!(d2.len(), 2);
    assert_eq!(d2.changes["a"], Some(json!(1)));
    assert_eq!(d2.changes["b"], None);
}

#[test]
fn delta_new_is_empty() {
    assert!(StateDelta::new().is_empty());
}

#[test]
fn delta_with_change_chains() {
    let d = StateDelta::new()
        .with_change("a", Some(json!(1)))
        .with_change("b", None);
    assert_eq!(d.len(), 2);
    assert_eq!(d.changes["a"], Some(json!(1)));
    assert_eq!(d.changes["b"], None);
}

#[test]
fn delta_from_changes_takes_map() {
    let d = StateDelta::from_changes(HashMap::from([
        ("a".to_string(), Some(json!(1))),
        ("b".to_string(), None),
    ]));
    assert_eq!(d.len(), 2);
    assert_eq!(d.changes["a"], Some(json!(1)));
    assert_eq!(d.changes["b"], None);
}

// ── SessionState get/set/remove ──

#[test]
fn set_and_get_typed() {
    let mut s = SessionState::new();
    s.set("count", 42_i64).unwrap();
    assert_eq!(s.get::<i64>("count"), Some(42));
}

#[test]
fn get_raw_returns_value_ref() {
    let mut s = SessionState::new();
    s.set("key", "hello").unwrap();
    assert_eq!(s.get_raw("key"), Some(&json!("hello")));
}

#[test]
fn get_missing_returns_none() {
    let s = SessionState::new();
    assert_eq!(s.get::<String>("nope"), None);
}

#[test]
fn get_wrong_type_returns_none() {
    let mut s = SessionState::new();
    s.set("key", "hello").unwrap();
    // Try to get as i64 — should fail gracefully
    assert_eq!(s.get::<i64>("key"), None);
    // Original value still intact
    assert_eq!(s.get::<String>("key"), Some("hello".to_string()));
}

#[test]
fn remove_existing_key() {
    let mut s = SessionState::new();
    s.set("x", 1).unwrap();
    s.remove("x");
    assert!(!s.contains("x"));
    assert!(s.is_empty());
}

#[test]
fn remove_absent_key_is_noop() {
    let mut s = SessionState::new();
    s.remove("nope");
    assert!(s.delta().is_empty());
}

#[test]
fn contains_keys_len_is_empty() {
    let mut s = SessionState::new();
    assert!(s.is_empty());
    s.set("a", 1).unwrap();
    s.set("b", 2).unwrap();
    assert!(s.contains("a"));
    assert!(!s.contains("c"));
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
    let keys: Vec<&str> = s.keys().collect();
    assert!(keys.contains(&"a"));
    assert!(keys.contains(&"b"));
}

#[test]
fn clear_records_all_removals() {
    let mut s = SessionState::new();
    s.set("a", 1).unwrap();
    s.set("b", 2).unwrap();
    s.flush_delta(); // reset
    s.clear();
    assert!(s.is_empty());
    assert_eq!(s.delta().len(), 2);
    assert_eq!(s.delta().changes["a"], None);
    assert_eq!(s.delta().changes["b"], None);
}

// ── Delta collapse ──

#[test]
fn delta_set_set_last_wins() {
    let mut s = SessionState::new();
    s.set("k", 1).unwrap();
    s.set("k", 2).unwrap();
    assert_eq!(s.delta().changes["k"], Some(json!(2)));
    assert_eq!(s.delta().len(), 1);
}

#[test]
fn delta_set_remove_is_none() {
    let mut s = SessionState::new();
    s.set("k", 1).unwrap();
    s.remove("k");
    assert_eq!(s.delta().changes["k"], None);
}

#[test]
fn delta_remove_set_is_some() {
    let mut s = SessionState::with_data(std::iter::once(("k".to_string(), json!(1))).collect());
    s.remove("k");
    s.set("k", 99).unwrap();
    assert_eq!(s.delta().changes["k"], Some(json!(99)));
}

// ── flush_delta ──

#[test]
fn flush_delta_returns_and_resets() {
    let mut s = SessionState::new();
    s.set("a", 1).unwrap();
    let d = s.flush_delta();
    assert_eq!(d.len(), 1);
    assert!(s.delta().is_empty());
}

#[test]
fn flush_empty_delta_returns_empty() {
    let mut s = SessionState::new();
    let d = s.flush_delta();
    assert!(d.is_empty());
}

// ── with_data (baseline semantics) ──

#[test]
fn with_data_pre_seeds_without_delta() {
    let data: HashMap<String, Value> = std::iter::once(("x".into(), json!(42))).collect();
    let s = SessionState::with_data(data);
    assert_eq!(s.get::<i64>("x"), Some(42));
    assert!(s.delta().is_empty());
}

// ── apply_baseline (baseline underlies, existing wins) ──

#[test]
fn apply_baseline_inserts_missing_keys_without_delta() {
    let mut s = SessionState::new();
    s.set("mine", "kept").unwrap();
    s.flush_delta(); // reset so only apply_baseline effects are observed

    let baseline = SessionState::with_data(
        [
            ("mine".to_string(), json!("overridden?")),
            ("extra".to_string(), json!("from baseline")),
        ]
        .into_iter()
        .collect(),
    );
    s.apply_baseline(&baseline);

    // Missing key filled in from the baseline.
    assert_eq!(s.get::<String>("extra"), Some("from baseline".to_string()));
    // Existing entry wins over the baseline.
    assert_eq!(s.get::<String>("mine"), Some("kept".to_string()));
    // Baseline inserts are NOT recorded in the delta.
    assert!(s.delta().is_empty());
}

#[test]
fn apply_baseline_into_empty_state_copies_all_keys() {
    let baseline = SessionState::with_data(
        [("a".to_string(), json!(1)), ("b".to_string(), json!("two"))]
            .into_iter()
            .collect(),
    );
    let mut s = SessionState::new();
    s.apply_baseline(&baseline);
    assert_eq!(s.get::<i64>("a"), Some(1));
    assert_eq!(s.get::<String>("b"), Some("two".to_string()));
    assert!(s.delta().is_empty());
}

// ── snapshot / restore ──

#[test]
fn snapshot_restore_roundtrip() {
    let mut s = SessionState::new();
    s.set("name", "alice").unwrap();
    s.set("age", 30).unwrap();
    let snap = s.snapshot();
    let s2 = SessionState::restore_from_snapshot(snap).unwrap();
    assert_eq!(s2.get::<String>("name"), Some("alice".to_string()));
    assert_eq!(s2.get::<i64>("age"), Some(30));
    assert!(s2.delta().is_empty());
}

// ── Serialize roundtrip (delta skipped) ──

#[test]
fn serde_roundtrip_skips_delta() {
    let mut s = SessionState::new();
    s.set("k", "v").unwrap();
    // Delta has an entry
    assert!(!s.delta().is_empty());
    let json = serde_json::to_string(&s).unwrap();
    let s2: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(s2.get::<String>("k"), Some("v".to_string()));
    // Delta is empty after deserialization (skipped)
    assert!(s2.delta().is_empty());
}

// ── Serialization error handling ──

#[test]
fn set_returns_error_on_serialization_failure() {
    use serde::ser::{self, Serializer};

    /// A type whose `Serialize` impl always fails.
    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(ser::Error::custom("intentional serialization failure"))
        }
    }

    let mut s = SessionState::new();
    let result = s.set("bad", Unserializable);
    assert!(result.is_err());
    // State must remain unchanged after a failed set.
    assert!(!s.contains("bad"));
    assert!(s.delta().is_empty());
}

// ── Nested JSON values ──

#[test]
fn nested_json_roundtrip() {
    let mut s = SessionState::new();
    let nested = json!({
        "user": {"name": "bob", "scores": [1, 2, 3]},
        "active": true
    });
    s.set("profile", nested.clone()).unwrap();
    let snap = s.snapshot();
    let s2 = SessionState::restore_from_snapshot(snap).unwrap();
    assert_eq!(s2.get_raw("profile"), Some(&nested));
}

#[test]
fn restore_from_corrupt_snapshot_returns_error() {
    let err = SessionState::restore_from_snapshot(json!(["not", "an", "object"])).unwrap_err();
    assert!(err.to_string().contains("map"));
}
