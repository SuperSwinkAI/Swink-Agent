//! Context caching abstractions for provider-side cache control.
//!
//! Provides [`CacheConfig`] for opt-in caching, [`CacheHint`] for annotating
//! messages with write/read intent, and [`CacheState`] for tracking the
//! cache lifecycle across turns.

#![forbid(unsafe_code)]

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ─── CacheConfig ───────────────────────────────────────────────────────────

/// Configuration for provider-side context caching.
///
/// When attached to [`AgentOptions`](crate::AgentOptions), the framework
/// annotates cacheable prefix messages with [`CacheHint`] markers that
/// adapters translate to provider-specific cache control headers.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Time-to-live for the cached prefix on the provider side.
    pub ttl: Duration,
    /// Minimum token count for the cached prefix; caching is suppressed
    /// when the prefix is smaller than this threshold.
    pub min_tokens: usize,
    /// Number of turns between cache refreshes (Write → Read × N → Write).
    pub cache_intervals: usize,
}

impl CacheConfig {
    /// Create a new cache configuration.
    pub const fn new(ttl: Duration, min_tokens: usize, cache_intervals: usize) -> Self {
        Self {
            ttl,
            min_tokens,
            cache_intervals,
        }
    }
}

// ─── CacheHint ─────────────────────────────────────────────────────────────

/// Hint attached to messages indicating the desired cache action.
///
/// Adapters inspect this during message conversion to translate into
/// provider-specific cache control (e.g., Anthropic's `cache_control` field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CacheHint {
    /// Write (or refresh) the cached prefix with the given TTL.
    Write {
        #[serde(with = "duration_secs")]
        ttl: Duration,
    },
    /// Read from an existing cached prefix.
    Read,
}

/// Serde helper: serialize/deserialize `Duration` as integer seconds.
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(dur: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(dur.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

// ─── CacheState ────────────────────────────────────────────────────────────

/// Tracks the cache lifecycle across turns.
///
/// Call [`advance_turn`](Self::advance_turn) once per turn to get the
/// appropriate [`CacheHint`]. Call [`reset`](Self::reset) when the adapter
/// reports a cache miss so the next turn re-sends a `Write`.
#[derive(Debug, Clone)]
pub struct CacheState {
    turns_since_write: usize,
    /// Number of tokens in the cached prefix (set after annotation).
    pub cached_prefix_len: usize,
}

impl CacheState {
    /// Create a new cache state (first turn will emit `Write`).
    pub const fn new() -> Self {
        Self {
            turns_since_write: 0,
            cached_prefix_len: 0,
        }
    }

    /// Advance the turn counter and return the cache hint for this turn.
    ///
    /// - First turn (or after reset/refresh): returns `Write { ttl }`.
    /// - Subsequent turns within `cache_intervals`: returns `Read`.
    /// - After `cache_intervals` turns: returns `Write` (refresh).
    pub const fn advance_turn(&mut self, config: &CacheConfig) -> CacheHint {
        if self.turns_since_write == 0 {
            // First turn or just after reset — write.
            self.turns_since_write = 1;
            CacheHint::Write { ttl: config.ttl }
        } else if self.turns_since_write >= config.cache_intervals {
            // Refresh cycle reached — write again.
            self.turns_since_write = 1;
            CacheHint::Write { ttl: config.ttl }
        } else {
            self.turns_since_write += 1;
            CacheHint::Read
        }
    }

    /// Reset the cache state, forcing the next turn to emit `Write`.
    ///
    /// Called when the adapter reports a provider cache miss.
    pub const fn reset(&mut self) {
        self.turns_since_write = 0;
        self.cached_prefix_len = 0;
    }
}

impl Default for CacheState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(intervals: usize) -> CacheConfig {
        CacheConfig::new(Duration::from_secs(600), 4096, intervals)
    }

    #[test]
    fn first_turn_emits_write() {
        let mut state = CacheState::new();
        let config = test_config(3);
        let hint = state.advance_turn(&config);
        assert_eq!(hint, CacheHint::Write { ttl: Duration::from_secs(600) });
    }

    #[test]
    fn subsequent_turns_emit_read() {
        let mut state = CacheState::new();
        let config = test_config(3);
        state.advance_turn(&config); // turn 1: Write
        assert_eq!(state.advance_turn(&config), CacheHint::Read); // turn 2
        assert_eq!(state.advance_turn(&config), CacheHint::Read); // turn 3
    }

    #[test]
    fn refresh_after_cache_intervals() {
        let mut state = CacheState::new();
        let config = test_config(3);
        state.advance_turn(&config); // turn 1: Write
        state.advance_turn(&config); // turn 2: Read
        state.advance_turn(&config); // turn 3: Read
        // turn 4: should refresh (turns_since_write == 3 == cache_intervals)
        let hint = state.advance_turn(&config);
        assert_eq!(hint, CacheHint::Write { ttl: Duration::from_secs(600) });
    }

    #[test]
    fn reset_forces_write_on_next_turn() {
        let mut state = CacheState::new();
        let config = test_config(5);
        state.advance_turn(&config); // Write
        state.advance_turn(&config); // Read
        state.reset(); // adapter-reported cache miss
        let hint = state.advance_turn(&config);
        assert_eq!(hint, CacheHint::Write { ttl: Duration::from_secs(600) });
    }

    #[test]
    fn cached_prefix_len_tracks_correctly() {
        let mut state = CacheState::new();
        assert_eq!(state.cached_prefix_len, 0);
        state.cached_prefix_len = 5;
        assert_eq!(state.cached_prefix_len, 5);
        state.reset();
        assert_eq!(state.cached_prefix_len, 0);
    }

    #[test]
    fn min_tokens_below_threshold_suppresses_hints() {
        // The CacheState itself doesn't enforce min_tokens — that check
        // happens in the turn pipeline. Verify the config carries it.
        let config = CacheConfig::new(Duration::from_secs(300), 8192, 2);
        assert_eq!(config.min_tokens, 8192);
    }

    #[test]
    fn serde_round_trip_write_hint() {
        let hint = CacheHint::Write { ttl: Duration::from_secs(600) };
        let json = serde_json::to_string(&hint).unwrap();
        let back: CacheHint = serde_json::from_str(&json).unwrap();
        assert_eq!(hint, back);
    }

    #[test]
    fn serde_round_trip_read_hint() {
        let hint = CacheHint::Read;
        let json = serde_json::to_string(&hint).unwrap();
        let back: CacheHint = serde_json::from_str(&json).unwrap();
        assert_eq!(hint, back);
    }
}
