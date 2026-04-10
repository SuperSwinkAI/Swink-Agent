//! Session key-value state store with delta tracking.
//!
//! Provides [`SessionState`] for per-session structured data that tools can
//! read/write during execution, and [`StateDelta`] for tracking mutations
//! since the last flush. State is shared via `Arc<RwLock<SessionState>>`.
#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── StateDelta ─────────────────────────────────────────────────────────────

/// Record of mutations since the last flush.
///
/// `Some(value)` = set/update, `None` = removed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    /// Map of changed keys. `Some(v)` means the key was set to `v`;
    /// `None` means the key was removed.
    pub changes: HashMap<String, Option<Value>>,
}

impl StateDelta {
    /// True if no changes recorded.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Number of changed keys.
    pub fn len(&self) -> usize {
        self.changes.len()
    }
}

// ─── SessionState ───────────────────────────────────────────────────────────

/// Key-value store with change tracking for session-attached structured data.
///
/// Tools receive an `Arc<RwLock<SessionState>>` during execution and can
/// read/write arbitrary typed values. Changes are tracked in a [`StateDelta`]
/// that is flushed at the end of each turn.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionState {
    data: HashMap<String, Value>,
    #[serde(skip)]
    delta: StateDelta,
}

impl SessionState {
    /// Create a new empty session state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create session state pre-populated with the given data.
    ///
    /// Pre-seeded data does NOT appear in the delta (baseline semantics).
    pub fn with_data(data: HashMap<String, Value>) -> Self {
        Self {
            data,
            delta: StateDelta::default(),
        }
    }

    /// Get a typed value by key. Returns `None` if key is missing or
    /// deserialization fails.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.data
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Get the raw JSON value by key without deserialization.
    pub fn get_raw(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// Set a typed value. Serializes to `Value` and records in delta.
    ///
    /// Returns an error if the value cannot be serialized to JSON.
    pub fn set<T: Serialize>(&mut self, key: &str, value: T) -> Result<(), serde_json::Error> {
        let val = serde_json::to_value(value)?;
        self.data.insert(key.to_string(), val.clone());
        self.delta.changes.insert(key.to_string(), Some(val));
        Ok(())
    }

    /// Remove a key. Records removal in delta. No-op if key absent.
    pub fn remove(&mut self, key: &str) {
        if self.data.remove(key).is_some() {
            self.delta.changes.insert(key.to_string(), None);
        }
    }

    /// Check if a key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// Iterate over all keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.data.keys().map(String::as_str)
    }

    /// Number of key-value pairs.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True if no key-value pairs.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Remove all key-value pairs. Records all existing keys as removed in delta.
    pub fn clear(&mut self) {
        for key in self.data.keys() {
            self.delta.changes.insert(key.clone(), None);
        }
        self.data.clear();
    }

    /// Read-only reference to pending delta.
    pub const fn delta(&self) -> &StateDelta {
        &self.delta
    }

    /// Take the pending delta and reset tracking. Returns the delta.
    pub fn flush_delta(&mut self) -> StateDelta {
        std::mem::take(&mut self.delta)
    }

    /// Snapshot the materialized data as a JSON Value (for persistence).
    pub fn snapshot(&self) -> Value {
        serde_json::to_value(&self.data).expect("HashMap<String, Value> is always serializable")
    }

    /// Restore from a JSON Value snapshot. Returns a new `SessionState` with
    /// empty delta.
    pub fn restore_from_snapshot(snapshot: Value) -> Self {
        let data: HashMap<String, Value> = serde_json::from_value(snapshot).unwrap_or_default();
        Self {
            data,
            delta: StateDelta::default(),
        }
    }
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SessionState>();
    assert_send_sync::<StateDelta>();
    assert_send_sync::<std::sync::Arc<std::sync::RwLock<SessionState>>>();
};

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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

    // ── snapshot / restore ──

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut s = SessionState::new();
        s.set("name", "alice").unwrap();
        s.set("age", 30).unwrap();
        let snap = s.snapshot();
        let s2 = SessionState::restore_from_snapshot(snap);
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
        let s2 = SessionState::restore_from_snapshot(snap);
        assert_eq!(s2.get_raw("profile"), Some(&nested));
    }
}
