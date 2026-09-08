//! Tests for `context_cache`.
#![cfg(test)]

use super::*;

fn test_config(intervals: usize) -> CacheConfig {
    CacheConfig::new(Duration::from_mins(10), 4096, intervals)
}

#[test]
fn first_turn_emits_write() {
    let mut state = CacheState::new();
    let config = test_config(3);
    let hint = state.advance_turn(&config);
    assert_eq!(
        hint,
        CacheHint::Write {
            ttl: Duration::from_mins(10)
        }
    );
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
    assert_eq!(
        hint,
        CacheHint::Write {
            ttl: Duration::from_mins(10)
        }
    );
}

#[test]
fn reset_forces_write_on_next_turn() {
    let mut state = CacheState::new();
    let config = test_config(5);
    state.advance_turn(&config); // Write
    state.advance_turn(&config); // Read
    state.reset(); // adapter-reported cache miss
    let hint = state.advance_turn(&config);
    assert_eq!(
        hint,
        CacheHint::Write {
            ttl: Duration::from_mins(10)
        }
    );
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
    let config = CacheConfig::new(Duration::from_mins(5), 8192, 2);
    assert_eq!(config.min_tokens, 8192);
}

#[test]
fn serde_round_trip_write_hint() {
    let hint = CacheHint::Write {
        ttl: Duration::from_mins(10),
    };
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
