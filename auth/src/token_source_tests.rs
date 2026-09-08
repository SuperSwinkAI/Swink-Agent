//! Tests for `token_source`.
#![cfg(test)]

use super::*;

fn ready_refresh(value: &str) -> RefreshFuture<String, String> {
    let value = value.to_string();
    async move {
        Ok(ExpiringValue::new(
            value,
            Instant::now() + Duration::from_secs(300),
        ))
    }
    .boxed()
    .shared()
}

#[test]
fn stale_generation_clear_preserves_newer_refresh() {
    let mut state = InFlightState::new();
    let (old_generation, _) = state.get_or_start_with(|| ready_refresh("old"));

    state.clear_generation(old_generation);
    let (new_generation, _) = state.get_or_start_with(|| ready_refresh("new"));

    state.clear_generation(old_generation);

    assert_eq!(
        state.current.as_ref().map(|current| current.generation),
        Some(new_generation)
    );
}

#[test]
fn expiring_value_debug_redacts_value() {
    let expires_at = Instant::now() + Duration::from_secs(300);
    let value = ExpiringValue::new("LEAK_SENTINEL_ABC123".to_string(), expires_at);

    let debug = format!("{value:?}");

    assert!(
        !debug.contains("LEAK_SENTINEL_ABC123"),
        "cached token leaked: {debug}"
    );
    assert!(
        debug.contains("expires_at"),
        "expiry metadata should remain visible: {debug}"
    );
}
