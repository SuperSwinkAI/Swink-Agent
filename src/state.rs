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
#[non_exhaustive]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    /// Map of changed keys. `Some(v)` means the key was set to `v`;
    /// `None` means the key was removed.
    pub changes: HashMap<String, Option<Value>>,
}

impl StateDelta {
    /// Create an empty delta.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a delta from a complete change map.
    ///
    /// `Some(value)` entries mean the key was set to `value`; `None` entries
    /// mean the key was removed.
    #[must_use]
    pub fn from_changes(changes: HashMap<String, Option<Value>>) -> Self {
        Self { changes }
    }

    /// Record a single change, chainable builder-style.
    ///
    /// Pass `Some(value)` to record a set/update of `key`, or `None` to
    /// record its removal.
    #[must_use]
    pub fn with_change(mut self, key: impl Into<String>, value: Option<Value>) -> Self {
        self.changes.insert(key.into(), value);
        self
    }

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

    /// Layer baseline entries underneath the existing data.
    ///
    /// For each key in `baseline` that is absent from this state, the
    /// baseline value is inserted. Existing entries win: keys already
    /// present keep their current value and are never overridden by the
    /// baseline. Inserted baseline entries do NOT appear in the delta,
    /// mirroring [`Self::with_data`] baseline semantics.
    pub fn apply_baseline(&mut self, baseline: &Self) {
        for (key, value) in &baseline.data {
            if !self.data.contains_key(key) {
                self.data.insert(key.clone(), value.clone());
            }
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
    pub fn restore_from_snapshot(snapshot: Value) -> Result<Self, serde_json::Error> {
        let data: HashMap<String, Value> = serde_json::from_value(snapshot)?;
        Ok(Self {
            data,
            delta: StateDelta::default(),
        })
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
#[path = "state_tests.rs"]
mod tests;
